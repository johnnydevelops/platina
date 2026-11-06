// usage: chat-native <model.q8> <tok> <temperature> <message>...   (one message per turn)
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
    let mut c = llm::chat::Chat::new(model, &tok, 1024, 42);
    if a[3] == "tok" {
        for m in &a[4..] {
            println!("{:?}", c.encode(m));
        }
        return;
    }
    c.temperature = a[3].parse().unwrap();
    c.no_batch = std::env::var("NO_BATCH").is_ok();
    for msg in &a[4..] {
        println!("> {msg}");
        c.user(msg);
        let prompt = c.pending();
        let t = std::time::Instant::now();
        let mut n = 0;
        while let Some(_) = c.step() {
            std::io::stdout().write_all(&c.text).unwrap();
            std::io::stdout().flush().unwrap();
            n += 1;
        }
        eprintln!("\n[{prompt} prompt + {} reply tokens, {:.1} tok/s]", n - prompt, n as f64 / t.elapsed().as_secs_f64());
    }
}
