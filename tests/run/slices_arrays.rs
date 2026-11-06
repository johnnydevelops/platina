use core::fmt::Write;

fn bubble(a: &mut [i32]) {
    let n = a.len();
    for i in 0..n {
        for j in 0..n - 1 - i {
            if a[j] > a[j + 1] {
                a.swap(j, j + 1);
            }
        }
    }
}

fn sieve(out: &mut [bool; 100]) {
    out.fill(true);
    out[0] = false;
    out[1] = false;
    let mut i = 2;
    while i * i < 100 {
        if out[i] {
            let mut j = i * i;
            while j < 100 {
                out[j] = false;
                j += i;
            }
        }
        i += 1;
    }
}

pub fn run(w: &mut dyn Write) {
    let mut a = [5, -2, 9, 0, 3, 3, 7, -8];
    bubble(&mut a);
    let _ = writeln!(w, "{:?}", a);
    let mut s = [false; 100];
    sieve(&mut s);
    let primes = s.iter().enumerate().filter(|(_, p)| **p).map(|(i, _)| i).fold(0, |acc, x| acc + x);
    let _ = writeln!(w, "sum of primes < 100 = {}", primes);
    let words = "the quick brown fox jumps over the lazy dog";
    let mut longest = "";
    for wd in words.split(' ') {
        if wd.len() > longest.len() {
            longest = wd;
        }
    }
    let _ = writeln!(w, "{} {}", longest, words.split(' ').count());
    let m = [[1u16, 2, 3], [4, 5, 6]];
    let mut t = [[0u16; 2]; 3];
    for i in 0..2 {
        for j in 0..3 {
            t[j][i] = m[i][j];
        }
    }
    let _ = writeln!(w, "{:?}", t);
    let v = &a[2..6];
    let _ = writeln!(w, "{:?} {:?} {:?}", v, v.first(), v.iter().rev().max());
    let bytes = b"Hello";
    let up: [u8; 5] = core::array::from_fn(|i| bytes[i].to_ascii_uppercase());
    let _ = writeln!(w, "{}", core::str::from_utf8(&up).unwrap());
}
