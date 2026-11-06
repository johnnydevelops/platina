//! Dominating-condition propagation: a branch on a condition already decided by a dominating
//! branch becomes a jump (removes repeated bounds checks).

use crate::ir::*;
use crate::structure::Cfg;

pub fn run(f: &mut Function) -> bool {
    let n = f.vars.len();
    let mut defs = vec![0u32; n];
    for p in &f.params {
        defs[*p as usize] += 1;
    }
    for b in &f.blocks {
        for s in &b.stmts {
            for d in s.def() {
                defs[d as usize] += 1;
            }
        }
    }
    let cfg = Cfg::new(f);
    // dominator-tree children
    let nb = f.blocks.len();
    let mut kids = vec![Vec::new(); nb];
    for &b in cfg.rpo.iter().skip(1) {
        kids[cfg.idom[b as usize] as usize].push(b);
    }
    // facts: (cond var, value) known on entry to block
    let mut facts: Vec<Vec<(VarId, bool)>> = vec![vec![]; nb];
    let mut changed = false;
    // walk dominator tree in RPO, inheriting facts from idom (and adding edge facts)
    for &b in &cfg.rpo {
        let bi = b as usize;
        let mut known = if b == f.entry { vec![] } else { facts[cfg.idom[bi] as usize].clone() };
        // edge fact from a single predecessor's branch
        if cfg.preds[bi].len() == 1 {
            let p = cfg.preds[bi][0] as usize;
            if let Term::Br(Operand::Var(c), t, e) = f.blocks[p].term {
                if defs[c as usize] == 1 && t != e {
                    known.push((c, t == b));
                }
            }
        }
        // resolve this block's branch
        if let Term::Br(Operand::Var(c), t, e) = f.blocks[bi].term {
            if let Some((_, v)) = known.iter().find(|(k, _)| *k == c) {
                f.blocks[bi].term = Term::Goto(if *v { t } else { e });
                changed = true;
            }
        }
        facts[bi] = known;
    }
    let _ = kids;
    changed
}

/// Abstract value for jump threading.
#[derive(Clone, Copy, PartialEq)]
enum Av {
    C(f64),
    B(bool),
    /// non-zero number
    Nz,
    U,
}

/// Jump threading: a predecessor `P -> B` whose own assignments decide `B`'s branch (e.g.
/// `v = 0` or `v = <non-null pointer>` before `B: c = v == 0; br c`) gets `B`'s (pure, short)
/// statements copied and jumps straight to the decided target. Removes `Option` niche re-tests
/// after inlined iterators.
pub fn thread(f: &mut Function) -> bool {
    let cfg = Cfg::new(f);
    let nb = f.blocks.len();
    let range_av = |f: &Function, v: VarId| match f.vars[v as usize].range {
        Some((lo, hi)) if lo > 0.0 || hi < 0.0 => Av::Nz,
        _ => Av::U,
    };
    let mut touched = vec![false; nb];
    let mut changed = false;
    for bi in 0..nb {
        let b = bi as BlockId;
        if cfg.rpo_index[bi] == usize::MAX || touched[bi] || cfg.preds[bi].len() < 2 {
            continue;
        }
        let blk = &f.blocks[bi];
        if blk.stmts.len() > 6
            || !blk.stmts.iter().all(|s| matches!(s, Stmt::Assign(_, Rv::Use(_) | Rv::Bin(..) | Rv::Select(..) | Rv::Un(Un::Not, _))))
            || !matches!(blk.term, Term::Br(..) | Term::Switch(..))
        {
            continue;
        }
        // loop headers: threading the entry edge into the body would make the CFG irreducible
        if cfg.preds[bi].iter().any(|&q| cfg.dominates(b, q)) {
            continue;
        }
        for &p in &cfg.preds[bi] {
            let pi = p as usize;
            if p == b || touched[pi] || f.blocks[pi].term != Term::Goto(b) {
                continue;
            }
            // value of a var at the end of P
            let at_end = |f: &Function, v: VarId| -> Av {
                let r = range_av(f, v);
                if r != Av::U {
                    return r;
                }
                for s in f.blocks[pi].stmts.iter().rev() {
                    if s.def().contains(&v) {
                        return match s {
                            Stmt::Assign(_, Rv::Use(Operand::Num(k))) => Av::C(*k),
                            Stmt::Assign(_, Rv::Use(Operand::Bool(k))) => Av::B(*k),
                            Stmt::Assign(_, Rv::Use(Operand::Var(w))) => {
                                // w must not be redefined between this copy and the end of P
                                range_av(f, *w)
                            }
                            _ => Av::U,
                        };
                    }
                }
                Av::U
            };
            let mut env: Vec<(VarId, Av)> = vec![];
            let ev = |env: &Vec<(VarId, Av)>, o: &Operand| -> Av {
                match o {
                    Operand::Num(k) => Av::C(*k),
                    Operand::Bool(k) => Av::B(*k),
                    Operand::Var(v) => env.iter().rev().find(|(x, _)| x == v).map(|e| e.1).unwrap_or_else(|| at_end(f, *v)),
                }
            };
            for s in &f.blocks[bi].stmts {
                let Stmt::Assign(d, rv) = s else { unreachable!() };
                let r = match rv {
                    Rv::Use(o) => ev(&env, o),
                    Rv::Un(Un::Not, o) => match ev(&env, o) {
                        Av::B(x) => Av::B(!x),
                        _ => Av::U,
                    },
                    Rv::Select(c, x, y) => match ev(&env, c) {
                        Av::B(true) => ev(&env, x),
                        Av::B(false) => ev(&env, y),
                        _ => Av::U,
                    },
                    Rv::Bin(op @ (Bin::Eq | Bin::Ne), x, y) => {
                        let eq = match (ev(&env, x), ev(&env, y)) {
                            (Av::C(a), Av::C(b)) => Some(a == b),
                            (Av::B(a), Av::B(b)) => Some(a == b),
                            (Av::Nz, Av::C(z)) | (Av::C(z), Av::Nz) if z == 0.0 => Some(false),
                            _ => None,
                        };
                        match eq {
                            Some(e) => Av::B(if *op == Bin::Eq { e } else { !e }),
                            None => Av::U,
                        }
                    }
                    Rv::Bin(op @ (Bin::Lt | Bin::Le), x, y) => match (ev(&env, x), ev(&env, y)) {
                        (Av::C(a), Av::C(b)) => Av::B(if *op == Bin::Lt { a < b } else { a <= b }),
                        _ => Av::U,
                    },
                    _ => Av::U,
                };
                env.push((*d, r));
            }
            let target = match &f.blocks[bi].term {
                Term::Br(c, t, e) => match ev(&env, c) {
                    Av::B(x) => Some(if x { *t } else { *e }),
                    _ => None,
                },
                Term::Switch(c, cases, d) => match ev(&env, c) {
                    Av::C(k) => Some(cases.iter().find(|(v, _)| *v == k).map(|c| c.1).unwrap_or(*d)),
                    _ => None,
                },
                _ => None,
            };
            let Some(t) = target else { continue };
            if t == b {
                continue;
            }
            let stmts = f.blocks[bi].stmts.clone();
            f.blocks[pi].stmts.extend(stmts);
            f.blocks[pi].term = Term::Goto(t);
            touched[pi] = true;
            touched[t as usize] = true;
            changed = true;
        }
    }
    changed
}
