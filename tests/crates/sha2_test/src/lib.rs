use sha2::{Digest, Sha256, Sha512};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn run() {
    println!("{}", hex(&Sha256::digest(b"")));
    println!("{}", hex(&Sha256::digest(b"hello roblox")));
    let mut h = Sha512::new();
    for i in 0..100u32 {
        h.update(i.to_le_bytes());
    }
    println!("{}", hex(&h.finalize()));
}
