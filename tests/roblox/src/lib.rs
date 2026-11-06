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

/// Signals with Rust closures, against a Luau signal object (`TestSignal` in the runner).
#[unsafe(no_mangle)]
pub extern "C" fn signals() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let sig_v = Value::global("TestSignal");
    let sig: Signal<(f64, String, Option<Part>)> = Signal::from_value(sig_v.clone()).expect("TestSignal");
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let l = log.clone();
    let conn = sig.connect(move |n: f64, s: String, p: Option<Part>| {
        l.borrow_mut().push(format!("got {n} {s} {:?}", p.map(|p| p.name())));
    });
    let part = Part::new();
    part.set_name("Hit");
    sig_v.call_method("Fire", &[1.5.into_value(), "a".into_value(), part.into_value()]);
    sig_v.call_method("Fire", &[2.0.into_value(), "b".into_value(), Value::NIL]);
    println!("connected: {}", conn.is_connected());
    conn.disconnect();
    sig_v.call_method("Fire", &[3.0.into_value(), "c".into_value(), Value::NIL]);
    for line in log.borrow().iter() {
        println!("{line}");
    }
}

/// The showcase's touch-pad handler, fired repeatedly by a character's part.
#[unsafe(no_mangle)]
pub extern "C" fn touch() {
    use std::cell::Cell;
    use std::rc::Rc;
    let pad = Part::new();
    pad.set_name("Pad");
    let character = Model::new();
    Humanoid::new().set_parent(Some(&character));
    let leg = Part::new();
    leg.set_name("LeftLeg");
    leg.set_parent(Some(&character));
    let sig_v = Value::global("TestSignal");
    let touched: Signal<(Option<BasePart>,)> = Signal::from_value(sig_v.clone()).unwrap();
    let n = Rc::new(Cell::new(0u32));
    let (p, c) = (pad.clone(), n.clone());
    touched.connect(move |other: Option<BasePart>| {
        let Some(other) = other else { return };
        if other.parent().and_then(|m| m.find_first_child_which_is_a("Humanoid", false)).is_none() {
            return;
        }
        c.set(c.get() + 1);
        p.set_color(Color3::from_hsv((c.get() as f64 * 0.137) % 1.0, 0.9, 1.0));
    });
    for _ in 0..3 {
        sig_v.call_method("Fire", &[leg.into_value()]);
    }
    println!("touches {} color {:?}", n.get(), pad.color());
}

thread_local! {
    static STOP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static STRESS: std::cell::RefCell<(u32, u32, u32, Vec<String>)> = Default::default();
}

/// Events firing between the yields of running Rust tasks (see the runner's `stress` mode).
#[unsafe(no_mangle)]
pub extern "C" fn stress_setup() {
    let sig_v = Value::global("TestSignal");
    let sig: Signal<(f64, Option<BasePart>)> = Signal::from_value(sig_v).unwrap();
    let pad = Part::new();
    sig.connect(move |i: f64, other: Option<BasePart>| {
        let mut buf = [0u8; 256]; // stack data in the handler's frame
        buf[(i as usize) % 256] = 1;
        pad.set_color(Color3::from_hsv((i * 0.137) % 1.0, 0.9, 1.0));
        let ok = other.is_some() && buf.iter().map(|&b| b as u32).sum::<u32>() == 1;
        STRESS.with(|s| {
            let mut s = s.borrow_mut();
            s.0 += 1;
            if !ok {
                s.3.push(format!("bad event {i}"));
            }
        });
    });
    for k in 0..2u32 {
        task::spawn(move || {
            let mut frame = [k; 128];
            while !STOP.with(|s| s.get()) {
                let before: u32 = frame.iter().sum();
                task::wait(0.0);
                frame[0] = frame[0].wrapping_add(0);
                let after: u32 = frame.iter().sum();
                STRESS.with(|s| {
                    let mut s = s.borrow_mut();
                    if k == 0 { s.1 += 1 } else { s.2 += 1 }
                    if before != after {
                        s.3.push(format!("task {k} frame corrupted"));
                    }
                });
            }
        });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn stress_report() {
    STOP.with(|s| s.set(true));
    STRESS.with(|s| {
        let s = s.borrow();
        println!("events {} task0 resumes>50 {} task1 resumes>50 {} errors {:?}", s.0, s.1 > 50, s.2 > 50, s.3);
    });
}
