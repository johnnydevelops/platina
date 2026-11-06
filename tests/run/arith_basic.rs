use core::fmt::Write;

fn collatz(mut n: u32) -> u32 {
    let mut steps = 0;
    while n != 1 {
        n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 };
        steps += 1;
    }
    steps
}

fn gcd(a: i32, b: i32) -> i32 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

pub fn run(w: &mut dyn Write) {
    let _ = writeln!(w, "{}", collatz(27));
    let _ = writeln!(w, "{}", gcd(-48, 18));
    let x: i32 = -7;
    let _ = writeln!(w, "{} {} {} {}", x / 2, x % 2, x >> 1, (x as u32) >> 28);
    let y: u8 = 200;
    let _ = writeln!(w, "{} {} {}", y.wrapping_add(100), y.wrapping_mul(3), !y);
    let z: i16 = -300;
    let _ = writeln!(w, "{} {} {}", z as u8, z as i8, z as u32);
    let f = 2.5f64;
    let _ = writeln!(w, "{} {} {}", f as i32, (-f) as u32, 1e10 as i32);
}
