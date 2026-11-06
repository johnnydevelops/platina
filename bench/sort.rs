// Vec<u32> sort of pseudo-random data
pub fn bench(n: u32) -> u32 {
    let mut v: alloc::vec::Vec<u32> = (0..n).map(|i| i.wrapping_mul(2654435761) ^ (i >> 3)).collect();
    v.sort_unstable();
    v[(n / 2) as usize]
}
