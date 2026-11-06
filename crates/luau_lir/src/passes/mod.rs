//! Optimization passes over LIR.

use std::collections::HashMap;

use crate::ir::*;

mod cond;
mod inline;
mod loops;
mod mem;
mod promote;
mod range;

/// Merge functions with identical bodies (common across generic instantiations).
pub fn dedup_functions(p: &mut Program) -> bool {
    let n = p.funcs.len();
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut repl: Vec<u32> = (0..n as u32).collect();
    for (i, f) in p.funcs.iter().enumerate() {
        let key = format!(
            "{:?}|{:?}|{:?}|{:?}|{}|{:?}",
            f.params,
            f.rets,
            f.vars.iter().map(|v| (v.ty, v.range)).collect::<Vec<_>>(),
            f.blocks.iter().map(|b| (&b.stmts, &b.term)).collect::<Vec<_>>(),
            f.entry,
            f.frame_objects
        );
        match seen.get(&key) {
            Some(j) => repl[i] = *j,
            None => {
                seen.insert(key, i as u32);
            }
        }
    }
    if repl.iter().enumerate().all(|(i, r)| *r == i as u32) {
        return false;
    }
    for f in &mut p.funcs {
        for b in &mut f.blocks {
            for s in &mut b.stmts {
                if let Stmt::Call { callee: Callee::Func(c), .. } = s {
                    *c = repl[*c as usize];
                }
            }
        }
    }
    for t in p.table.iter_mut().skip(1) {
        *t = repl[*t as usize];
    }
    for e in &mut p.exports {
        e.1 = repl[e.1 as usize];
    }
    true
}

/// Merge blocks that are identical up to renaming of their block-local temporaries (e.g. the
/// arms of a match whose bodies compile to the same code).
fn merge_blocks(f: &mut Function) -> bool {
    let n = f.vars.len();
    // which block each var is defined/used in; usize::MAX = several
    let mut home: Vec<Option<usize>> = vec![None; n];
    let mut multi = vec![false; n];
    let note = |v: VarId, b: usize, home: &mut Vec<Option<usize>>, multi: &mut Vec<bool>| match home[v as usize] {
        None => home[v as usize] = Some(b),
        Some(h) if h != b => multi[v as usize] = true,
        _ => {}
    };
    for p in &f.params {
        multi[*p as usize] = true;
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        for s in &b.stmts {
            for d in s.def() {
                note(d, bi, &mut home, &mut multi);
            }
            for o in s.operands() {
                if let Operand::Var(v) = o {
                    note(*v, bi, &mut home, &mut multi);
                }
            }
        }
        let mut t = b.term.clone();
        for o in t.operands_mut() {
            if let Operand::Var(v) = o {
                note(*v, bi, &mut home, &mut multi);
            }
        }
    }
    let local = |v: VarId| !multi[v as usize];
    // blocks inside loops are left alone: sharing them can complicate hot-loop structure
    let cfg = crate::structure::Cfg::new(f);
    let mut in_loop = vec![false; f.blocks.len()];
    for (_, body) in loops::natural_loops(f, &cfg) {
        for b in body {
            in_loop[b as usize] = true;
        }
    }
    let mut key_of: HashMap<String, BlockId> = HashMap::new();
    let mut redirect: Vec<BlockId> = (0..f.blocks.len() as u32).collect();
    let mut any = false;
    for (bi, b) in f.blocks.iter().enumerate() {
        if (bi as u32) == f.entry || (b.stmts.is_empty() && b.term == Term::Unreachable) || in_loop[bi] {
            continue;
        }
        // sharing a small exit block (return / trap / diverging call) saves little but makes the
        // structured control flow around it worse; only merge big ones
        if b.term.successors().is_empty() && b.stmts.len() < 8 {
            continue;
        }
        // canonical text with local vars numbered by first appearance
        let mut map: HashMap<VarId, u32> = HashMap::new();
        let canon = |v: VarId, map: &mut HashMap<VarId, u32>| -> VarId {
            if local(v) {
                let k = map.len() as u32;
                u32::MAX - *map.entry(v).or_insert(k)
            } else {
                v
            }
        };
        let mut nb = b.clone();
        for s in &mut nb.stmts {
            // operands first (evaluation order), then defs
            for o in s.operands_mut() {
                if let Operand::Var(v) = o {
                    *v = canon(*v, &mut map);
                }
            }
            match s {
                Stmt::Assign(v, _) | Stmt::GetSp(v) => *v = canon(*v, &mut map),
                Stmt::Call { dests, .. } => {
                    for d in dests.iter_mut() {
                        *d = canon(*d, &mut map);
                    }
                }
                _ => {}
            }
        }
        for o in nb.term.operands_mut() {
            if let Operand::Var(v) = o {
                *v = canon(*v, &mut map);
            }
        }
        // locals must keep their types
        let tys: Vec<(u32, Ty)> = {
            let mut v: Vec<(u32, Ty)> = map.iter().map(|(var, k)| (*k, f.vars[*var as usize].ty)).collect();
            v.sort_by_key(|x| x.0);
            v
        };
        let key = format!("{:?}|{:?}|{:?}", nb.stmts, nb.term, tys);
        match key_of.get(&key) {
            Some(first) => {
                redirect[bi] = *first;
                any = true;
            }
            None => {
                key_of.insert(key, bi as u32);
            }
        }
    }
    if !any {
        return false;
    }
    // apply merges one at a time, keeping those that leave the CFG reducible
    let mut applied = false;
    for (from, to) in redirect.iter().enumerate().filter(|(i, t)| **t != *i as u32).map(|(i, t)| (i as u32, *t)) {
        let saved: Vec<Term> = f.blocks.iter().map(|b| b.term.clone()).collect();
        for b in &mut f.blocks {
            for t in b.term.successors_mut() {
                if *t == from {
                    *t = to;
                }
            }
        }
        if crate::structure::is_reducible(f) {
            applied = true;
        } else {
            for (b, t) in f.blocks.iter_mut().zip(saved) {
                b.term = t;
            }
        }
    }
    applied
}

pub fn optimize(p: &mut Program) {
    for f in &mut p.funcs {
        simplify(f);
    }
    program_dce(p);
    if std::env::var("LUAU_NO_INLINE").is_err() {
        inline::inline_program(p);
        program_dce(p);
    }
    // size-only clean-ups once inlining decisions are made
    if enabled("merge") {
        for f in &mut p.funcs {
            if merge_blocks(f) {
                simplify(f);
            }
        }
    }
    while dedup_functions(p) {
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
    f.frame_objects.retain(|o| map[o.0 as usize] != u32::MAX);
    for o in &mut f.frame_objects {
        o.0 = map[o.0 as usize];
    }
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
        changed |= local_copy_prop(f);
        changed |= fold_consts(f);
        changed |= dce(f);
        if enabled("loops") {
            changed |= loops::remove_dead_loops(f);
        }
        if enabled("cond") {
            changed |= cond::run(f);
            changed |= cond::thread(f);
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

/// Within a block: uses of `t` after `t = s` read `s` directly while neither is redefined.
fn local_copy_prop(f: &mut Function) -> bool {
    let mut changed = false;
    for b in &mut f.blocks {
        let mut map: Vec<(VarId, Operand)> = vec![];
        for s in &mut b.stmts {
            for o in s.operands_mut() {
                if let Operand::Var(v) = o {
                    if let Some((_, src)) = map.iter().find(|(t, _)| t == v) {
                        *o = *src;
                        changed = true;
                    }
                }
            }
            for d in s.def() {
                map.retain(|(t, src)| *t != d && *src != Operand::Var(d));
            }
            if let Stmt::Assign(t, Rv::Use(src)) = s {
                if Operand::Var(*t) != *src {
                    map.push((*t, *src));
                }
            }
        }
        for o in b.term.operands_mut() {
            if let Operand::Var(v) = o {
                if let Some((_, src)) = map.iter().find(|(t, _)| t == v) {
                    *o = *src;
                    changed = true;
                }
            }
        }
    }
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

/// Branches into `unreachable` blocks are dead (reaching them is UB in Rust): drop them.
fn prune_unreachable_edges(f: &mut Function) -> bool {
    let dead: Vec<bool> = f.blocks.iter().map(|b| b.stmts.is_empty() && b.term == Term::Unreachable).collect();
    let mut changed = false;
    for b in &mut f.blocks {
        let new = match &b.term {
            Term::Br(_, t, e) if dead[*t as usize] && !dead[*e as usize] => Some(Term::Goto(*e)),
            Term::Br(_, t, e) if dead[*e as usize] && !dead[*t as usize] => Some(Term::Goto(*t)),
            Term::Br(_, t, e) if dead[*t as usize] && dead[*e as usize] => Some(Term::Unreachable),
            Term::Switch(x, cases, d) => {
                let live: Vec<(f64, BlockId)> = cases.iter().copied().filter(|c| !dead[c.1 as usize]).collect();
                if dead[*d as usize] {
                    match live.split_last() {
                        None => Some(Term::Unreachable),
                        Some((last, rest)) if rest.is_empty() => Some(Term::Goto(last.1)),
                        Some((last, rest)) => Some(Term::Switch(*x, rest.to_vec(), last.1)),
                    }
                } else if live.len() != cases.len() {
                    Some(if live.is_empty() { Term::Goto(*d) } else { Term::Switch(*x, live, *d) })
                } else {
                    None
                }
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

/// Merge straight-line block chains and thread jumps through empty blocks.
fn cfg_simplify(f: &mut Function) -> bool {
    let mut changed = prune_unreachable_edges(f);
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
