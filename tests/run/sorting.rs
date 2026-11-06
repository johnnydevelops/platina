use alloc::vec::Vec;
use core::fmt::Write;

pub fn run(w: &mut dyn Write) {
    for n in [5u32, 17, 33, 100, 1000] {
        let mut v: Vec<u32> = (0..n).map(|i| i.wrapping_mul(2654435761) ^ (i >> 3)).collect();
        v.sort_unstable();
        let ok = v.windows(2).all(|p| p[0] <= p[1]);
        let _ = writeln!(w, "{n} sorted={ok} first={} mid={} last={}", v[0], v[(n / 2) as usize], v[(n - 1) as usize]);
    }
    let mut s: Vec<i64> = (0..300).map(|i: i64| (i * 7919) % 1000 - 500).collect();
    s.sort();
    let _ = writeln!(w, "{:?}", &s[..8]);
    let mut words = alloc::vec!["pear", "apple", "fig", "banana", "cherry"];
    words.sort_by_key(|w| w.len());
    let _ = writeln!(w, "{:?}", words);
}
