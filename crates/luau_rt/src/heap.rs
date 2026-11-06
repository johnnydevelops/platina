//! Global allocator: forwards to the Luau runtime heap (shared with std's `System`).

use core::alloc::{GlobalAlloc, Layout};

unsafe extern "C" {
    fn __luau_alloc(size: usize, align: usize) -> *mut u8;
    fn __luau_alloc_zeroed(size: usize, align: usize) -> *mut u8;
    fn __luau_dealloc(ptr: *mut u8, size: usize, align: usize);
    fn __luau_realloc(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8;
}

pub struct LuauAlloc;

impl LuauAlloc {
    pub const fn new() -> Self {
        LuauAlloc
    }
}

impl Default for LuauAlloc {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for LuauAlloc {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { __luau_alloc(l.size(), l.align()) }
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { __luau_alloc_zeroed(l.size(), l.align()) }
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { __luau_dealloc(p, l.size(), l.align()) }
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        unsafe { __luau_realloc(p, l.size(), l.align(), new_size) }
    }
}
