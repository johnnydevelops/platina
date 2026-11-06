//! Runtime support for Rust programs compiled to Luau.
#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod heap;

#[cfg(all(feature = "alloc", feature = "global-allocator"))]
#[global_allocator]
static GLOBAL: heap::LuauAlloc = heap::LuauAlloc::new();

use core::fmt::{self, Write};

unsafe extern "C" {
    fn __luau_print(ptr: *const u8, len: usize);
    fn __luau_trap(ptr: *const u8, len: usize) -> !;
    fn __luau_clock() -> f64;
}

/// Print a line to the Luau output (`print`).
pub fn print_line(s: &str) {
    unsafe { __luau_print(s.as_ptr(), s.len()) }
}

/// Seconds since an arbitrary point (`os.clock()`).
pub fn clock() -> f64 {
    unsafe { __luau_clock() }
}

/// Raise a Luau error with the given message.
pub fn trap(msg: &str) -> ! {
    unsafe { __luau_trap(msg.as_ptr(), msg.len()) }
}

/// Line-buffered writer to Luau `print`.
pub struct Out {
    buf: [u8; 1024],
    len: usize,
}

impl Out {
    pub const fn new() -> Self {
        Out { buf: [0; 1024], len: 0 }
    }
    pub fn flush(&mut self) {
        if self.len > 0 {
            let s = unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) };
            print_line(s);
            self.len = 0;
        }
    }
    fn push(&mut self, b: u8) {
        if self.len == self.buf.len() {
            self.flush();
        }
        self.buf[self.len] = b;
        self.len += 1;
    }
}

impl Default for Out {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Out {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if b == b'\n' {
                // print emits its own newline
                let s = unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) };
                print_line(s);
                self.len = 0;
            } else {
                self.push(b);
            }
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {{
        let mut o = $crate::Out::new();
        let _ = ::core::fmt::Write::write_fmt(&mut o, format_args!($($arg)*));
        o.flush();
    }};
}

struct Fixed<'a> {
    buf: &'a mut [u8],
    len: usize,
}
impl Write for Fixed<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

#[cfg(feature = "panic-handler")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let mut buf = [0u8; 512];
    let mut w = Fixed { buf: &mut buf, len: 0 };
    let _ = write!(w, "{}", info.message());
    if let Some(loc) = info.location() {
        let _ = write!(w, " at {}:{}:{}", loc.file(), loc.line(), loc.column());
    }
    let len = w.len;
    let s = unsafe { core::str::from_utf8_unchecked(&buf[..len]) };
    trap(s)
}
