use core::fmt::Write;

fn newton_sqrt(x: f64) -> f64 {
    let mut g = x / 2.0;
    for _ in 0..20 { g = (g + x / g) / 2.0; }
    g
}

pub fn run(w: &mut dyn Write) {
    let _ = writeln!(w, "{} {}", newton_sqrt(2.0), newton_sqrt(1e6));
    let a = 0.1f32 + 0.2f32;
    let b = 1.0f32 / 3.0;
    let _ = writeln!(w, "{} {} {}", a, b, a * b);
    let _ = writeln!(w, "{} {}", (16777217u32) as f32, 0.1f64 as f32 as f64);
    let _ = writeln!(w, "{} {} {}", f64::NAN as i32, f64::INFINITY as u8, -1.5f64 as u8);
    let _ = writeln!(w, "{:.3} {:e} {}", core::f64::consts::PI, 1234.5f64, -0.0f64);
    let xs = [3.5f64, -1.25, 9.0, 2.0];
    let s: f64 = xs.iter().sum();
    let mx = xs.iter().cloned().fold(f64::MIN, f64::max);
    let _ = writeln!(w, "{} {}", s, mx);
    let _ = writeln!(w, "{} {} {}", 7.5f64 % 2.0, (-7.5f64) % 2.0, (1..=10).fold(1.0f64, |a, _| a * 2.0));
    let _ = writeln!(w, "{} {}", (1.0f32).to_bits(), f32::from_bits(0x40490fdb));
}
