//! The whole in-game client, in Rust: a LocalScript only calls `require(N64).main()`.
//!
//! Reads the ROM from `ReplicatedStorage.Rom` (base64 ModuleScripts, see make_rom_modules.py),
//! builds the UI, and runs the emulator, drawing each frame into an EditableImage.

use roblox::prelude::*;

use crate::Emu;

/// Keyboard -> N64 controller word (`buttons << 16 | stick_x << 8 | stick_y`).
fn pad(input: &UserInputService) -> u32 {
    use enums::KeyCode as K;
    const BUTTONS: [(K, u32); 9] = [
        (K::Space, 0x8000),     // A
        (K::LeftShift, 0x4000), // B
        (K::Q, 0x2000),         // Z
        (K::Return, 0x1000),    // Start
        (K::E, 0x0010),         // R
        (K::Up, 0x0008),        // C buttons
        (K::Down, 0x0004),
        (K::Left, 0x0002),
        (K::Right, 0x0001),
    ];
    let mut buttons = 0;
    for (key, bit) in BUTTONS {
        if input.is_key_down(key) {
            buttons |= bit;
        }
    }
    let axis = |neg: K, pos: K| (input.is_key_down(pos) as i32 - input.is_key_down(neg) as i32) * 80;
    let (x, y) = (axis(K::A, K::D), axis(K::S, K::W));
    (buttons << 16) | ((x as u8 as u32) << 8) | (y as u8 as u32)
}

/// Concatenate the ROM's base64 chunk modules (Part01, Part02, ...) and decode them.
fn load_rom(status: &TextLabel) -> Vec<u8> {
    let folder = ReplicatedStorage::get().wait_for_child("Rom", 60.0).expect("ReplicatedStorage.Rom is missing");
    let mut parts = folder.get_children();
    parts.sort_by_key(|p| p.name());
    let encoding = EncodingService::get();
    let fromstring = Value::global("buffer").get("fromstring");
    let mut rom = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let b64 = Value::require(part);
        let decoded = encoding.base64_decode(fromstring.call(&[b64]).index(1));
        rom.extend_from_slice(&decoded.to_bytes());
        status.set_text(&format!("loading ROM... {}/{}", i + 1, parts.len()));
        task::wait(0.0);
    }
    rom
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    // ---- UI ----
    let player = Players::get().local_player().expect("main() must run in a LocalScript");
    let player_gui = player.wait_for_child("PlayerGui", 60.0).expect("PlayerGui");
    let gui = ScreenGui::new();
    gui.set_name("N64");
    gui.set_ignore_gui_inset(true);
    gui.set_reset_on_spawn(false);
    let bg = Frame::new();
    bg.set_size(UDim2::from_scale(1.0, 1.0));
    bg.set_background_color3(Color3::new(0.0, 0.0, 0.0));
    bg.set_parent(Some(&gui));
    let screen = ImageLabel::new();
    screen.set_anchor_point(Vector2::new(0.5, 0.5));
    screen.set_position(UDim2::from_scale(0.5, 0.5));
    screen.set_size(UDim2::from_scale(0.9, 0.9));
    screen.set_background_transparency(1.0);
    screen.set_resample_mode(enums::ResamplerMode::Pixelated);
    screen.set_parent(Some(&bg));
    let aspect = UIAspectRatioConstraint::new();
    aspect.set_aspect_ratio(4.0 / 3.0);
    aspect.set_parent(Some(&screen));
    let status = TextLabel::new();
    status.set_size(UDim2::new(1.0, 0.0, 0.0, 24.0));
    status.set_background_transparency(1.0);
    status.set_text_color3(Color3::new(1.0, 1.0, 1.0));
    status.set_font(enums::Font::Code);
    status.set_text_size(16.0);
    status.set_parent(Some(&bg));
    gui.set_parent(Some(&player_gui));

    // let the first-load hitch settle
    for i in (1..=10).rev() {
        status.set_text(&format!("starting in {i}..."));
        task::wait(1.0);
    }

    // ---- emulator ----
    let rom = load_rom(&status);
    let mut emu = match Emu::new(&rom) {
        Ok(e) => e,
        Err(e) => {
            status.set_text(&format!("boot failed: {e}"));
            return;
        }
    };
    drop(rom);

    let input = UserInputService::get();
    let assets = AssetService::get();
    let content = Value::global("Content").get("fromObject");
    let mut image: Option<EditableImage> = None;
    let (mut img_w, mut img_h) = (0, 0);
    let (start, mut frames) = (task::clock(), 0u32);
    loop {
        emu.system.bus.controllers[0] = pad(&input);
        let t = task::clock();
        emu.run_frame();
        let dt = task::clock() - t;
        frames += 1;
        let (w, h) = (emu.w, emu.h);
        if w > 0 && h > 0 {
            if (w, h) != (img_w, img_h) {
                let opts = Value::table();
                opts.set("Size", Vector2::new(w as f64, h as f64));
                let img = assets.create_editable_image(opts).expect("CreateEditableImage failed");
                screen.set_image_content(content.call(&[img.into_value()]).index(1));
                image = Some(img);
                (img_w, img_h) = (w, h);
            }
            if let Some(img) = &image {
                let pixels = Value::buffer_from(&emu.fb[..(w * h * 4) as usize]);
                img.write_pixels_buffer(Vector2::ZERO, Vector2::new(w as f64, h as f64), pixels);
            }
        }
        status.set_text(&format!(
            "frame {frames}   {dt:.2}s/frame   {:.2} fps avg",
            frames as f64 / (task::clock() - start)
        ));
        task::wait(0.0);
    }
}
