use core::fmt::Write;

fn checked(a: u8, b: u8) -> Option<u8> { a.checked_add(b) }

pub fn run(w: &mut dyn Write) {
    let _ = writeln!(w, "{:?} {:?}", checked(200, 50), checked(200, 60));
    let _ = writeln!(w, "{:?}", "123".parse::<u32>());
    let _ = writeln!(w, "{:?}", "12x".parse::<u32>().is_err());
    let _ = writeln!(w, "{:?}", "-42".parse::<i64>());
}
