use alloc::vec::Vec;
use core::fmt::Write;
use core::hint::black_box;
use core::iter;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Id(u32);

pub fn run(w: &mut dyn Write) {
    let mut v: Vec<Id> = Vec::new();
    for k in 0..3u32 {
        let n = black_box(4usize);
        v.extend(iter::repeat(Id(k)).take(n));
        let _ = writeln!(w, "len {}", v.len());
    }
    let _ = writeln!(w, "{:?}", v);
    let mut u: Vec<u8> = Vec::new();
    u.extend(iter::repeat(7u8).take(black_box(5)));
    u.extend_from_slice(&[1, 2, 3]);
    u.extend((0..4).map(|x| x * 2));
    let _ = writeln!(w, "{:?}", u);
    let mut s: Vec<u64> = alloc::vec![1, 2];
    s.extend(iter::repeat_n(9u64, black_box(3)));
    s.resize(7, 5);
    let _ = writeln!(w, "{:?}", s);
}
