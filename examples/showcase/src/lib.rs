//! A small Roblox experience written entirely in Rust. A server Script calls
//! `require(Showcase).server()` and a LocalScript calls `require(Showcase).client()`:
//! - server: a rainbow tower, a platform spun from Heartbeat, touch pads that count character
//!   touches and recolor, and balls dropped on request;
//! - client: a GUI with a live touch counter and a "Drop a ball" button.
//!
//! Server and client talk through a RemoteEvent (`DropBall`) and an IntValue (`PadTouches`).

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
pub extern "C" fn server() {
    let storage = ReplicatedStorage::get();
    let drop_ball = RemoteEvent::new();
    drop_ball.set_name("DropBall");
    drop_ball.set_parent(Some(&storage));
    let touches = IntValue::new();
    touches.set_name("PadTouches");
    touches.set_parent(Some(&storage));

    let world = Model::new();
    world.set_name("RustShowcase");
    world.set_parent(Some(&Workspace::get()));
    let root = world.as_instance();

    // a rainbow tower of slowly twisting neon blocks
    for i in 0..24 {
        let t = i as f64 / 24.0;
        let cf = CFrame::new(-20.0, 1.0 + i as f64 * 1.2, 0.0) * CFrame::angles(0.0, t * std::f64::consts::PI, 0.0);
        let p = part(&format!("Tower{i}"), Vector3::new(6.0, 1.0, 2.0), cf, Color3::from_hsv(t, 0.8, 1.0), &root);
        p.set_material(enums::Material::Neon);
    }

    // a spinning platform, rotated every frame from Heartbeat
    let platform = part("Spinner", Vector3::new(16.0, 1.0, 16.0), CFrame::new(0.0, 0.5, 20.0), Color3::from_rgb(60, 60, 70), &root);
    let angle = Rc::new(Cell::new(0.0));
    RunService::get().heartbeat().connect(move |dt: f64| {
        angle.set(angle.get() + dt * 0.8);
        platform.set_cframe(CFrame::new(0.0, 0.5, 20.0) * CFrame::angles(0.0, angle.get(), 0.0));
    });

    // touch pads: count touches by characters (once per contact), flash a new color
    for k in 0..3 {
        let pad = part(&format!("Pad{k}"), Vector3::new(8.0, 0.4, 8.0), CFrame::new(-8.0 + k as f64 * 10.0, 0.2, -12.0),
                       Color3::from_rgb(40, 120, 255), &root);
        let (p, count) = (pad.clone(), touches.clone());
        let cooldown = Rc::new(Cell::new(0.0));
        pad.touched().connect(move |other: Option<BasePart>| {
            let Some(other) = other else { return };
            let character = other.parent();
            if character.and_then(|m| m.find_first_child_which_is_a("Humanoid", false)).is_none() {
                return; // only characters
            }
            // Touched fires for every limb and contact; count at most twice a second
            if task::clock() - cooldown.get() < 0.5 {
                return;
            }
            cooldown.set(task::clock());
            count.set_value(count.value() + 1);
            p.set_color(Color3::from_hsv((count.value() as f64 * 0.137) % 1.0, 0.9, 1.0));
        });
    }

    // balls on request from any client (rate-limited per player)
    let last_drop = Rc::new(Cell::new(0.0));
    drop_ball.on_server_event().connect(move |_player: Option<Player>, _: Varargs| {
        if task::clock() - last_drop.get() < 0.2 {
            return;
        }
        last_drop.set(task::clock());
        let ball = Part::new();
        ball.set_shape(enums::PartType::Ball);
        ball.set_size(Vector3::new(3.0, 3.0, 3.0));
        ball.set_position(Vector3::new(0.0, 30.0, 20.0));
        ball.set_color(Color3::from_hsv(task::clock() % 1.0, 0.8, 1.0));
        ball.set_material(enums::Material::SmoothPlastic);
        ball.set_parent(Some(&root));
        Debris::get().add_item(&ball, 15.0);
    });
    println!("Rust showcase ready: {} instances", world.get_descendants().len());
}

#[unsafe(no_mangle)]
pub extern "C" fn client() {
    let player = Players::get().local_player().expect("client() must run in a LocalScript");
    let storage = ReplicatedStorage::get();
    let drop_ball: RemoteEvent = storage.wait_for_child("DropBall", 60.0).and_then(|r| r.cast()).expect("DropBall");
    let touches: IntValue = storage.wait_for_child("PadTouches", 60.0).and_then(|r| r.cast()).expect("PadTouches");

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
    gui.set_parent(Some(&player.wait_for_child("PlayerGui", 60.0).expect("PlayerGui")));

    let show = move |n: i64| label.set_text(&format!("pad touches: {n}"));
    show(touches.value());
    touches.changed().connect(show);
    button.activated().connect(move |_: Option<InputObject>, _: i32| drop_ball.fire_server(&[]));
}
