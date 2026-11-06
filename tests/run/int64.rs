use core::fmt::Write;

fn fib(n: u32) -> u64 {
    let (mut a, mut b) = (0u64, 1u64);
    for _ in 0..n { let t = a.wrapping_add(b); a = b; b = t; }
    a
}

fn xorshift(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

pub fn run(w: &mut dyn Write) {
    let _ = writeln!(w, "{} {} {}", fib(50), fib(90), fib(100));
    let mut x = 88172645463325252u64;
    for _ in 0..5 { x = xorshift(x); let _ = writeln!(w, "{}", x); }
    let a: i64 = -123456789012345;
    let b: i64 = 987654321;
    let _ = writeln!(w, "{} {} {} {}", a * 3, a / b, a % b, a >> 5);
    let _ = writeln!(w, "{} {}", (a as u64) >> 40, (a as i32));
    let c: u64 = u64::MAX;
    let _ = writeln!(w, "{} {} {:?}", c / 7, c % 1000, c.checked_add(1));
    let _ = writeln!(w, "{} {}", i64::MIN, i64::MAX);
    let _ = writeln!(w, "{} {} {}", 1u64 << 63, (3u64 << 40).leading_zeros(), (1u64 << 45).trailing_zeros());
    let f = 1.5e15f64;
    let _ = writeln!(w, "{} {} {}", f as u64, (-f) as i64, (a as f64));
    let _ = writeln!(w, "{:x} {:o}", 0xdead_beef_cafe_u64, 1u64 << 50);
    let m = 0x1234_5678_9abc_def0u64.wrapping_mul(0x0fed_cba9_8765_4321);
    let _ = writeln!(w, "{:#x}", m);
}
