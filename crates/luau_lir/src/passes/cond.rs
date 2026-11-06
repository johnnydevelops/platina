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
