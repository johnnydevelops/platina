//! Whole-program inliner over LIR.

use crate::ir::*;

/// Cost of a function body (statements + blocks).
fn cost(f: &Function) -> usize {
    f.blocks.iter().map(|b| b.stmts.len() + 1).sum()
}

const SMALL: usize = 40;
const SINGLE_CALLER: usize = 120;
const MAX_VARS: usize = 900;
const MAX_COST: usize = 3000;

fn calls_of(f: &Function) -> Vec<u32> {
    let mut v = vec![];
    for b in &f.blocks {
        for s in &b.stmts {
            if let Stmt::Call { callee: Callee::Func(c), .. } = s {
                v.push(*c);
            }
        }
    }
    v
}

pub fn inline_program(p: &mut Program) {
    let n = p.funcs.len();
    // static call-site counts and exported/table (address-taken) functions
    let mut sites = vec![0usize; n];
    for f in &p.funcs {
        for c in calls_of(f) {
            sites[c as usize] += 1;
        }
    }
    // post-order over the call graph so callees are processed (and grown) first
    let mut order = vec![];
    let mut state = vec![0u8; n]; // 0 new, 1 on stack, 2 done
    let mut recursive = vec![false; n];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, Vec<u32>, usize)> = vec![(root, calls_of(&p.funcs[root]), 0)];
        state[root] = 1;
        while let Some((f, callees, i)) = stack.last_mut() {
            if *i < callees.len() {
                let c = callees[*i] as usize;
                *i += 1;
                match state[c] {
                    0 => {
                        state[c] = 1;
                        let cs = calls_of(&p.funcs[c]);
                        stack.push((c, cs, 0));
                    }
                    1 => recursive[c] = true,
                    _ => {}
                }
            } else {
                let f = *f;
                state[f] = 2;
                order.push(f);
                stack.pop();
            }
        }
    }
    for &fi in &order {
        let mut f = std::mem::replace(&mut p.funcs[fi], Function::new(""));
        let mut changed = false;
        loop {
            if f.vars.len() > MAX_VARS || cost(&f) > MAX_COST {
                break;
            }
            // find one inlinable call site
            let mut site = None;
            'outer: for (bi, b) in f.blocks.iter().enumerate() {
                for (si, s) in b.stmts.iter().enumerate() {
                    if let Stmt::Call { callee: Callee::Func(c), .. } = s {
                        let c = *c as usize;
                        if c == fi || recursive[c] {
                            continue;
                        }
                        let cc = cost(&p.funcs[c]);
                        if cc <= SMALL || (sites[c] == 1 && cc <= SINGLE_CALLER) {
                            site = Some((bi, si, c));
                            break 'outer;
                        }
                    }
                }
            }
            let Some((bi, si, c)) = site else { break };
            let callee = p.funcs[c].clone();
            splice(&mut f, bi, si, &callee);
            changed = true;
        }
        if changed {
            super::simplify(&mut f);
        }
        p.funcs[fi] = f;
    }
}

fn splice(f: &mut Function, bi: usize, si: usize, callee: &Function) {
    let Stmt::Call { dests, args, .. } = f.blocks[bi].stmts[si].clone() else { unreachable!() };
    // fresh vars
    let base = f.vars.len() as u32;
    for v in &callee.vars {
        f.vars.push(v.clone());
    }
    let mv = |v: VarId| v + base;
    let mo = |o: Operand| match o {
        Operand::Var(v) => Operand::Var(mv(v)),
        o => o,
    };
    // fresh blocks
    let bbase = f.blocks.len() as u32;
    let mb = |b: BlockId| b + bbase;
    // continuation block gets the rest of the original block
    let rest: Vec<Stmt> = f.blocks[bi].stmts.drain(si..).skip(1).collect();
    let term = std::mem::replace(&mut f.blocks[bi].term, Term::Unreachable);
    for b in &callee.blocks {
        let mut nb = b.clone();
        for s in &mut nb.stmts {
            match s {
                Stmt::Assign(v, _) | Stmt::GetSp(v) => *v = mv(*v),
                Stmt::Call { dests, .. } => {
                    for d in dests.iter_mut() {
                        *d = mv(*d);
                    }
                }
                _ => {}
            }
            for o in s.operands_mut() {
                *o = mo(*o);
            }
        }
        for o in nb.term.operands_mut() {
            *o = mo(*o);
        }
        for t in nb.term.successors_mut() {
            *t = mb(*t);
        }
        f.blocks.push(nb);
    }
    let cont = f.new_block();
    f.blocks[cont as usize].stmts = rest;
    f.blocks[cont as usize].term = term;
    // returns -> assign dests, goto cont
    for i in 0..callee.blocks.len() {
        let b = &mut f.blocks[mb(i as u32) as usize];
        if let Term::Return(ops) = &b.term {
            let ops = ops.clone();
            for (d, o) in dests.iter().zip(ops) {
                b.stmts.push(Stmt::Assign(*d, Rv::Use(o)));
            }
            b.term = Term::Goto(cont);
        }
    }
    // params <- args
    for (p, a) in callee.params.iter().zip(args) {
        f.blocks[bi].stmts.push(Stmt::Assign(mv(*p), Rv::Use(a)));
    }
    f.blocks[bi].term = Term::Goto(mb(callee.entry));
}
