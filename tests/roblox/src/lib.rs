use roblox::prelude::*;

#[unsafe(no_mangle)]
pub extern "C" fn run() {
    // instances, properties, parenting
    let model = Model::new();
    model.set_name("Stuff");
    let part = Part::new();
    part.set_name("Brick");
    part.set_size(Vector3::new(4.0, 1.0, 2.0));
    part.set_position(Vector3::new(1.0, 2.0, 3.0));
    part.set_color(Color3::from_rgb(255, 128, 0));
    part.set_anchored(true);
    part.set_transparency(0.25);
    part.set_material(enums::Material::Neon);
    part.set_parent(Some(&model));
    println!("name={} parent={:?}", part.name(), part.parent().map(|p| p.name()));
    println!("size={:?} pos={:?}", part.size(), part.position());
    println!("color={:?} anchored={} transparency={}", part.color(), part.anchored(), part.transparency());
    println!("material is neon: {}", part.material() == enums::Material::Neon);

    // CFrame round trip and Rust-side math
    let cf = CFrame::new(5.0, 6.0, 7.0) * CFrame::angles(0.0, core::f64::consts::FRAC_PI_2, 0.0);
    part.set_cframe(cf);
    let back = part.cframe();
    println!("cframe pos={:?} look={:.3?}", back.position(), back.look_vector());

    // hierarchy queries and casts
    for i in 0..3 {
        let p = Part::new();
        p.set_name(&format!("P{i}"));
        p.set_parent(Some(&model));
    }
    let kids = model.get_children();
    println!("children: {:?}", kids.iter().map(|c| c.name()).collect::<Vec<_>>());
    let found = model.find_first_child("P1", false).expect("P1");
    println!("found {} is BasePart: {} is Model: {}", found.name(), found.is_a("BasePart"), found.cast::<Model>().is_some());
    let bp: BasePart = found.cast().unwrap();
    println!("cast size {:?}", bp.size());
    println!("class={} full={}", part.class_name(), part.get_full_name());

    // GUI types
    let frame = Frame::new();
    frame.set_size(UDim2::new(0.5, 10.0, 0.25, -4.0));
    frame.set_background_color3(Color3::new(0.1, 0.2, 0.3));
    println!("udim2={:?}", frame.size());

    // dynamic Value access
    let v = part.into_value();
    println!("dynamic Name={} typeof={}", v.get("Name").to_string_luau(), v.type_name());
    v.set("Name", "Renamed");
    println!("renamed: {}", part.name());
    let t = Value::table();
    t.set_index(1, 10.0);
    t.set_index(2, "x");
    println!("table len={}", t.len());
    let buf = Value::buffer_from(&[1, 2, 3, 250]);
    println!("buffer bytes={:?}", buf.to_bytes());

    // destroy
    part.destroy();
    println!("after destroy parent={:?}", part.parent().is_none());
}

/// Interleaved yielding Rust tasks: each keeps locals in shadow-stack memory (an array it
/// borrows across `task::wait`), so a shared stack would corrupt them.
#[unsafe(no_mangle)]
pub extern "C" fn tasks() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let log = Rc::new(RefCell::new(Vec::new()));
    for id in 0..3u32 {
        let log = log.clone();
        task::spawn(move || {
            let mut frame = [id * 100; 64]; // lives in the shadow stack (address taken below)
            for step in 0..3 {
                let p = &mut frame;
                p[step] += step as u32 + 1;
                task::wait(0.01 * (3 - id) as f64);
                let sum: u32 = p.iter().sum();
                log.borrow_mut().push(format!("task {id} step {step} sum {sum}"));
            }
        });
    }
    let l2 = log.clone();
    task::delay(0.05, move || l2.borrow_mut().push("delayed".to_string()));
    task::wait(0.3);
    let mut l = log.borrow().clone();
    l.sort();
    for line in l {
        println!("{line}");
    }
}
