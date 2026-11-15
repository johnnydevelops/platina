// Regression test: wrapping integer arithmetic carried around a loop. The range pass once gave
// the loop-carried hash an unsoundly narrow interval and replaced its wrap by a one-step
// `WrapNear`, while `h * 31` can be off by up to 31 multiples of 2^32 (Luau printed 0).
use alloc::string::String;
use core::fmt::Write;
use core::hint::black_box;

#[inline(never)]
fn jh_bytes(s: &[u8]) -> i32 {
    let mut h = 0i32;
    for &u in s {
        h = h.wrapping_mul(31).wrapping_add(u as i32);
    }
    h
}

/// Java's `String.hashCode`.
#[inline(never)]
fn java_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

#[inline(never)]
fn mul_big(seed: u32, n: u32) -> (u32, i32, u64) {
    let (mut a, mut b, mut c) = (seed, seed as i32, seed as u64);
    for i in 0..n {
        a = a.wrapping_mul(2654435761).wrapping_add(i);
        b = b.wrapping_mul(-1640531535).wrapping_sub(i as i32);
        c = c.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    (a, b, c)
}

pub fn run(w: &mut dyn Write) {
    let words = ["", "a", "oak_log", "mineable/pickaxe", "minecraft:x", "minecraft:stone_bricks",
                 "the quick brown fox jumps over the lazy dog, again and again and again"];
    for s in words {
        let s = black_box(s);
        let _ = writeln!(w, "{:?}: bytes {} java {}", s, jh_bytes(s.as_bytes()), java_hash(s));
    }
    let mut long = String::new();
    for i in 0..300 {
        long.push(char::from(b'a' + (i % 26) as u8));
        if i % 50 == 49 {
            let _ = writeln!(w, "len {}: {} {}", long.len(), jh_bytes(black_box(long.as_bytes())), java_hash(black_box(&long)));
        }
    }
    let _ = writeln!(w, "non-ascii: {}", java_hash(black_box("héllo wörld ✓ 𝄞")));
    for n in [0, 1, 7, 100, 1000] {
        let _ = writeln!(w, "mul_big {n}: {:?}", mul_big(black_box(12345), black_box(n)));
    }
}
