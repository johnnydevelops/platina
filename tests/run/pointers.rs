use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt::Write;

enum List { Cons(i32, Box<List>), Nil }

fn sum(l: &List) -> i32 {
    match l { List::Cons(v, rest) => v + sum(rest), List::Nil => 0 }
}

#[derive(Debug, Default, Clone)]
struct Acc { total: u64, count: u32, name: String }

fn bump(a: &mut Acc, by: u64) { a.total += by; a.count += 1; }

pub fn run(w: &mut dyn Write) {
    let mut l = List::Nil;
    for i in 1..=10 { l = List::Cons(i * i, Box::new(l)); }
    let _ = writeln!(w, "list sum {}", sum(&l));

    let shared = Rc::new(RefCell::new(Vec::<u32>::new()));
    let s2 = Rc::clone(&shared);
    let push = move |x| s2.borrow_mut().push(x);
    for i in 0..5 { push(i * 3); }
    let _ = writeln!(w, "{:?} rc={}", shared.borrow(), Rc::strong_count(&shared));

    let mut a = Acc { name: String::from("a"), ..Default::default() };
    let mut b = Acc { name: String::from("b"), ..Default::default() };
    for i in 0..7 { bump(if i % 3 == 0 { &mut a } else { &mut b }, i); }
    core::mem::swap(&mut a, &mut b);
    let _ = writeln!(w, "{:?} {:?}", a, b);

    let mut grid = [[0i32; 4]; 4];
    for (r, row) in grid.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() { *cell = (r * 4 + c) as i32; }
    }
    let diag: i32 = (0..4).map(|i| grid[i][i]).sum();
    let mut flat: Vec<i32> = grid.iter().flatten().copied().collect();
    flat.reverse();
    let _ = writeln!(w, "diag {} {:?}", diag, &flat[..5]);

    let mut counter = 0u32;
    let mut incr = |k: u32| { counter += k; counter };
    let r = [incr(1), incr(2), incr(3)];
    let _ = writeln!(w, "{:?} {}", r, counter);

    let mut v: Vec<(u8, &str)> = alloc::vec![(3, "c"), (1, "a"), (2, "b"), (1, "z")];
    v.sort_by(|x, y| x.0.cmp(&y.0).then(y.1.cmp(x.1)));
    let (mut lo, mut hi) = (0usize, v.len() - 1);
    while lo < hi { v.swap(lo, hi); lo += 1; hi -= 1; }
    let _ = writeln!(w, "{:?}", v);

    let mut buf = [0u8; 16];
    for (i, chunk) in buf.chunks_mut(4).enumerate() { chunk.fill(i as u8 + 1); }
    let s: u32 = buf.iter().map(|&b| b as u32).sum();
    let _ = writeln!(w, "{:?} {}", &buf[2..6], s);
}
