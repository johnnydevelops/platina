use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

trait Shape { fn area(&self) -> f64; }
struct Sq(f64);
struct Tri(f64, f64);
impl Shape for Sq { fn area(&self) -> f64 { self.0 * self.0 } }
impl Shape for Tri { fn area(&self) -> f64 { 0.5 * self.0 * self.1 } }

pub fn run(w: &mut dyn Write) {
    let mut v: Vec<u32> = Vec::new();
    for i in 0..1000 { v.push(i * i % 97); }
    v.sort();
    v.dedup();
    let _ = writeln!(w, "{} {:?}", v.len(), &v[..10]);
    let s: String = ["alpha", "beta", "gamma"].join("-");
    let _ = writeln!(w, "{} {} {}", s, s.to_uppercase(), s.len());
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    for wd in "a rose is a rose is a rose".split_whitespace() {
        *words.entry(wd.to_string()).or_insert(0) += 1;
    }
    let _ = writeln!(w, "{:?}", words);
    let shapes: Vec<Box<dyn Shape>> = vec![Box::new(Sq(3.0)), Box::new(Tri(4.0, 5.0))];
    let total: f64 = shapes.iter().map(|s| s.area()).sum();
    let _ = writeln!(w, "{}", total);
    let shared = Rc::new(vec![1, 2, 3]);
    let other = Rc::clone(&shared);
    let _ = writeln!(w, "{} {:?}", Rc::strong_count(&shared), other);
    let msg = format!("{:>8}|{:<6}|{:^7}|{:08.3}", "right", "left", "mid", 3.14159);
    let _ = writeln!(w, "{}", msg);
    let nested: Vec<Vec<i32>> = (0..4).map(|i| (0..i).collect()).collect();
    let _ = writeln!(w, "{:?}", nested);
    let mut big = String::new();
    for i in 0..2000 { big.push(char::from(b'a' + (i % 26) as u8)); }
    let _ = writeln!(w, "{} {}", big.len(), &big[1990..]);
    let rev: String = "héllo wörld".chars().rev().collect();
    let _ = writeln!(w, "{}", rev);
}
