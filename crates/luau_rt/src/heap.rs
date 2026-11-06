//! Global allocator over the Luau `buffer` linear memory.
//!
//! Power-of-two size classes with intrusive free lists, carved from a bump region that grows
//! the buffer on demand. Blocks of class `c` are aligned to `min(c, PAGE)`, so any alignment up
//! to the class size is satisfied.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;

unsafe extern "C" {
    fn __luau_mem_size() -> usize;
    fn __luau_mem_grow(additional: usize) -> usize;
    fn __luau_heap_base() -> usize;
}

const MIN_CLASS: usize = 3; // 8 bytes
const NUM_CLASSES: usize = 20; // up to 4 MiB
const PAGE: usize = 4096;

struct State {
    init: bool,
    bump: usize,
    end: usize,
    free: [usize; NUM_CLASSES],
}

pub struct LuauAlloc {
    st: UnsafeCell<State>,
}

unsafe impl Sync for LuauAlloc {}

impl LuauAlloc {
    pub const fn new() -> Self {
        LuauAlloc { st: UnsafeCell::new(State { init: false, bump: 0, end: 0, free: [0; NUM_CLASSES] }) }
    }
}

impl Default for LuauAlloc {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn class_of(size: usize, align: usize) -> usize {
    let n = if size > align { size } else { align };
    let n = if n < (1 << MIN_CLASS) { 1 << MIN_CLASS } else { n };
    (usize::BITS - (n - 1).leading_zeros()) as usize - MIN_CLASS
}

impl State {
    unsafe fn bump_alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        if !self.init {
            self.init = true;
            self.bump = unsafe { __luau_heap_base() };
            self.end = unsafe { __luau_mem_size() };
        }
        let start = (self.bump + align - 1) & !(align - 1);
        let new_bump = start + size;
        if new_bump > self.end {
            let need = new_bump - self.end;
            let grow = if need > self.end / 2 { need } else { self.end / 2 };
            let grow = (grow + PAGE - 1) & !(PAGE - 1);
            unsafe { __luau_mem_grow(grow) };
            self.end += grow;
        }
        self.bump = new_bump;
        start as *mut u8
    }
}

unsafe impl GlobalAlloc for LuauAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let st = unsafe { &mut *self.st.get() };
        let c = class_of(layout.size(), layout.align());
        if c >= NUM_CLASSES {
            return unsafe { st.bump_alloc(layout.size(), layout.align().max(8)) };
        }
        let head = st.free[c];
        if head != 0 {
            st.free[c] = unsafe { *(head as *const usize) };
            return head as *mut u8;
        }
        let size = 1usize << (c + MIN_CLASS);
        let align = if size < PAGE { size } else { PAGE };
        unsafe { st.bump_alloc(size, align) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let st = unsafe { &mut *self.st.get() };
        let c = class_of(layout.size(), layout.align());
        if c >= NUM_CLASSES {
            return; // huge blocks are not reused
        }
        unsafe { *(ptr as *mut usize) = st.free[c] };
        st.free[c] = ptr as usize;
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_c = class_of(layout.size(), layout.align());
        let new_c = class_of(new_size, layout.align());
        if old_c == new_c && old_c < NUM_CLASSES {
            return ptr;
        }
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let np = unsafe { self.alloc(new_layout) };
        let n = if layout.size() < new_size { layout.size() } else { new_size };
        unsafe { core::ptr::copy_nonoverlapping(ptr, np, n) };
        unsafe { self.dealloc(ptr, layout) };
        np
    }
}
