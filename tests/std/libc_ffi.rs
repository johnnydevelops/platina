// Regression test: C library functions that crates reach through FFI declarations, with no
// libc on Roblox: `strlen` (`CStr::from_ptr`, used by zlib-rs's error messages), `memcmp`, `bcmp`.

use std::ffi::{CStr, c_char};

unsafe extern "C" {
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

pub fn run() {
    let raw = b"hello, world\0";
    let c = unsafe { CStr::from_ptr(raw.as_ptr() as *const c_char) };
    println!("cstr len {} text {:?}", c.to_bytes().len(), c.to_str());
    let empty = unsafe { CStr::from_ptr(b"\0".as_ptr() as *const c_char) };
    println!("empty len {}", empty.to_bytes().len());
    let (x, y) = (*b"abcd", *b"abce");
    unsafe {
        // the sign is all C promises for memcmp
        println!("memcmp {} {} {}", memcmp(x.as_ptr(), y.as_ptr(), 4).signum(), memcmp(y.as_ptr(), x.as_ptr(), 4).signum(), memcmp(x.as_ptr(), y.as_ptr(), 3));
        println!("bcmp equal {} differ {}", bcmp(x.as_ptr(), x.as_ptr(), 4), bcmp(x.as_ptr(), y.as_ptr(), 4) != 0);
    }
}
