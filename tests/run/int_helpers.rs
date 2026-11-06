use core::fmt::Write;
use core::hint::black_box;

pub fn run(w: &mut dyn Write) {
    for n in [1usize, 2, 3, 4, 5, 7, 8, 9, 255, 256, 257] {
        let n = black_box(n);
        let p = n.next_power_of_two();
        let tz = p.trailing_zeros();
        let s = 1usize << tz;
        let _ = writeln!(w, "{n} npot={p} tz={tz} stride={s} lz={} ilog2={} cp={:?}", (n as u32).leading_zeros(), n.ilog2(), n.checked_next_power_of_two());
    }
    for x in [0u64, 1, 0x8000_0000, 0x1_0000_0000, u64::MAX] {
        let x = black_box(x);
        let _ = writeln!(w, "{x} {} {} {} {}", x.leading_zeros(), x.trailing_zeros(), x.count_ones(), x.swap_bytes());
    }
    for x in [0u8, 1, 0x80, 0xff] {
        let x = black_box(x);
        let _ = writeln!(w, "{x} {} {} {} {}", x.leading_zeros(), x.trailing_zeros(), x.reverse_bits(), x.rotate_left(3));
    }
    for x in [0u16, 1, 0x8000, 0x1234] {
        let x = black_box(x);
        let _ = writeln!(w, "{x} {} {} {}", x.leading_zeros(), x.trailing_zeros(), x.swap_bytes());
    }
}
