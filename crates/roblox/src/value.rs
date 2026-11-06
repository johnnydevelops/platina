//! `Value`: any Luau value, for dynamic access to APIs the typed bindings don't cover.

use alloc::string::String;
use alloc::vec::Vec;

use crate::datatypes::*;
use crate::rt::{luau_fn, Handle, RawStr};
use crate::Class;

luau_fn! {
    fn v_num(x: f64) -> u32 = "rt_href";
    fn v_bool(x: bool) -> u32 = "rt_href";
    fn v_str(p: *const u8, n: usize) -> u32 = "function(p, n) return rt_href(rt_str(p, n)) end";
    fn v_v3(v: Vector3) -> u32 = "function(x, y, z) return rt_href(Vector3.new(x, y, z)) end";
    fn v_v2(v: Vector2) -> u32 = "function(x, y) return rt_href(Vector2.new(x, y)) end";
    fn v_c3(v: Color3) -> u32 = "function(r, g, b) return rt_href(Color3.new(r, g, b)) end";
    fn v_udim2(v: UDim2) -> u32 = "function(a, b, c, d) return rt_href(UDim2.new(a, b, c, d)) end";
    fn v_cf(v: CFrame) -> u32 = "function(...) return rt_href(CFrame.new(...)) end";
    fn v_table() -> u32 = "function() return rt_href({}) end";
    fn v_to_num(h: u32) -> f64 = "function(h) return tonumber(rt_hget(h)) or 0 end";
    fn v_to_bool(h: u32) -> bool = "function(h) return not not rt_hget(h) end";
    fn v_to_str(h: u32) -> RawStr = "function(h) local v = rt_hget(h) if type(v) == 'buffer' then v = buffer.tostring(v) end return rt_str_out(tostring(v)) end";
    fn v_to_v3(h: u32) -> Vector3 = "function(h) local v = rt_hget(h) return v.X, v.Y, v.Z end";
    fn v_to_v2(h: u32) -> Vector2 = "function(h) local v = rt_hget(h) return v.X, v.Y end";
    fn v_to_c3(h: u32) -> Color3 = "function(h) local v = rt_hget(h) return v.R, v.G, v.B end";
    fn v_to_udim2(h: u32) -> UDim2 = "function(h) local v = rt_hget(h) return v.X.Scale, v.X.Offset, v.Y.Scale, v.Y.Offset end";
    fn v_to_cf(h: u32) -> CFrame = "function(h) return rt_hget(h):GetComponents() end";
    fn v_is_a(h: u32, p: *const u8, n: usize) -> bool = "function(h, p, n) local v = rt_hget(h) return typeof(v) == 'Instance' and v:IsA(rt_str(p, n)) end";
    fn v_get(h: u32, p: *const u8, n: usize) -> u32 = "function(h, p, n) return rt_href(rt_hget(h)[rt_str(p, n)]) end";
    fn v_set(h: u32, p: *const u8, n: usize, v: u32) = "function(h, p, n, v) rt_hget(h)[rt_str(p, n)] = rt_hget(v) end";
    fn v_index(h: u32, i: u32) -> u32 = "function(h, i) return rt_href(rt_hget(h)[i]) end";
    fn v_set_index(h: u32, i: u32, v: u32) = "function(h, i, v) rt_hget(h)[i] = rt_hget(v) end";
    fn v_len(h: u32) -> u32 = "function(h) local v = rt_hget(h) if type(v) == 'buffer' then return buffer.len(v) end if type(v) == 'table' and v.n then return v.n end return #v end";
    fn v_call(f: u32, args: u32) -> u32 = "function(f, a) local t = rt_hget(a) return rt_href(table.pack(rt_hget(f)(table.unpack(t, 1, t.n)))) end";
    fn v_call_method(h: u32, p: *const u8, n: usize, args: u32) -> u32 = "function(h, p, n, a) local o = rt_hget(h) local t = rt_hget(a) return rt_href(table.pack(o[rt_str(p, n)](o, table.unpack(t, 1, t.n)))) end";
    fn v_call_method_yield(h: u32, p: *const u8, n: usize, args: u32) -> u32 = "function(h, p, n, a) local o = rt_hget(h) local t = rt_hget(a) return rt_href(table.pack(rt_yield(o[rt_str(p, n)], o, table.unpack(t, 1, t.n)))) end";
    fn v_pack(n: u32) -> u32 = "function(n) return rt_href({n = n}) end";
    fn v_global(p: *const u8, n: usize) -> u32 = "function(p, n) local name = rt_str(p, n) local v = _G[name] if v == nil then v = getfenv()[name] end return rt_href(v) end";
    fn v_require(h: u32) -> u32 = "function(h) return rt_href(require(rt_hget(h))) end";
    fn v_buffer_from(p: *const u8, n: usize) -> u32 = "function(p, n) local b = buffer.create(n) buffer.copy(b, 0, MEM, p, n) return rt_href(b) end";
    fn v_copy_to(h: u32, at: u32, p: *const u8, n: usize) = "function(h, at, p, n) buffer.copy(rt_hget(h), at, MEM, p, n) end";
    fn v_bytes_into(h: u32, p: *mut u8, n: usize) = "function(h, p, n) local v = rt_hget(h) if type(v) == 'string' then buffer.writestring(MEM, p, v, n) else buffer.copy(MEM, p, v, 0, n) end end";
    fn v_byte_len(h: u32) -> u32 = "function(h) local v = rt_hget(h) if type(v) == 'string' then return #v end return buffer.len(v) end";
}

/// Any Luau value (or nil).
#[derive(Clone, PartialEq)]
pub struct Value(Option<Handle>);

impl Value {
    pub const NIL: Value = Value(None);

    #[doc(hidden)]
    pub unsafe fn from_raw(id: u32) -> Value {
        Value(unsafe { Handle::from_raw_opt(id) })
    }
    #[doc(hidden)]
    pub fn raw(&self) -> u32 {
        self.0.as_ref().map_or(0, |h| h.id())
    }
    pub fn is_nil(&self) -> bool {
        self.0.is_none()
    }
    /// A new empty table.
    pub fn table() -> Value {
        unsafe { Value::from_raw(v_table()) }
    }
    /// A global (`game`, `Enum`, `math`, ...).
    pub fn global(name: &str) -> Value {
        unsafe { Value::from_raw(v_global(name.as_ptr(), name.len())) }
    }
    /// `typeof(value)`.
    pub fn type_name(&self) -> String {
        match &self.0 {
            Some(h) => h.type_name(),
            None => String::from("nil"),
        }
    }
    /// `value[key]`.
    pub fn get(&self, key: &str) -> Value {
        unsafe { Value::from_raw(v_get(self.raw(), key.as_ptr(), key.len())) }
    }
    /// `value[key] = v`.
    pub fn set(&self, key: &str, v: impl IntoValue) {
        let v = v.into_value();
        unsafe { v_set(self.raw(), key.as_ptr(), key.len(), v.raw()) }
    }
    /// `value[i]` (1-based, like Luau).
    pub fn index(&self, i: u32) -> Value {
        unsafe { Value::from_raw(v_index(self.raw(), i)) }
    }
    pub fn set_index(&self, i: u32, v: impl IntoValue) {
        let v = v.into_value();
        unsafe { v_set_index(self.raw(), i, v.raw()) }
    }
    /// `#value` (bytes for a buffer).
    pub fn len(&self) -> u32 {
        unsafe { v_len(self.raw()) }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Call a function value; returns the results packed (`table.pack`).
    pub fn call(&self, args: &[Value]) -> Value {
        let a = pack(args);
        unsafe { Value::from_raw(v_call(self.raw(), a.raw())) }
    }
    /// `value:name(args...)`; returns the results packed (`.index(1)` is the first).
    pub fn call_method(&self, name: &str, args: &[Value]) -> Value {
        let a = pack(args);
        unsafe { Value::from_raw(v_call_method(self.raw(), name.as_ptr(), name.len(), a.raw())) }
    }
    /// Like `call_method`, for methods that may yield (`WaitForChild`, `InvokeServer`, ...).
    pub fn call_method_yielding(&self, name: &str, args: &[Value]) -> Value {
        let a = pack(args);
        unsafe { Value::from_raw(v_call_method_yield(self.raw(), name.as_ptr(), name.len(), a.raw())) }
    }
    /// `require(module)` for a ModuleScript value.
    pub fn require(module: &impl IntoValue) -> Value {
        let m = module.into_value();
        unsafe { Value::from_raw(v_require(m.raw())) }
    }
    /// A new Luau `buffer` holding a copy of `bytes`.
    pub fn buffer_from(bytes: &[u8]) -> Value {
        unsafe { Value::from_raw(v_buffer_from(bytes.as_ptr(), bytes.len())) }
    }
    /// Copy `bytes` into this buffer at byte offset `at`.
    pub fn buffer_write(&self, at: u32, bytes: &[u8]) {
        unsafe { v_copy_to(self.raw(), at, bytes.as_ptr(), bytes.len()) }
    }
    /// The bytes of a string or buffer value.
    pub fn to_bytes(&self) -> Vec<u8> {
        let n = unsafe { v_byte_len(self.raw()) } as usize;
        let mut v = alloc::vec![0u8; n];
        unsafe { v_bytes_into(self.raw(), v.as_mut_ptr(), n) };
        v
    }
    pub fn to_f64(&self) -> f64 {
        unsafe { v_to_num(self.raw()) }
    }
    pub fn to_bool(&self) -> bool {
        unsafe { v_to_bool(self.raw()) }
    }
    /// `tostring(value)` (a buffer's contents for buffers).
    pub fn to_string_luau(&self) -> String {
        unsafe { v_to_str(self.raw()).into_string() }
    }
    /// The value as an instance of class `T`, if it is one.
    pub fn to_instance<T: Class>(&self) -> Option<T> {
        let h = self.0.as_ref()?;
        if unsafe { v_is_a(h.id(), T::NAME.as_ptr(), T::NAME.len()) } { Some(T::from_handle(h.clone())) } else { None }
    }
}

impl core::fmt::Debug for Value {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Value({})", self.to_string_luau())
    }
}

impl Value {
    /// Pack values into a table `{n = count, ...}` (an argument list).
    pub fn pack(args: &[Value]) -> Value {
        pack(args)
    }
}

/// Pack values as an argument list (`{n = count, ...}`).
fn pack(args: &[Value]) -> Value {
    let t = unsafe { Value::from_raw(v_pack(args.len() as u32)) };
    for (i, a) in args.iter().enumerate() {
        unsafe { v_set_index(t.raw(), i as u32 + 1, a.raw()) };
    }
    t
}

/// Conversion into a Luau value.
pub trait IntoValue {
    fn into_value(&self) -> Value;
}

/// Conversion from a Luau value (lenient: mismatched values convert to a default).
pub trait FromValue: Sized {
    fn from_value(v: Value) -> Self;
}

impl IntoValue for Value {
    fn into_value(&self) -> Value {
        self.clone()
    }
}
impl<T: IntoValue + ?Sized> IntoValue for &T {
    fn into_value(&self) -> Value {
        (**self).into_value()
    }
}
impl IntoValue for f64 {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_num(*self)) }
    }
}
impl IntoValue for f32 {
    fn into_value(&self) -> Value {
        (*self as f64).into_value()
    }
}
impl IntoValue for i32 {
    fn into_value(&self) -> Value {
        (*self as f64).into_value()
    }
}
impl IntoValue for u32 {
    fn into_value(&self) -> Value {
        (*self as f64).into_value()
    }
}
impl IntoValue for bool {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_bool(*self)) }
    }
}
impl IntoValue for str {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_str(self.as_ptr(), self.len())) }
    }
}
impl IntoValue for String {
    fn into_value(&self) -> Value {
        self.as_str().into_value()
    }
}
impl IntoValue for Vector3 {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_v3(*self)) }
    }
}
impl IntoValue for Vector2 {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_v2(*self)) }
    }
}
impl IntoValue for Color3 {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_c3(*self)) }
    }
}
impl IntoValue for UDim2 {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_udim2(*self)) }
    }
}
impl IntoValue for CFrame {
    fn into_value(&self) -> Value {
        unsafe { Value::from_raw(v_cf(*self)) }
    }
}

impl FromValue for Value {
    fn from_value(v: Value) -> Self {
        v
    }
}
impl FromValue for f64 {
    fn from_value(v: Value) -> Self {
        v.to_f64()
    }
}
impl FromValue for f32 {
    fn from_value(v: Value) -> Self {
        v.to_f64() as f32
    }
}
impl FromValue for i32 {
    fn from_value(v: Value) -> Self {
        v.to_f64() as i32
    }
}
impl FromValue for i64 {
    fn from_value(v: Value) -> Self {
        v.to_f64() as i64
    }
}
impl FromValue for u32 {
    fn from_value(v: Value) -> Self {
        v.to_f64() as u32
    }
}
impl FromValue for bool {
    fn from_value(v: Value) -> Self {
        v.to_bool()
    }
}
impl FromValue for String {
    fn from_value(v: Value) -> Self {
        v.to_string_luau()
    }
}
impl FromValue for Vector3 {
    fn from_value(v: Value) -> Self {
        unsafe { v_to_v3(v.raw()) }
    }
}
impl FromValue for Vector2 {
    fn from_value(v: Value) -> Self {
        unsafe { v_to_v2(v.raw()) }
    }
}
impl FromValue for Color3 {
    fn from_value(v: Value) -> Self {
        unsafe { v_to_c3(v.raw()) }
    }
}
impl FromValue for UDim2 {
    fn from_value(v: Value) -> Self {
        unsafe { v_to_udim2(v.raw()) }
    }
}
impl FromValue for CFrame {
    fn from_value(v: Value) -> Self {
        unsafe { v_to_cf(v.raw()) }
    }
}
impl<T: Class> FromValue for Option<T> {
    fn from_value(v: Value) -> Self {
        v.to_instance::<T>()
    }
}
