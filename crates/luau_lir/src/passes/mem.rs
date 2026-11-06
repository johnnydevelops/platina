//! Memory clean-up: store-to-load forwarding, dead stores to non-escaping frames, and
//! removal of empty shadow-stack frames left behind by inlining.

use std::collections::HashMap;

use crate::ir::*;

pub fn run(f: &mut Function) -> bool {
    let mut changed = forward(f);
    changed |= dead_frames(f);
    changed
}

/// Within a block: `store [b+o] = x; ...; y = load [b+o]` -> `y = x` when nothing in between
/// can write memory at an unknown address.
fn forward(f: &mut Function) -> bool {
    let mut changed = false;
    for b in &mut f.blocks {
        // (base var, offset) -> (mem kind, value)
        let mut known: HashMap<(VarId, i32), (Mem, Operand)> = HashMap::new();
        for s in &mut b.stmts {
            match s {
                Stmt::Assign(v, rv) => {
                    let mut load_fact = None;
                    if let Rv::Load { mem, addr: Operand::Var(base), off } = rv {
                        match known.get(&(*base, *off)) {
                            Some((m, val)) if m == mem && *val != Operand::Var(*v) => {
                                *rv = Rv::Use(*val);
                                changed = true;
                            }
                            _ => load_fact = Some(((*base, *off), *mem)),
                        }
                    }
                    // redefining a var invalidates facts mentioning it
                    let v = *v;
                    known.retain(|(base, _), (_, val)| *base != v && *val != Operand::Var(v));
                    // later loads of the same location can reuse this one
                    if let Some((key, mem)) = load_fact {
                        if key.0 != v {
                            known.insert(key, (mem, Operand::Var(v)));
                        }
                    }
                }
                Stmt::Store { mem, addr: Operand::Var(base), off, val } => {
                    let (base, off, mem, val) = (*base, *off, *mem, *val);
                    // a store may alias anything not provably disjoint on the same base
                    let size = mem.size() as i32;
                    known.retain(|(b2, o2), (m2, _)| {
                        *b2 == base && (*o2 + m2.size() as i32 <= off || off + size <= *o2)
                    });
                    if !matches!(mem, Mem::U8 | Mem::I8 | Mem::U16 | Mem::I16 | Mem::F32) {
                        known.insert((base, off), (mem, val));
                    }
                }
                Stmt::GetSp(v) => {
                    let v = *v;
                    known.retain(|(base, _), (_, val)| *base != v && *val != Operand::Var(v));
                }
                _ => {
                    for d in s.def() {
                        known.retain(|(base, _), (_, val)| *base != d && *val != Operand::Var(d));
                    }
                    if !matches!(s, Stmt::SetSp(_)) {
                        known.clear();
                    }
                }
            }
        }
    }
    changed
}

/// A frame pointer `fp = sp0 - k` whose address is only used as a load/store base with
/// constant offsets: if nothing loads from it, its stores are dead; if nothing uses it at all,
/// the frame push/pop is dead.
fn dead_frames(f: &mut Function) -> bool {
    let n = f.vars.len();
    let mut sp_vars = vec![false; n];
    let mut defs = vec![0u32; n];
    for p in &f.params {
        defs[*p as usize] += 1;
    }
    for b in &f.blocks {
        for s in &b.stmts {
            for d in s.def() {
                defs[d as usize] += 1;
            }
            if let Stmt::GetSp(v) = s {
                sp_vars[*v as usize] = true;
            }
        }
    }
    // frame vars: single def `fp = sp - k` (or `fp = sp`)
    let mut frame = vec![false; n];
    for b in &f.blocks {
        for s in &b.stmts {
            if let Stmt::Assign(v, rv) = s {
                let from_sp = match rv {
                    Rv::Bin(Bin::Sub, Operand::Var(s0), Operand::Num(_)) | Rv::Use(Operand::Var(s0)) => sp_vars[*s0 as usize],
                    _ => false,
                };
                if from_sp && defs[*v as usize] == 1 {
                    frame[*v as usize] = true;
                }
            }
        }
    }
    // classify uses of frame vars
    let mut escapes = vec![false; n];
    let mut loaded = vec![false; n];
    let mut other_use = vec![false; n]; // any use except SetSp(fp)
    let mut mark_escape = |o: &Operand, escapes: &mut Vec<bool>| {
        if let Operand::Var(v) = o {
            escapes[*v as usize] = true;
        }
    };
    for b in &f.blocks {
        for s in &b.stmts {
            match s {
                Stmt::Assign(_, Rv::Load { addr: Operand::Var(base), .. }) => {
                    loaded[*base as usize] = true;
                    other_use[*base as usize] = true;
                }
                Stmt::Store { addr, val, .. } => {
                    if let Operand::Var(base) = addr {
                        other_use[*base as usize] = true;
                    }
                    mark_escape(val, &mut escapes);
                }
                Stmt::SetSp(_) => {}
                _ => {
                    for o in s.operands() {
                        mark_escape(o, &mut escapes);
                        if let Operand::Var(v) = o {
                            other_use[*v as usize] = true;
                        }
                    }
                }
            }
        }
        let mut t = b.term.clone();
        for o in t.operands_mut() {
            mark_escape(o, &mut escapes);
            if let Operand::Var(v) = o {
                other_use[*v as usize] = true;
            }
        }
    }
    let mut changed = false;
    // dead stores into never-loaded, non-escaping frames
    for b in &mut f.blocks {
        let before = b.stmts.len();
        b.stmts.retain(|s| match s {
            Stmt::Store { addr: Operand::Var(base), .. } => {
                let bi = *base as usize;
                !(frame[bi] && !escapes[bi] && !loaded[bi])
            }
            _ => true,
        });
        changed |= b.stmts.len() != before;
    }
    if changed {
        return true;
    }
    // frames with no uses at all: drop `SP = fp`, and then `sp0` pairs that only restore
    let mut drop_set = vec![false; n];
    for v in 0..n {
        if frame[v] && !other_use[v] && !escapes[v] {
            drop_set[v] = true;
        }
    }
    for b in &mut f.blocks {
        let before = b.stmts.len();
        b.stmts.retain(|s| !matches!(s, Stmt::SetSp(Operand::Var(v)) if drop_set[*v as usize]));
        changed |= b.stmts.len() != before;
    }
    if changed {
        return true;
    }
    // sp0 vars used only by `SP = sp0` (and dead frame defs): the frame is balanced, drop them
    let mut sp_only = vec![true; n];
    for b in &f.blocks {
        for s in &b.stmts {
            match s {
                Stmt::SetSp(_) => {}
                _ => {
                    for o in s.operands() {
                        if let Operand::Var(v) = o {
                            sp_only[*v as usize] = false;
                        }
                    }
                }
            }
        }
        let mut t = b.term.clone();
        for o in t.operands_mut() {
            if let Operand::Var(v) = o {
                sp_only[*v as usize] = false;
            }
        }
    }
    // only when no SetSp(non-sp0) remains for this frame: i.e. no SetSp(fp) of a live frame
    // derived from it. Check: every SetSp in the function is of an sp var or of a live frame.
    let mut has_push_from: Vec<bool> = vec![false; n];
    for b in &f.blocks {
        for s in &b.stmts {
            if let Stmt::Assign(fp, Rv::Bin(Bin::Sub, Operand::Var(s0), _) | Rv::Use(Operand::Var(s0))) = s {
                if frame[*fp as usize] {
                    // is that frame still pushed?
                    let pushed = f.blocks.iter().any(|bb| bb.stmts.iter().any(|x| matches!(x, Stmt::SetSp(Operand::Var(v)) if v == fp)));
                    if pushed {
                        has_push_from[*s0 as usize] = true;
                    }
                }
            }
        }
    }
    for b in &mut f.blocks {
        let before = b.stmts.len();
        b.stmts.retain(|s| match s {
            Stmt::SetSp(Operand::Var(v)) => {
                let vi = *v as usize;
                !(sp_vars[vi] && sp_only[vi] && !has_push_from[vi])
            }
            _ => true,
        });
        changed |= b.stmts.len() != before;
    }
    changed
}
