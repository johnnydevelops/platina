// u64 arithmetic
pub fn bench(n: u32) -> u32 {
    let mut x = 0x2545F4914F6CDD1Du64;
    let mut acc = 0u64;
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        acc = acc.wrapping_add(x);
    }
    (acc ^ (acc >> 32)) as u32
}
