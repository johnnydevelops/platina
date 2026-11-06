//! Loop clean-ups: removal of side-effect-free counted loops whose results are unused
//! (e.g. drop loops over elements with no-op drop glue).

use std::collections::BTreeSet;

use crate::ir::*;
use crate::structure::Cfg;

/// Natural loops: (header, body blocks).
pub fn natural_loops(f: &Function, cfg: &Cfg) -> Vec<(BlockId, BTreeSet<BlockId>)> {
    let mut out: Vec<(BlockId, BTreeSet<BlockId>)> = vec![];
    for &n in &cfg.rpo {
        for s in crate::structure::succs_dedup(&f.blocks[n as usize].term) {
            if cfg.rpo_index[s as usize] <= cfg.rpo_index[n as usize] && cfg.dominates(s, n) {
                // back edge n -> s
                let mut body = BTreeSet::new();
                body.insert(s);
                let mut stack = vec![n];
                while let Some(b) = stack.pop() {
                    if body.insert(b) {
                        stack.extend(cfg.preds[b as usize].iter().copied());
                    }
                }
                if let Some(l) = out.iter_mut().find(|l| l.0 == s) {
                    l.1.extend(body);
                } else {
                    out.push((s, body));
                }
            }
        }
    }
    out
}

pub fn remove_dead_loops(f: &mut Function) -> bool {
    let cfg = Cfg::new(f);
    let loops = natural_loops(f, &cfg);
    let n = f.vars.len();
    for (header, body) in loops {
        // no side effects inside
        if body.iter().any(|b| f.blocks[*b as usize].stmts.iter().any(|s| s.has_side_effects())) {
            continue;
        }
        if body.iter().any(|b| matches!(f.blocks[*b as usize].term, Term::Return(_) | Term::Trap(_) | Term::Unreachable)) {
            continue;
        }
        // single exit target
        let mut exits = BTreeSet::new();
        let mut exit_from = vec![];
        for b in &body {
            for s in crate::structure::succs_dedup(&f.blocks[*b as usize].term) {
                if !body.contains(&s) {
                    exits.insert(s);
                    exit_from.push(*b);
                }
            }
        }
        if exits.len() != 1 || exit_from.len() != 1 {
            continue;
        }
        let exit = *exits.iter().next().unwrap();
        // vars defined in the loop must be dead outside it
        let mut defined = vec![false; n];
        for b in &body {
            for s in &f.blocks[*b as usize].stmts {
                for d in s.def() {
                    defined[d as usize] = true;
                }
            }
        }
        let mut used_outside = false;
        for (bi, b) in f.blocks.iter().enumerate() {
            if body.contains(&(bi as u32)) {
                continue;
            }
            for s in &b.stmts {
                if s.operands().iter().any(|o| matches!(o, Operand::Var(v) if defined[*v as usize])) {
                    used_outside = true;
                }
            }
            let mut t = b.term.clone();
            if t.operands_mut().iter().any(|o| matches!(**o, Operand::Var(v) if defined[v as usize])) {
                used_outside = true;
            }
        }
        if used_outside {
            continue;
        }
        // termination: the exit test compares a +1-stepped counter against a loop-invariant bound
        if !terminates(f, &body, exit_from[0], &defined) {
            continue;
        }
        // redirect entries into the header to the exit
        for (bi, b) in f.blocks.iter_mut().enumerate() {
            if body.contains(&(bi as u32)) {
                continue;
            }
            for t in b.term.successors_mut() {
                if *t == header {
                    *t = exit;
                }
            }
        }
        return true;
    }
    false
}

fn terminates(f: &Function, body: &BTreeSet<BlockId>, exiting: BlockId, defined: &[bool]) -> bool {
    let Term::Br(Operand::Var(c), _, _) = f.blocks[exiting as usize].term else { return false };
    // find the compare
    let mut cmp = None;
    for s in &f.blocks[exiting as usize].stmts {
        if let Stmt::Assign(v, rv) = s {
            if *v == c {
                cmp = Some(rv.clone());
            }
        }
    }
    let Some(Rv::Bin(Bin::Eq | Bin::Ne | Bin::Lt | Bin::Le, a, b)) = cmp else { return false };
    let (ctr, bound) = match (a, b) {
        (Operand::Var(x), o) if defined[x as usize] => (x, o),
        (o, Operand::Var(x)) if defined[x as usize] => (x, o),
        _ => return false,
    };
    if let Operand::Var(bv) = bound {
        if defined[bv as usize] {
            return false;
        }
    }
    // the counter's in-loop definitions: copies of itself + 1 (possibly via temps / wraps)
    let defs_of = |v: VarId| -> Vec<Rv> {
        let mut r = vec![];
        for b in body {
            for s in &f.blocks[*b as usize].stmts {
                if let Stmt::Assign(d, rv) = s {
                    if *d == v {
                        r.push(rv.clone());
                    }
                }
            }
        }
        r
    };
    let inc_of = |x: VarId, v: VarId| -> bool {
        // x = v + 1, or x is a temp defined as v + 1
        x == v || defs_of(x).iter().any(|r2| matches!(r2, Rv::Bin(Bin::Add, Operand::Var(y), Operand::Num(o)) if *y == v && *o == 1.0))
    };
    let steps_by_one = |v: VarId| -> bool {
        let ds = defs_of(v);
        !ds.is_empty()
            && ds.iter().all(|rv| match rv {
                Rv::Bin(Bin::Add, Operand::Var(x), Operand::Num(one)) => *one == 1.0 && *x == v,
                Rv::Un(Un::Wrap(_) | Un::WrapNear(..), Operand::Var(x)) => inc_of(*x, v),
                Rv::Use(Operand::Var(x)) => *x != v && inc_of(*x, v),
                _ => false,
            })
    };
    steps_by_one(ctr)
}
