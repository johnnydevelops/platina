//! Events: `RBXScriptSignal`s whose handlers are Rust closures.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::marker::PhantomData;

use crate::rt::{luau_fn, Handle};
use crate::value::{FromValue, Value};

luau_fn! {
    fn sig_connect(sig: u32, slot: u32, abort: u32, id: u32) -> u32 = "function(sig, slot, abort, id)
        local f, a = FN[slot], FN[abort]
        return rt_href(rt_hget(sig):Connect(function(...)
            local sp = SP
            local ok, err = pcall(f, id, rt_href(table.pack(...)))
            SP = sp
            if not ok then
                a(id)
                error(err, 0)
            end
        end))
    end";
    fn sig_wait(sig: u32) -> u32 = "function(sig) local s = rt_hget(sig) return rt_href(table.pack(rt_yield(s.Wait, s))) end";
    fn conn_disconnect(c: u32) = "function(c) rt_hget(c):Disconnect() end";
    fn conn_connected(c: u32) -> bool = "function(c) return rt_hget(c).Connected end";
}

type Callback = Box<dyn FnMut(Value)>;

struct Slot {
    /// the closure, boxed so its address is stable while the slab grows
    f: *mut dyn FnMut(Value),
    once: bool,
    running: bool,
    /// unregistered while running: free it when the call ends
    removed: bool,
}

/// Rust closures called from Luau, by id.
struct Callbacks(UnsafeCell<Vec<Option<Slot>>>);
unsafe impl Sync for Callbacks {}
static CALLBACKS: Callbacks = Callbacks(UnsafeCell::new(Vec::new()));

fn slots() -> &'static mut Vec<Option<Slot>> {
    unsafe { &mut *CALLBACKS.0.get() }
}

/// Register a closure; `once` closures are released after their first call.
pub(crate) fn register(f: Callback, once: bool) -> u32 {
    let v = slots();
    let slot = Some(Slot { f: Box::into_raw(f), once, running: false, removed: false });
    if let Some(i) = v.iter().position(|s| s.is_none()) {
        v[i] = slot;
        i as u32 + 1
    } else {
        v.push(slot);
        v.len() as u32
    }
}

fn free(i: usize) {
    if let Some(s) = slots()[i].take() {
        drop(unsafe { Box::from_raw(s.f) });
    }
}

pub(crate) fn unregister(id: u32) {
    let i = id as usize - 1;
    match slots().get_mut(i) {
        Some(Some(s)) if s.running => s.removed = true,
        Some(Some(_)) => free(i),
        _ => {}
    }
}

/// End of a call (normal or aborted by a Luau error).
fn finish(i: usize) {
    if let Some(Some(s)) = slots().get_mut(i) {
        s.running = false;
        if s.once || s.removed {
            free(i);
        }
    }
}

/// Called from Luau with a callback id and a handle to the packed arguments. A handler that is
/// already running (its event fired re-entrantly) is skipped.
pub(crate) extern "C" fn dispatch(id: u32, args: u32) {
    let args = unsafe { Value::from_raw(args) };
    let i = id as usize - 1;
    let f = match slots().get_mut(i) {
        Some(Some(s)) if !s.running => {
            s.running = true;
            s.f
        }
        _ => return,
    };
    unsafe { (*f)(args) };
    finish(i);
}

/// Called from Luau when a handler raised an error: the Rust frames were abandoned, so mark the
/// handler idle again (it keeps receiving events).
pub(crate) extern "C" fn dispatch_abort(id: u32) {
    finish(id as usize - 1);
}

pub(crate) fn abort_slot() -> u32 {
    dispatch_abort as extern "C" fn(u32) as usize as u32
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
    /// A signal obtained dynamically (any value with `:Connect` / `:Wait`), e.g.
    /// `Signal::<(f64,)>::from_value(obj.get("Changed"))`. `None` for nil.
    pub fn from_value(v: Value) -> Option<Self> {
        v.into_handle().map(Signal::from_handle)
    }
    /// Run `f` every time the event fires. The handler stays connected until
    /// `Connection::disconnect` (dropping the `Connection` does not disconnect).
    pub fn connect<F: Handler<A>>(&self, mut f: F) -> Connection {
        let id = register(Box::new(move |args: Value| f.call(A::from_pack(&args))), false);
        let c = unsafe { sig_connect(self.h.id(), dispatch_slot(), abort_slot(), id) };
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
