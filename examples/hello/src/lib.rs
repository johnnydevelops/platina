use std::collections::HashMap;

/// n-th Fibonacci number (u64 math, returned as a Luau number).
#[unsafe(no_mangle)]
pub extern "C" fn fib(n: u32) -> f64 {
    let (mut a, mut b) = (0u64, 1u64);
    for _ in 0..n {
        (a, b) = (b, a.wrapping_add(b));
    }
    a as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn greet() {
    let mut counts: HashMap<char, usize> = HashMap::new();
    for c in "hello from rust".chars().filter(|c| c.is_alphabetic()) {
        *counts.entry(c).or_default() += 1;
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("hello from Rust! top letters: {:?}", &v[..3]);
}
