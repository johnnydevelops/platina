// usage: n64-native <rom.z64> [frames] [out dir]   (writes a PPM every 30 frames)
use std::io::Write;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("rom path");
    let frames: u32 = std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(300);
    let out_dir = std::env::args().nth(3).unwrap_or_else(|| ".".into());
    let rom = std::fs::read(&path).unwrap();
    let mut emu = n64::Emu::new(&rom).unwrap();
    let t = Instant::now();
    for f in 1..=frames {
        emu.run_frame();
        if f % 30 == 0 {
            let lit = emu.fb[..(emu.w * emu.h * 4) as usize].chunks(4).filter(|p| p[0] | p[1] | p[2] != 0).count();
            println!("frame {f}: {}x{} lit={lit} t={:.2}s", emu.w, emu.h, t.elapsed().as_secs_f64());
            // write a PPM of the current frame
            let mut out = std::fs::File::create(format!("{out_dir}/frame_{f:04}.ppm")).unwrap();
            write!(out, "P6\n{} {}\n255\n", emu.w, emu.h).unwrap();
            for p in emu.fb[..(emu.w * emu.h * 4) as usize].chunks(4) {
                out.write_all(&p[..3]).unwrap();
            }
        }
    }
}
