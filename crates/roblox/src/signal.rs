//! Events: `RBXScriptSignal`s whose handlers are Rust closures.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::marker::PhantomData;

use crate::rt::{luau_fn, Handle};
use crate::value::{FromValue, Value};

luau_fn! {
    fn sig_connect(sig: u32, slot: u32, id: u32) -> u32 = "function(sig, slot, id)
        local f = FN[slot]
        return rt_href(rt_hget(sig):Connect(function(...)
            local sp = SP
            local ok, err = pcall(f, id, rt_href(table.pack(...)))
            SP = sp
            if not ok then error(err, 0) end
        end))
    end";
    fn sig_wait(sig: u32) -> u32 = "function(sig) local s = rt_hget(sig) return rt_href(table.pack(rt_yield(s.Wait, s))) end";
    fn conn_disconnect(c: u32) = "function(c) rt_hget(c):Disconnect() end";
    fn conn_connected(c: u32) -> bool = "function(c) return rt_hget(c).Connected end";
}

type Callback = Box<dyn FnMut(Value)>;

enum Slot {
    Free,
    /// taken out by `dispatch` while it runs; `removed` if unregistered meanwhile
    Busy { removed: bool },
    Cb { f: Callback, once: bool },
}

/// Rust closures called from Luau, by id.
struct Callbacks(UnsafeCell<Vec<Slot>>);
unsafe impl Sync for Callbacks {}
static CALLBACKS: Callbacks = Callbacks(UnsafeCell::new(Vec::new()));

fn slots() -> &'static mut Vec<Slot> {
    unsafe { &mut *CALLBACKS.0.get() }
}

/// Register a closure; `once` closures are released after their first call.
pub(crate) fn register(f: Callback, once: bool) -> u32 {
    let v = slots();
    let slot = Slot::Cb { f, once };
    if let Some(i) = v.iter().position(|s| matches!(s, Slot::Free)) {
        v[i] = slot;
        i as u32 + 1
    } else {
        v.push(slot);
        v.len() as u32
    }
}

pub(crate) fn unregister(id: u32) {
    if let Some(s) = slots().get_mut(id as usize - 1) {
        match s {
            Slot::Busy { removed } => *removed = true,
            _ => *s = Slot::Free,
        }
    }
}

/// Called from Luau with a callback id and a handle to the packed arguments. The closure is
/// taken out while it runs, so handlers may fire re-entrantly or disconnect themselves.
pub(crate) extern "C" fn dispatch(id: u32, args: u32) {
    let args = unsafe { Value::from_raw(args) };
    let i = id as usize - 1;
    let Some(s) = slots().get_mut(i) else { return };
    if !matches!(s, Slot::Cb { .. }) {
        return; // unregistered, or already running (a handler can't re-enter itself)
    }
    let Slot::Cb { mut f, once } = core::mem::replace(s, Slot::Busy { removed: false }) else { unreachable!() };
    f(args);
    let s = &mut slots()[i];
    *s = match s {
        Slot::Busy { removed: false } if !once => Slot::Cb { f, once },
        _ => Slot::Free,
    };
}

/// The function-table slot of `dispatch`, for glue code (`FN[slot](id, args)`).
pub(crate) fn dispatch_slot() -> u32 {
    dispatch as extern "C" fn(u32, u32) as usize as u32
}

/// Event arguments, converted from the packed Luau values.
pub trait SignalArgs: Sized {
    fn from_pack(pack: &Value) -> Self;
}

/// One event argument at position `i` of the packed arguments.
pub trait FromPack: Sized {
    fn from_pack_at(pack: &Value, i: u32) -> Self;
}

impl<T: FromValue> FromPack for T {
    fn from_pack_at(pack: &Value, i: u32) -> Self {
        T::from_value(pack.index(i + 1))
    }
}

/// The remaining arguments of an event with variable arguments (`...`).
#[derive(Clone, Debug, Default)]
pub struct Varargs(pub Vec<Value>);

impl core::ops::Deref for Varargs {
    type Target = [Value];
    fn deref(&self) -> &[Value] {
        &self.0
    }
}

impl FromPack for Varargs {
    fn from_pack_at(pack: &Value, i: u32) -> Self {
        Varargs((i + 1..=pack.len()).map(|k| pack.index(k)).collect())
    }
}

/// A handler for a signal with arguments `A`: any `FnMut(T1, T2, ...)`.
pub trait Handler<A>: 'static {
    fn call(&mut self, args: A);
}

macro_rules! tuples {
    ($(($($t:ident $i:tt),*))*) => {$(
        impl<$($t: FromPack),*> SignalArgs for ($($t,)*) {
            #[allow(unused_variables)]
            fn from_pack(pack: &Value) -> Self {
                ($($t::from_pack_at(pack, $i),)*)
            }
        }
        impl<Func: FnMut($($t),*) + 'static, $($t),*> Handler<($($t,)*)> for Func {
            #[allow(non_snake_case)]
            fn call(&mut self, ($($t,)*): ($($t,)*)) {
                self($($t),*)
            }
        }
    )*};
}
tuples! {
    ()
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
    (A 0, B 1, C 2, D 3, E 4)
    (A 0, B 1, C 2, D 3, E 4, F 5)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7)
}

/// An `RBXScriptSignal` with arguments `A` (a tuple).
pub struct Signal<A> {
    h: Handle,
    _a: PhantomData<fn(A)>,
}

impl<A: SignalArgs + 'static> Signal<A> {
    #[doc(hidden)]
    pub fn from_handle(h: Handle) -> Self {
        Signal { h, _a: PhantomData }
    }
    /// Run `f` every time the event fires. The handler stays connected until
    /// `Connection::disconnect` (dropping the `Connection` does not disconnect).
    pub fn connect<F: Handler<A>>(&self, mut f: F) -> Connection {
        let id = register(Box::new(move |args: Value| f.call(A::from_pack(&args))), false);
        let c = unsafe { sig_connect(self.h.id(), dispatch_slot(), id) };
        Connection { h: unsafe { Handle::from_raw(c) }, id }
    }
    /// Yield until the event fires, returning its arguments.
    pub fn wait(&self) -> A {
        let pack = unsafe { Value::from_raw(sig_wait(self.h.id())) };
        A::from_pack(&pack)
    }
}

/// A connected handler.
pub struct Connection {
    h: Handle,
    id: u32,
}

impl Connection {
    pub fn disconnect(self) {
        unsafe { conn_disconnect(self.h.id()) };
        unregister(self.id);
    }
    pub fn is_connected(&self) -> bool {
        unsafe { conn_connected(self.h.id()) }
    }
}
