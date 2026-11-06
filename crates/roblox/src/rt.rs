//! Low-level glue: handles and string transfer. Glue functions are Luau snippets attached to
//! `extern "C"` declarations through `#[link_name = "luau::<code>"]`; Platina emits each snippet
//! once and only if it is called. Prelude helpers available to snippets: `rt_href(v)` (store a
//! value, returns its handle id; nil -> 0), `rt_hget(id)`, `rt_hdrop(id)`, `rt_str(ptr, len)`,
//! `rt_str_out(s)` (returns a Rust-heap `(ptr, len)`), `rt_yield(f, ...)` (call a yielding
//! function), and `FN[slot]` (call a Rust function pointer).

use alloc::string::String;

/// Declare Rust functions implemented by Luau code.
#[macro_export]
#[doc(hidden)]
macro_rules! __luau_fn {
    ($($vis:vis fn $name:ident($($a:ident: $t:ty),* $(,)?) $(-> $r:ty)? = $code:literal;)*) => {$(
        #[allow(clippy::missing_safety_doc)]
        #[inline(always)]
        $vis unsafe fn $name($($a: $t),*) $(-> $r)? {
            #[allow(clashing_extern_declarations, improper_ctypes)]
            unsafe extern "C" {
                #[link_name = concat!("luau::", $code)]
                fn imp($($a: $t),*) $(-> $r)?;
            }
            unsafe { imp($($a),*) }
        }
    )*};
}
pub use crate::__luau_fn as luau_fn;

luau_fn! {
    fn h_clone(h: u32) -> u32 = "function(h) return rt_href(rt_hget(h)) end";
    fn h_drop(h: u32) = "rt_hdrop";
    fn h_eq(a: u32, b: u32) -> bool = "function(a, b) return rt_hget(a) == rt_hget(b) end";
    fn h_tostring(h: u32) -> RawStr = "function(h) return rt_str_out(tostring(rt_hget(h))) end";
    fn h_typeof(h: u32) -> RawStr = "function(h) return rt_str_out(typeof(rt_hget(h))) end";
    pub fn instance_new(p: *const u8, n: usize) -> u32 = "function(p, n) return rt_href(Instance.new(rt_str(p, n))) end";
}

/// An owned handle to a Luau value (never nil: absent values are `Option<Handle>`).
#[repr(transparent)]
pub struct Handle(u32);

impl Handle {
    /// # Safety
    /// `id` must be a live, non-zero handle id owned by the caller.
    pub unsafe fn from_raw(id: u32) -> Handle {
        debug_assert!(id != 0);
        Handle(id)
    }
    /// `None` for handle id 0 (nil).
    ///
    /// # Safety
    /// `id` must be 0 or a live handle id owned by the caller.
    pub unsafe fn from_raw_opt(id: u32) -> Option<Handle> {
        if id == 0 { None } else { Some(Handle(id)) }
    }
    pub fn id(&self) -> u32 {
        self.0
    }
    /// Release ownership without dropping the Luau reference.
    pub fn into_raw(self) -> u32 {
        let id = self.0;
        core::mem::forget(self);
        id
    }
    pub fn to_string_luau(&self) -> String {
        unsafe { h_tostring(self.0).into_string() }
    }
    pub fn type_name(&self) -> String {
        unsafe { h_typeof(self.0).into_string() }
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        Handle(unsafe { h_clone(self.0) })
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { h_drop(self.0) }
    }
}

impl PartialEq for Handle {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 || unsafe { h_eq(self.0, other.0) }
    }
}

/// Handle id of an optional value (0 for `None`).
pub fn opt_id<T: crate::Class>(v: Option<&T>) -> u32 {
    v.map_or(0, |v| v.handle().id())
}

/// A string allocated on the Rust heap by `rt_str_out`.
#[repr(C)]
pub struct RawStr {
    ptr: *mut u8,
    len: usize,
}

impl RawStr {
    /// The bytes as a `Vec`.
    pub fn into_bytes(self) -> alloc::vec::Vec<u8> {
        if self.len == 0 {
            return alloc::vec::Vec::new();
        }
        unsafe { alloc::vec::Vec::from_raw_parts(self.ptr, self.len, self.len) }
    }
    /// As a `String`; Luau strings may hold any bytes, so invalid UTF-8 is replaced (U+FFFD).
    pub fn into_string(self) -> String {
        match String::from_utf8(self.into_bytes()) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        }
    }
}

/// Split an `i64` for Luau (`rt_i64_in` joins it) and back.
pub fn i64_parts(v: i64) -> (u32, u32) {
    (v as u32, (v >> 32) as u32)
}
