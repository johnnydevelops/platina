use core::fmt::Write;

trait Animal {
    fn name(&self) -> &str;
    fn legs(&self) -> u32 { 4 }
    fn describe(&self, w: &mut dyn Write) {
        let _ = writeln!(w, "{} has {} legs", self.name(), self.legs());
    }
}
struct Dog;
struct Bird { n: &'static str }
impl Animal for Dog { fn name(&self) -> &str { "dog" } }
impl Animal for Bird { fn name(&self) -> &str { self.n } fn legs(&self) -> u32 { 2 } }

fn total<T: Animal + ?Sized>(xs: &[&T]) -> u32 { xs.iter().map(|a| a.legs()).sum() }

fn apply<F: Fn(i32) -> i32>(f: F, x: i32) -> i32 { f(f(x)) }
fn apply_dyn(f: &dyn Fn(i32) -> i32, x: i32) -> i32 { f(x) + 1 }
fn make_adder(k: i32) -> impl Fn(i32) -> i32 { move |x| x + k }

pub fn run(w: &mut dyn Write) {
    let d = Dog;
    let b = Bird { n: "parrot" };
    let zoo: [&dyn Animal; 3] = [&d, &b, &Bird { n: "owl" }];
    for a in zoo.iter() {
        a.describe(w);
    }
    let _ = writeln!(w, "total legs {}", total(&zoo));
    let k = 10;
    let _ = writeln!(w, "{} {} {}", apply(|x| x * 3 + k, 2), apply_dyn(&|x| x - k, 5), make_adder(7)(8));
    let mut counter = 0;
    let mut inc = || { counter += 2; counter };
    inc();
    inc();
    let _ = writeln!(w, "counter {}", counter);
    let fp: fn(i32) -> i32 = |x| x * x;
    let fs: [fn(i32) -> i32; 2] = [fp, i32::wrapping_neg];
    let _ = writeln!(w, "{:?}", fs.map(|f| f(9)));
}
