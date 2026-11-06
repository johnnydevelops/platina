//! Optimization passes over LIR.

use crate::ir::*;

mod cond;
mod inline;
mod mem;
mod promote;
mod range;

pub fn optimize(p: &mut Program) {
    for f in &mut p.funcs {
        simplify(f);
    }
    program_dce(p);
    if std::env::var("LUAU_NO_INLINE").is_err() {
        inline::inline_program(p);
        program_dce(p);
    }
}

fn enabled(name: &str) -> bool {
    std::env::var("LUAU_DISABLE").map(|v| !v.split(',').any(|x| x == name)).unwrap_or(true)
}

pub fn simplify(f: &mut Function) {
    simplify_inner(f);
    compact_vars(f);
}

/// Renumber variables so that only referenced ones remain.
pub fn compact_vars(f: &mut Function) {
    let n = f.vars.len();
    let mut used = vec![false; n];
    for p in &f.params {
        used[*p as usize] = true;
    }
    for b in &f.blocks {
        for s in &b.stmts {
            for d in s.def() {
                used[d as usize] = true;
            }
            for o in s.operands() {
                if let Operand::Var(v) = o {
                    used[*v as usize] = true;
                }
            }
        }
        let mut t = b.term.clone();
        for o in t.operands_mut() {
            if let Operand::Var(v) = o {
                used[*v as usize] = true;
            }
        }
    }
    let mut map = vec![u32::MAX; n];
    let mut vars = Vec::new();
    for v in 0..n {
        if used[v] {
            map[v] = vars.len() as u32;
            vars.push(f.vars[v].clone());
        }
    }
    if vars.len() == n {
        return;
    }
    f.vars = vars;
    let m = |v: &mut VarId| *v = map[*v as usize];
    for p in &mut f.params {
        m(p);
    }
    for b in &mut f.blocks {
        for s in &mut b.stmts {
            match s {
                Stmt::Assign(v, _) | Stmt::GetSp(v) => m(v),
                Stmt::Call { dests, .. } => dests.iter_mut().for_each(m),
                _ => {}
            }
            for o in s.operands_mut() {
                if let Operand::Var(v) = o {
                    m(v);
                }
            }
        }
        for o in b.term.operands_mut() {
            if let Operand::Var(v) = o {
                m(v);
            }
        }
    }
}

fn simplify_inner(f: &mut Function) {
    for _ in 0..8 {
        let mut changed = false;
        changed |= cfg_simplify(f);
        changed |= copy_prop(f);
        changed |= fold_consts(f);
        changed |= dce(f);
        if enabled("cond") {
            changed |= cond::run(f);
        }
        if enabled("range") {
            changed |= range::run(f);
        }
        if enabled("mem") {
            changed |= promote::addr_fold(f);
            changed |= mem::run(f);
        }
        if enabled("promote") {
            changed |= promote::promote(f);
        }
        if enabled("coalesce") {
            changed |= coalesce(f);
        }
        if !changed {
            break;
        }
    }
}

fn def_counts(f: &Function) -> Vec<u32> {
    let mut d = vec![0u32; f.vars.len()];
    for p in &f.params {
        d[*p as usize] += 1;
    }
    for b in &f.blocks {
        for s in &b.stmts {
            for v in s.def() {
                d[v as usize] += 1;
            }
        }
    }
    d
}

fn use_counts(f: &Function) -> Vec<u32> {
    let mut u = vec![0u32; f.vars.len()];
    for b in &f.blocks {
        for s in &b.stmts {
            for o in s.operands() {
                if let Operand::Var(v) = o {
                    u[*v as usize] += 1;
                }
            }
        }
        let mut t = b.term.clone();
        for o in t.operands_mut() {
            if let Operand::Var(v) = o {
                u[*v as usize] += 1;
            }
        }
    }
    u
}

fn for_each_operand(f: &mut Function, mut g: impl FnMut(&mut Operand)) {
    for b in &mut f.blocks {
        for s in &mut b.stmts {
            for o in s.operands_mut() {
                g(o);
            }
        }
        for o in b.term.operands_mut() {
            g(o);
        }
    }
}

/// Replace uses of single-definition copies (`t = x` where `x` is constant or itself a
/// single-definition variable).
fn copy_prop(f: &mut Function) -> bool {
    let defs = def_counts(f);
    let mut repl: Vec<Option<Operand>> = vec![None; f.vars.len()];
    for b in &f.blocks {
        for s in &b.stmts {
            if let Stmt::Assign(t, Rv::Use(x)) = s {
                if defs[*t as usize] != 1 {
                    continue;
                }
                match x {
                    Operand::Var(src) if defs[*src as usize] == 1 && *src != *t => {
                        if f.vars[*src as usize].ty == f.vars[*t as usize].ty {
                            repl[*t as usize] = Some(*x);
                        }
                    }
                    Operand::Num(_) | Operand::Bool(_) => repl[*t as usize] = Some(*x),
                    _ => {}
                }
            }
        }
    }
    // resolve chains
    let resolve = |mut o: Operand, repl: &Vec<Option<Operand>>| {
        for _ in 0..64 {
            match o {
                Operand::Var(v) => match repl[v as usize] {
                    Some(n) => o = n,
                    None => break,
                },
                _ => break,
            }
        }
        o
    };
    let mut changed = false;
    for_each_operand(f, |o| {
        let n = resolve(*o, &repl);
        if n != *o {
            *o = n;
            changed = true;
        }
    });
    changed
}

/// Evaluate an rvalue with constant operands.
pub fn fold_rv(rv: &Rv) -> Option<Operand> {
    use Operand::Num as N;
    let u32ok = |x: f64| (0.0..4294967296.0).contains(&x) && x.fract() == 0.0;
    Some(match rv {
        Rv::Bin(op, N(a), N(b)) => {
            let (a, b) = (*a, *b);
            match op {
                Bin::Add => N(a + b),
                Bin::Sub => N(a - b),
                Bin::Mul if (a * b).abs() < 9007199254740992.0 => N(a * b),
                Bin::Div if b != 0.0 => N(a / b),
                Bin::FloorDiv if b != 0.0 => N((a / b).floor()),
                Bin::Mod if b != 0.0 => N(a - (a / b).floor() * b),
                Bin::Eq => Operand::Bool(a == b),
                Bin::Ne => Operand::Bool(a != b),
                Bin::Lt => Operand::Bool(a < b),
                Bin::Le => Operand::Bool(a <= b),
                Bin::BAnd if u32ok(a) && u32ok(b) => N(((a as u32) & (b as u32)) as f64),
                Bin::BOr if u32ok(a) && u32ok(b) => N(((a as u32) | (b as u32)) as f64),
                Bin::BXor if u32ok(a) && u32ok(b) => N(((a as u32) ^ (b as u32)) as f64),
                Bin::Shl if u32ok(a) && u32ok(b) => N(((a as u32).wrapping_shl(b as u32 & 31)) as f64),
                Bin::Shr if u32ok(a) && u32ok(b) => N(((a as u32).wrapping_shr(b as u32 & 31)) as f64),
                Bin::Min => N(a.min(b)),
                Bin::Max => N(a.max(b)),
                _ => return None,
            }
        }
        Rv::Bin(Bin::And, Operand::Bool(a), Operand::Bool(b)) => Operand::Bool(*a && *b),
        Rv::Bin(Bin::Or, Operand::Bool(a), Operand::Bool(b)) => Operand::Bool(*a || *b),
        Rv::Bin(Bin::Eq, Operand::Bool(a), Operand::Bool(b)) => Operand::Bool(a == b),
        Rv::Bin(Bin::Ne, Operand::Bool(a), Operand::Bool(b)) => Operand::Bool(a != b),
        Rv::Bin(Bin::And, Operand::Bool(true), x) | Rv::Bin(Bin::And, x, Operand::Bool(true)) => *x,
        Rv::Bin(Bin::Or, Operand::Bool(false), x) | Rv::Bin(Bin::Or, x, Operand::Bool(false)) => *x,
        Rv::Bin(Bin::Add, x @ Operand::Var(_), N(z)) | Rv::Bin(Bin::Add, N(z), x @ Operand::Var(_)) if *z == 0.0 => *x,
        Rv::Bin(Bin::Sub, x @ Operand::Var(_), N(z)) if *z == 0.0 => *x,
        Rv::Bin(Bin::Mul, x @ Operand::Var(_), N(o)) | Rv::Bin(Bin::Mul, N(o), x @ Operand::Var(_)) if *o == 1.0 => *x,
        Rv::Un(Un::Wrap(k), N(a)) => N(k.wrap_const(*a)),
        Rv::Un(Un::Neg, N(a)) => N(-a),
        Rv::Un(Un::Not, Operand::Bool(b)) => Operand::Bool(!b),
        Rv::Un(Un::BoolToNum, Operand::Bool(b)) => N(if *b { 1.0 } else { 0.0 }),
        Rv::Un(Un::NumToBool, N(a)) => Operand::Bool(*a != 0.0),
        Rv::Un(Un::F32Round, N(a)) => N(*a as f32 as f64),
        Rv::Un(Un::Floor, N(a)) => N(a.floor()),
        Rv::Un(Un::Ceil, N(a)) => N(a.ceil()),
        Rv::Un(Un::Abs, N(a)) => N(a.abs()),
        Rv::Un(Un::Sqrt, N(a)) => N(a.sqrt()),
        Rv::Un(Un::BNot, N(a)) if u32ok(*a) => N(!(*a as u32) as f64),
        Rv::Select(Operand::Bool(c), a, b) => {
            if *c {
                *a
            } else {
                *b
            }
        }
        Rv::Select(_, a, b) if a == b => *a,
        _ => return None,
    })
}

fn fold_consts(f: &mut Function) -> bool {
    let mut changed = false;
    for b in &mut f.blocks {
        for s in &mut b.stmts {
            if let Stmt::Assign(_, rv) = s {
                if matches!(rv, Rv::Use(_)) {
                    continue;
                }
                if let Some(o) = fold_rv(rv) {
                    *rv = Rv::Use(o);
                    changed = true;
                }
            }
        }
        // constant branches
        let new = match &b.term {
            Term::Br(Operand::Bool(c), t, e) => Some(Term::Goto(if *c { *t } else { *e })),
            Term::Br(_, t, e) if t == e => Some(Term::Goto(*t)),
            Term::Switch(Operand::Num(x), cases, d) => {
                Some(Term::Goto(cases.iter().find(|c| c.0 == *x).map(|c| c.1).unwrap_or(*d)))
            }
            _ => None,
        };
        if let Some(t) = new {
            b.term = t;
            changed = true;
        }
    }
    changed
}

fn dce(f: &mut Function) -> bool {
    let mut changed = false;
    loop {
        let uses = use_counts(f);
        let mut removed = false;
        for b in &mut f.blocks {
            let before = b.stmts.len();
            b.stmts.retain(|s| match s {
                Stmt::Assign(v, _) | Stmt::GetSp(v) => uses[*v as usize] > 0,
                _ => true,
            });
            for s in &mut b.stmts {
                if let Stmt::Call { dests, .. } = s {
                    if !dests.is_empty() && dests.iter().all(|d| uses[*d as usize] == 0) {
                        dests.clear();
                        removed = true;
                    }
                }
            }
            removed |= b.stmts.len() != before;
        }
        if !removed {
            break;
        }
        changed = true;
    }
    changed
}

/// Merge straight-line block chains and thread jumps through empty blocks.
fn cfg_simplify(f: &mut Function) -> bool {
    let mut changed = false;
    let n = f.blocks.len();
    // reachability + predecessor counts
    let mut reach = vec![false; n];
    let mut stack = vec![f.entry];
    while let Some(b) = stack.pop() {
        if std::mem::replace(&mut reach[b as usize], true) {
            continue;
        }
        stack.extend(f.blocks[b as usize].term.successors());
    }
    for (i, r) in reach.iter().enumerate() {
        if !r && !(f.blocks[i].stmts.is_empty() && f.blocks[i].term == Term::Unreachable) {
            f.blocks[i].stmts.clear();
            f.blocks[i].term = Term::Unreachable;
            changed = true;
        }
    }
    // thread jumps through empty goto-blocks
    let mut fwd: Vec<BlockId> = (0..n as u32).collect();
    for i in 0..n {
        if reach[i] && f.blocks[i].stmts.is_empty() && i as u32 != f.entry {
            if let Term::Goto(t) = f.blocks[i].term {
                if t as usize != i {
                    fwd[i] = t;
                }
            }
        }
    }
    let resolve = |mut b: BlockId| {
        for _ in 0..64 {
            let nb = fwd[b as usize];
            if nb == b {
                break;
            }
            b = nb;
        }
        b
    };
    for b in &mut f.blocks {
        for s in b.term.successors_mut() {
            let r = resolve(*s);
            if r != *s {
                *s = r;
                changed = true;
            }
        }
    }
    // merge A -> B when B has a single predecessor
    let mut preds = vec![0u32; n];
    for (i, b) in f.blocks.iter().enumerate() {
        if reach[i] {
            for s in crate::structure::succs_dedup(&b.term) {
                preds[s as usize] += 1;
            }
        }
    }
    for a in 0..n {
        if !reach[a] {
            continue;
        }
        while let Term::Goto(bb) = f.blocks[a].term {
            let bb = bb as usize;
            if bb == a || preds[bb] != 1 || bb as u32 == f.entry {
                break;
            }
            let moved = std::mem::replace(&mut f.blocks[bb], Block { stmts: vec![], term: Term::Unreachable });
            f.blocks[a].stmts.extend(moved.stmts);
            f.blocks[a].term = moved.term;
            preds[bb] = 0;
            changed = true;
        }
    }
    changed
}

/// `t = rv; ...; x = t` (t single-def, single-use, same block, x untouched in between)
/// becomes `x = rv`, so updates happen in place on the destination variable.
fn coalesce(f: &mut Function) -> bool {
    let defs = def_counts(f);
    let uses = use_counts(f);
    let mut changed = false;
    for b in &mut f.blocks {
        let mut i = 0;
        while i < b.stmts.len() {
            if let Stmt::Assign(x, Rv::Use(Operand::Var(t))) = b.stmts[i].clone() {
                let (ti, xi) = (t as usize, x as usize);
                if t != x && defs[ti] == 1 && uses[ti] == 1 && f.vars[ti].ty == f.vars[xi].ty {
                    // find t's def earlier in this block
                    if let Some(j) = (0..i).rev().find(|&j| matches!(&b.stmts[j], Stmt::Assign(d, _) if *d == t)) {
                        let between = &b.stmts[j + 1..i];
                        let touches_x = between.iter().any(|s| {
                            s.def().contains(&x) || s.operands().iter().any(|o| **o == Operand::Var(x))
                        });
                        // x must keep its declared range invariant: only coalesce into a ranged
                        // var if t's value is canonical anyway (t's def is a wrap or x is unranged)
                        let canonical = f.vars[xi].range.is_none()
                            || matches!(&b.stmts[j], Stmt::Assign(_, Rv::Un(Un::Wrap(_) | Un::WrapNear(..), _)))
                            || f.vars[ti].range == f.vars[xi].range;
                        if !touches_x && canonical {
                            let Stmt::Assign(_, rv) = b.stmts[j].clone() else { unreachable!() };
                            b.stmts[j] = Stmt::Assign(x, rv);
                            b.stmts.remove(i);
                            changed = true;
                            continue;
                        }
                    }
                }
            }
            i += 1;
        }
    }
    changed
}

/// Drop functions unreachable from exports and the function table; renumber ids.
pub fn program_dce(p: &mut Program) {
    let n = p.funcs.len();
    let mut live = vec![false; n];
    let mut stack: Vec<u32> = p.exports.iter().map(|e| e.1).chain(p.table.iter().skip(1).copied()).collect();
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut live[i as usize], true) {
            continue;
        }
        for b in &p.funcs[i as usize].blocks {
            for s in &b.stmts {
                if let Stmt::Call { callee: Callee::Func(c), .. } = s {
                    stack.push(*c);
                }
            }
        }
    }
    let mut new_id = vec![u32::MAX; n];
    let mut k = 0;
    for i in 0..n {
        if live[i] {
            new_id[i] = k;
            k += 1;
        }
    }
    let funcs = std::mem::take(&mut p.funcs);
    p.funcs = funcs.into_iter().enumerate().filter(|(i, _)| live[*i]).map(|(_, f)| f).collect();
    for f in &mut p.funcs {
        for b in &mut f.blocks {
            for s in &mut b.stmts {
                if let Stmt::Call { callee: Callee::Func(c), .. } = s {
                    *c = new_id[*c as usize];
                }
            }
        }
    }
    for t in p.table.iter_mut().skip(1) {
        *t = new_id[*t as usize];
    }
    for e in &mut p.exports {
        e.1 = new_id[e.1 as usize];
    }
}
