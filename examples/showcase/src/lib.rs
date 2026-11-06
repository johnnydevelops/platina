//! A small Roblox experience written entirely in Rust: a server Script calls
//! `require(Showcase).main()`. It builds a rainbow tower, a spinning platform driven by
//! Heartbeat, touch pads that count and recolor, and a GUI for every player.

use std::cell::Cell;
use std::rc::Rc;

use roblox::prelude::*;

fn part(name: &str, size: Vector3, cf: CFrame, color: Color3, parent: &Instance) -> Part {
    let p = Part::new();
    p.set_name(name);
    p.set_size(size);
    p.set_cframe(cf);
    p.set_color(color);
    p.set_anchored(true);
    p.set_top_surface(enums::SurfaceType::Smooth);
    p.set_bottom_surface(enums::SurfaceType::Smooth);
    p.set_parent(Some(parent));
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let workspace = Workspace::get();
    let world = Model::new();
    world.set_name("RustShowcase");
    world.set_parent(Some(&workspace));
    let root = world.as_instance();

    // a rainbow tower of slowly twisting blocks
    for i in 0..24 {
        let t = i as f64 / 24.0;
        let cf = CFrame::new(-20.0, 1.0 + i as f64 * 1.2, 0.0) * CFrame::angles(0.0, t * std::f64::consts::PI, 0.0);
        let p = part(&format!("Tower{i}"), Vector3::new(6.0, 1.0, 2.0), cf, Color3::from_hsv(t, 0.8, 1.0), &root);
        p.set_material(enums::Material::Neon);
    }

    // a spinning platform, rotated every frame from Heartbeat
    let platform = part("Spinner", Vector3::new(16.0, 1.0, 16.0), CFrame::new(0.0, 0.5, 20.0), Color3::from_rgb(60, 60, 70), &root);
    let angle = Rc::new(Cell::new(0.0));
    let (spin, a) = (platform.clone(), angle.clone());
    RunService::get().heartbeat().connect(move |dt: f64| {
        a.set(a.get() + dt * 0.8);
        spin.set_cframe(CFrame::new(0.0, 0.5, 20.0) * CFrame::angles(0.0, a.get(), 0.0));
    });

    // touch pads: count touches, flash a random color
    let touches = Rc::new(Cell::new(0u32));
    for k in 0..3 {
        let pad = part(&format!("Pad{k}"), Vector3::new(8.0, 0.4, 8.0), CFrame::new(-8.0 + k as f64 * 10.0, 0.2, -12.0),
                       Color3::from_rgb(40, 120, 255), &root);
        let (p, n) = (pad.clone(), touches.clone());
        pad.touched().connect(move |other: Option<BasePart>| {
            let Some(other) = other else { return };
            if other.parent().and_then(|m| m.find_first_child_which_is_a("Humanoid", false)).is_none() {
                return; // only characters
            }
            n.set(n.get() + 1);
            p.set_color(Color3::from_hsv((n.get() as f64 * 0.137) % 1.0, 0.9, 1.0));
        });
    }

    // a GUI per player: a live counter and a button that drops a ball
    let players = Players::get();
    let setup = {
        let touches = touches.clone();
        let root = root.clone();
        move |player: Option<Player>| {
            let Some(player) = player else { return };
            let gui = ScreenGui::new();
            gui.set_reset_on_spawn(false);
            let label = TextLabel::new();
            label.set_size(UDim2::from_offset(260.0, 40.0));
            label.set_position(UDim2::new(0.5, -130.0, 0.0, 12.0));
            label.set_background_color3(Color3::from_rgb(20, 20, 24));
            label.set_background_transparency(0.3);
            label.set_text_color3(Color3::new(1.0, 1.0, 1.0));
            label.set_font(enums::Font::GothamBold);
            label.set_text_size(20.0);
            label.set_parent(Some(&gui));
            let button = TextButton::new();
            button.set_size(UDim2::from_offset(160.0, 40.0));
            button.set_position(UDim2::new(0.5, -80.0, 0.0, 60.0));
            button.set_background_color3(Color3::from_rgb(255, 120, 40));
            button.set_text("Drop a ball");
            button.set_font(enums::Font::GothamBold);
            button.set_text_size(18.0);
            button.set_parent(Some(&gui));
            UICorner::new().set_parent(Some(&button));
            gui.set_parent(Some(&player.wait_for_child("PlayerGui", 30.0).expect("PlayerGui")));

            let root = root.clone();
            button.activated().connect(move |_: Option<InputObject>, _: i32| {
                let ball = Part::new();
                ball.set_shape(enums::PartType::Ball);
                ball.set_size(Vector3::new(3.0, 3.0, 3.0));
                ball.set_position(Vector3::new(0.0, 30.0, 20.0));
                ball.set_color(Color3::from_hsv(task::clock() % 1.0, 0.8, 1.0));
                ball.set_material(enums::Material::SmoothPlastic);
                ball.set_parent(Some(&root));
                Debris::get().add_item(&ball, 15.0);
            });
            // a loop per player that keeps the label current
            let touches = touches.clone();
            task::spawn(move || {
                while label.parent().is_some() {
                    label.set_text(&format!("pad touches: {}", touches.get()));
                    task::wait(0.2);
                }
            });
        }
    };
    for p in players.get_players() {
        setup(p.cast());
    }
    players.player_added().connect(setup);
    println!("Rust showcase ready: {} parts", world.get_descendants().len());
}
