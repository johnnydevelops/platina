use core::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq)]
struct P { x: i32, y: i32 }

#[derive(Debug)]
enum Shape { Circle { r: u32 }, Rect(P, P), Empty }

fn area(s: &Shape) -> u32 {
    match s {
        Shape::Circle { r } => 3 * r * r,
        Shape::Rect(a, b) => ((b.x - a.x).unsigned_abs()) * ((b.y - a.y).unsigned_abs()),
        Shape::Empty => 0,
    }
}

fn find(xs: &[i32], t: i32) -> Option<usize> {
    xs.iter().position(|&x| x == t)
}

fn div(a: i32, b: i32) -> Result<i32, &'static str> {
    if b == 0 { Err("div by zero") } else { Ok(a / b) }
}

pub fn run(w: &mut dyn Write) {
    let shapes = [Shape::Circle { r: 5 }, Shape::Rect(P { x: 1, y: 2 }, P { x: 4, y: -3 }), Shape::Empty];
    for s in &shapes {
        let _ = writeln!(w, "{:?} -> {}", s, area(s));
    }
    let xs = [5, 3, 9, -1, 9];
    let _ = writeln!(w, "{:?} {:?}", find(&xs, 9), find(&xs, 7));
    let _ = writeln!(w, "{:?} {:?}", div(7, 2), div(1, 0));
    let p = P { x: 3, y: 4 };
    let q = P { y: p.x, ..p };
    let _ = writeln!(w, "{:?} {}", q, p == q);
    let t = (1u8, true, 'z', -2i16);
    let _ = writeln!(w, "{:?}", t);
    let o: Option<bool> = Some(false);
    let _ = writeln!(w, "{:?} {:?}", o, o.map(|b| !b));
}
