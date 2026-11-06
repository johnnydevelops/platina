// usage: llm-native <model.bin> <tokenizer.bin> [prompt] [temperature] [seed]
use std::io::Write;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let model: &'static [u8] = {
        let b = std::fs::read(&a[1]).unwrap();
        let v = vec![0u32; b.len().div_ceil(4)].leak();
        let bytes = unsafe { core::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, b.len()) };
        bytes.copy_from_slice(&b);
        bytes
    };
    let tok = std::fs::read(&a[2]).unwrap();
    let mut m = llm::Model::new(model, &tok, a.get(5).map_or(42, |s| s.parse().unwrap()));
    m.temperature = a.get(4).map_or(0.0, |s| s.parse().unwrap());
    m.start(a.get(3).map_or("", |s| s.as_str()));
    let t = std::time::Instant::now();
    let mut n = 0;
    while m.step() {
        std::io::stdout().write_all(&m.text).unwrap();
        std::io::stdout().flush().unwrap();
        n += 1;
    }
    let dt = t.elapsed().as_secs_f64();
    eprintln!("\n[{n} tokens, {:.1} tok/s]", n as f64 / dt);
}
