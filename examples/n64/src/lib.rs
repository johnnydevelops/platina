//! A tiny embedding of the RustyN64 core for Luau: load a ROM, run frames, read pixels.
use rustyn64_core::System;

#[cfg(target_vendor = "roblox")]
mod roblox_main;

pub const FB_W: usize = 640;
pub const FB_H: usize = 625;

pub struct Emu {
    pub system: Box<System>,
    pub fb: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

impl Emu {
    pub fn new(rom: &[u8]) -> Result<Emu, String> {
        let mut system = Box::new(System::new(0));
        system.reset();
        rustyn64_core::boot::hle_boot(&mut system, rom).map_err(|e| format!("{e:?}"))?;
        Ok(Emu { system, fb: vec![0; FB_W * FB_H * 4], w: 0, h: 0 })
    }

    pub fn run_frame(&mut self) {
        let target = self.system.master_ticks().saturating_add(rustyn64_core::MASTER_HZ / 60);
        let _ = self.system.run_until_exec(target);
        let (w, h) = self.system.bus.scanout_scaled(&mut self.fb);
        self.w = w;
        self.h = h;
        // opaque alpha for direct upload (e.g. Roblox EditableImage)
        for px in self.fb[..(w * h * 4) as usize].chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
}

// ---- Luau exports -------------------------------------------------------------------------

static mut EMU: Option<Emu> = None;

#[allow(static_mut_refs)]
fn emu() -> &'static mut Emu {
    unsafe { EMU.as_mut().expect("no ROM loaded") }
}

/// Allocate `len` bytes (for the caller to copy the ROM into); returns the address.
#[unsafe(no_mangle)]
pub extern "C" fn n64_alloc(len: u32) -> u32 {
    let v = vec![0u8; len as usize].leak();
    v.as_mut_ptr() as usize as u32
}

/// Boot the ROM at `ptr..ptr+len`. Returns 0 on success.
#[unsafe(no_mangle)]
#[allow(static_mut_refs)]
pub extern "C" fn n64_load(ptr: u32, len: u32) -> u32 {
    let rom = unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, len as usize) };
    match Emu::new(rom) {
        Ok(e) => {
            unsafe { EMU = Some(e) };
            0
        }
        Err(e) => {
            println!("boot failed: {e}");
            1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn n64_run_frame() {
    emu().run_frame();
}

/// Address of the RGBA8 framebuffer of the last frame (`n64_w` x `n64_h`).
#[unsafe(no_mangle)]
pub extern "C" fn n64_fb() -> u32 {
    emu().fb.as_ptr() as usize as u32
}
#[unsafe(no_mangle)]
pub extern "C" fn n64_w() -> u32 {
    emu().w
}
#[unsafe(no_mangle)]
pub extern "C" fn n64_h() -> u32 {
    emu().h
}
/// Instructions retired by the CPU so far (progress indicator).
#[unsafe(no_mangle)]
pub extern "C" fn n64_retired() -> f64 {
    emu().system.cpu.retired as f64
}

/// Controller 1 state: `buttons << 16 | (stick_x as u8) << 8 | (stick_y as u8)`.
/// Buttons: A=1<<15 B=1<<14 Z=1<<13 Start=1<<12 D-pad U/D/L/R=1<<11..8 L=1<<5 R=1<<4
/// C U/D/L/R=1<<3..0.
#[unsafe(no_mangle)]
pub extern "C" fn n64_set_pad(word: u32) {
    emu().system.bus.controllers[0] = word;
}
