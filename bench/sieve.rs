// count primes below n with a byte sieve
pub fn bench(n: u32) -> u32 {
    let mut v = alloc::vec![true; n as usize];
    v[0] = false;
    v[1] = false;
    let mut i = 2usize;
    while i * i < n as usize {
        if v[i] {
            let mut j = i * i;
            while j < n as usize {
                v[j] = false;
                j += i;
            }
        }
        i += 1;
    }
    v.iter().filter(|&&b| b).count() as u32
}
