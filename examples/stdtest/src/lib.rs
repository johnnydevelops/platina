#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let v: Vec<u32> = (0..3).collect();
    println!("before");
    let i = v.len() + std::hint::black_box(2);
    println!("{}", v[i]);
}

#[unsafe(no_mangle)]
pub extern "C" fn explicit(n: u32) -> u32 {
    if n > 10 {
        panic!("n too big: {n}");
    }
    n * 2
}
