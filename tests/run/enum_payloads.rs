use alloc::vec::Vec;
use core::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnitKind { U8(u8), Eoi(u16) }
#[derive(Clone, Copy, Debug)]
struct Unit(UnitKind);

struct Classes([u8; 256]);
impl Classes {
    fn get_by_unit(&self, u: Unit) -> usize {
        match u.0 { UnitKind::U8(b) => usize::from(self.0[usize::from(b)]), UnitKind::Eoi(n) => usize::from(n) }
    }
    fn alphabet_len(&self) -> usize { usize::from(self.0[255]) + 2 }
    fn eoi(&self) -> Unit { Unit(UnitKind::Eoi(u16::try_from(self.alphabet_len()).unwrap() - 1)) }
}

struct Reps<'a> { classes: &'a Classes, cur_byte: usize, cur_class: Option<u8>, end_byte: Option<usize>, done: bool }
impl<'a> Iterator for Reps<'a> {
    type Item = Unit;
    fn next(&mut self) -> Option<Unit> {
        if self.done { return None; }
        while self.cur_byte < self.end_byte.unwrap_or(256) {
            let byte = u8::try_from(self.cur_byte).unwrap();
            let class = self.classes.0[usize::from(byte)];
            self.cur_byte += 1;
            if self.cur_class != Some(class) {
                self.cur_class = Some(class);
                return Some(Unit(UnitKind::U8(byte)));
            }
        }
        if self.end_byte.is_none() {
            self.done = true;
            return Some(self.classes.eoi());
        }
        None
    }
}

fn opt_wide(x: u32) -> Option<(u16, u8)> { if x % 3 == 0 { None } else { Some((x as u16 * 7, x as u8)) } }

pub fn run(w: &mut dyn Write) {
    let mut c = Classes([0; 256]);
    for b in b'a'..=b'z' { c.0[b as usize] = 1; }
    c.0[b'b' as usize] = 2;
    for b in (b'c' as usize)..256 { if c.0[b] == 0 { c.0[b] = 3; } }
    c.0[255] = 3;
    let reps: Vec<Unit> = Reps { classes: &c, cur_byte: 0, cur_class: None, end_byte: None, done: false }.collect();
    let _ = writeln!(w, "{:?}", reps);
    let idx: Vec<usize> = reps.iter().map(|u| c.get_by_unit(*u)).collect();
    let _ = writeln!(w, "{:?} alpha={} stride={}", idx, c.alphabet_len(), c.alphabet_len().next_power_of_two());
    let _ = writeln!(w, "{:?}", (0..7).map(opt_wide).collect::<Vec<_>>());
}
