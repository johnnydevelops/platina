//! Interval range analysis: deletes redundant integer wraps and sinks wraps through
//! add/sub chains (doubles are exact below 2^53, so `((a+b)%m + c)%m == (a+b+c)%m`).

use crate::ir::*;

const INF: f64 = f64::INFINITY;
const EXACT: f64 = 4503599627370496.0; // 2^52 safety margin

#[derive(Clone, Copy, Debug, PartialEq)]
struct Iv(f64, f64);

impl Iv {
    const FULL: Iv = Iv(-INF, INF);
    const U32: Iv = Iv(0.0, 4294967295.0);
    fn union(self, o: Iv) -> Iv {
        Iv(self.0.min(o.0), self.1.max(o.1))
    }
    fn within(self, lo: f64, hi: f64) -> bool {
        self.0 >= lo && self.1 <= hi
    }
    fn mag(self) -> f64 {
        self.0.abs().max(self.1.abs())
    }
}

fn mem_iv(m: Mem) -> Iv {
    match m {
        Mem::U8 => Iv(0.0, 255.0),
        Mem::I8 => Iv(-128.0, 127.0),
        Mem::U16 => Iv(0.0, 65535.0),
        Mem::I16 => Iv(-32768.0, 32767.0),
        Mem::U32 => Iv::U32,
        Mem::I32 => Iv(-2147483648.0, 2147483647.0),
        _ => Iv::FULL,
    }
}

struct Ranges {
    iv: Vec<Option<Iv>>,
}

impl Ranges {
    fn op(&self, o: &Operand) -> Iv {
        match o {
            Operand::Num(n) => Iv(*n, *n),
            Operand::Bool(_) => Iv(0.0, 1.0),
            Operand::Var(v) => self.iv[*v as usize].unwrap_or(Iv::FULL),
        }
    }
    fn rv(&self, rv: &Rv) -> Iv {
        match rv {
            Rv::Use(o) => self.op(o),
            Rv::Bin(op, a, b) => {
                let (x, y) = (self.op(a), self.op(b));
                match op {
                    Bin::Add => Iv(x.0 + y.0, x.1 + y.1),
                    Bin::Sub => Iv(x.0 - y.1, x.1 - y.0),
                    Bin::Mul => {
                        let c = [x.0 * y.0, x.0 * y.1, x.1 * y.0, x.1 * y.1];
                        if c.iter().any(|v| v.is_nan()) {
                            Iv::FULL
                        } else {
                            Iv(c.iter().cloned().fold(INF, f64::min), c.iter().cloned().fold(-INF, f64::max))
                        }
                    }
                    Bin::FloorDiv if x.0 >= 0.0 && y.0 >= 1.0 => Iv(0.0, x.1),
                    Bin::Mod if y.0 == y.1 && y.0 > 0.0 => Iv(0.0, y.0 - 1.0),
                    Bin::Mod if y.0 >= 1.0 => Iv(0.0, y.1 - 1.0),
                    Bin::FMod if y.0 >= 1.0 => Iv(-(y.1 - 1.0).min(x.mag()), (y.1 - 1.0).min(x.mag())),
                    Bin::BAnd if x.0 >= 0.0 && y.0 >= 0.0 => Iv(0.0, x.1.min(y.1).min(4294967295.0)),
                    Bin::BAnd | Bin::BOr | Bin::BXor | Bin::Shl | Bin::Sar | Bin::Rol | Bin::Ror | Bin::IMul32 => Iv::U32,
                    Bin::Shr if x.0 >= 0.0 && x.1 <= 4294967295.0 => Iv(0.0, x.1),
                    Bin::Shr => Iv::U32,
                    Bin::Min => Iv(x.0.min(y.0), x.1.min(y.1)),
                    Bin::Max => Iv(x.0.max(y.0), x.1.max(y.1)),
                    Bin::Eq | Bin::Ne | Bin::Lt | Bin::Le | Bin::And | Bin::Or => Iv(0.0, 1.0),
                    _ => Iv::FULL,
                }
            }
            Rv::Un(op, a) => {
                let x = self.op(a);
                match op {
                    Un::Neg => Iv(-x.1, -x.0),
                    Un::Wrap(k) | Un::WrapNear(k, ..) => {
                        if x.within(k.min(), k.max()) {
                            x
                        } else {
                            Iv(k.min(), k.max())
                        }
                    }
                    Un::BNot | Un::BSwap => Iv::U32,
                    Un::Clz | Un::Ctz | Un::Popcnt => Iv(0.0, 32.0),
                    Un::BoolToNum => Iv(0.0, 1.0),
                    Un::FToI(k) => Iv(k.min(), k.max()),
                    Un::Abs => Iv(0.0, x.mag()),
                    _ => Iv::FULL,
                }
            }
            Rv::Load { mem, .. } => mem_iv(*mem),
            Rv::Select(_, a, b) => self.op(a).union(self.op(b)),
            Rv::Builtin(..) => Iv::FULL,
        }
    }
}

/// For each block: interval refinements valid at block entry, from the branch of its single
/// predecessor (`a < b` / `a <= b` on integer-ranged operands). Refinements for a var hold
/// until that var is redefined inside the block.
fn edge_facts(f: &Function, r: &Ranges) -> Vec<Vec<(VarId, Iv)>> {
    let n = f.vars.len();
    let mut def_at: Vec<Option<(usize, usize)>> = vec![None; n];
    let mut defs = vec![0u32; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (si, s) in b.stmts.iter().enumerate() {
            for d in s.def() {
                defs[d as usize] += 1;
                def_at[d as usize] = Some((bi, si));
            }
        }
    }
    let mut preds = vec![Vec::new(); f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        for s in crate::structure::succs_dedup(&b.term) {
            preds[s as usize].push(bi);
        }
    }
    let int = |v: VarId| f.vars[v as usize].range.is_some();
    let mut out = vec![Vec::new(); f.blocks.len()];
    for (bi, ps) in preds.iter().enumerate() {
        if ps.len() != 1 {
            continue;
        }
        let p = ps[0];
        let Term::Br(Operand::Var(c), t, e) = f.blocks[p].term else { continue };
        if t == e || defs[c as usize] != 1 {
            continue;
        }
        let Some((db, ds)) = def_at[c as usize] else { continue };
        if db != p {
            continue;
        }
        let Stmt::Assign(_, Rv::Bin(op, a, b)) = &f.blocks[p].stmts[ds] else { continue };
        // operands must not be redefined between the compare and the branch
        let later = &f.blocks[p].stmts[ds + 1..];
        let redefined = |o: &Operand| match o {
            Operand::Var(v) => later.iter().any(|s| s.def().contains(v)),
            _ => false,
        };
        if redefined(a) || redefined(b) {
            continue;
        }
        let taken = t as usize == bi;
        // normalize to x < y (strict) or x <= y
        let (x, y, strict) = match (op, taken) {
            (Bin::Lt, true) => (*a, *b, true),
            (Bin::Le, true) => (*a, *b, false),
            (Bin::Lt, false) => (*b, *a, false), // !(a<b) => b <= a
            (Bin::Le, false) => (*b, *a, true),  // !(a<=b) => b < a
            _ => continue,
        };
        let is_int = |o: &Operand| match o {
            Operand::Var(v) => int(*v),
            Operand::Num(n) => n.fract() == 0.0,
            _ => false,
        };
        if !(is_int(&x) && is_int(&y)) {
            continue;
        }
        let d = if strict { 1.0 } else { 0.0 };
        let (ix, iy) = (r.op(&x), r.op(&y));
        if let Operand::Var(xv) = x {
            out[bi].push((xv, Iv(ix.0, ix.1.min(iy.1 - d))));
        }
        if let Operand::Var(yv) = y {
            out[bi].push((yv, Iv(iy.0.max(ix.0 + d), iy.1)));
        }
    }
    out
}

fn analyze(f: &Function) -> Ranges {
    let base = analyze_flow(f, &vec![Vec::new(); f.blocks.len()]);
    let facts = edge_facts(f, &base);
    if facts.iter().all(|v| v.is_empty()) {
        return base;
    }
    analyze_flow(f, &facts)
}

fn analyze_flow(f: &Function, facts: &[Vec<(VarId, Iv)>]) -> Ranges {
    let n = f.vars.len();
    let declared: Vec<Option<Iv>> = f.vars.iter().map(|v| v.range.map(|(a, b)| Iv(a, b))).collect();
    let mut r = Ranges { iv: vec![None; n] };
    let _ = &facts;
    // params, call results, sp: declared or unknown
    for p in &f.params {
        r.iv[*p as usize] = Some(declared[*p as usize].unwrap_or(Iv::FULL));
    }
    for b in &f.blocks {
        for s in &b.stmts {
            match s {
                Stmt::Call { dests, .. } => {
                    for d in dests {
                        r.iv[*d as usize] = Some(declared[*d as usize].unwrap_or(Iv::FULL));
                    }
                }
                Stmt::GetSp(v) => r.iv[*v as usize] = Some(Iv::U32),
                _ => {}
            }
        }
    }
    // fixpoint with widening
    for round in 0..6 {
        let mut changed = false;
        for (bi, b) in f.blocks.iter().enumerate() {
            // block-entry refinements, valid until the var is redefined in this block
            let mut local: Vec<(VarId, Iv)> = facts[bi].clone();
            for s in &b.stmts {
                if let Stmt::Assign(v, rv) = s {
                    let mut x = if local.is_empty() {
                        r.rv(rv)
                    } else {
                        let saved: Vec<(VarId, Option<Iv>)> = local.iter().map(|(lv, _)| (*lv, r.iv[*lv as usize])).collect();
                        for (lv, iv) in &local {
                            let cur = r.iv[*lv as usize].unwrap_or(Iv::FULL);
                            r.iv[*lv as usize] = Some(Iv(cur.0.max(iv.0), cur.1.min(iv.1)));
                        }
                        let x = r.rv(rv);
                        for (lv, old) in saved {
                            r.iv[lv as usize] = old;
                        }
                        x
                    };
                    local.retain(|(lv, _)| lv != v);
                    if x.0.is_nan() || x.1.is_nan() {
                        x = Iv::FULL;
                    }
                    let cur = r.iv[*v as usize];
                    let mut nx = match cur {
                        None => x,
                        Some(c) => c.union(x),
                    };
                    if round >= 4 && Some(nx) != cur {
                        nx = Iv::FULL; // widen
                    }
                    if let Some(d) = declared[*v as usize] {
                        nx = Iv(nx.0.max(d.0), nx.1.min(d.1));
                    }
                    if Some(nx) != cur {
                        r.iv[*v as usize] = Some(nx);
                        changed = true;
                    }
                } else {
                    for d in s.def() {
                        local.retain(|(lv, _)| *lv != d);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    r
}

pub fn run(f: &mut Function) -> bool {
    let mut changed = false;
    // 1. redundant wraps
    let r = analyze(f);
    for b in &mut f.blocks {
        for s in &mut b.stmts {
            if let Stmt::Assign(_, rv) = s {
                if let Rv::Un(Un::Wrap(k) | Un::WrapNear(k, ..), x) = rv {
                    let (k, x) = (*k, *x);
                    let iv = r.op(&x);
                    if iv.within(k.min(), k.max()) {
                        *rv = Rv::Use(x);
                        changed = true;
                    } else if matches!(rv, Rv::Un(Un::Wrap(_), _))
                        && iv.within(k.min() - k.modulus(), k.max() + k.modulus())
                    {
                        *rv = Rv::Un(Un::WrapNear(k, iv.1 > k.max(), iv.0 < k.min()), x);
                        changed = true;
                    }
                }
            }
        }
    }
    changed |= sink_wraps(f);
    changed
}

/// Remove `t = wrap(x)` when `t` (an unranged temp) is used once, by an add/sub chain
/// that ends in a wrap of no larger width, and the unwrapped chain stays exact.
fn sink_wraps(f: &mut Function) -> bool {
    let n = f.vars.len();
    // def site and single use site of each var
    let mut def: Vec<Option<(usize, usize)>> = vec![None; n];
    let mut defs = vec![0u32; n];
    let mut uses = vec![0u32; n];
    let mut user: Vec<Option<(usize, usize)>> = vec![None; n];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (si, s) in b.stmts.iter().enumerate() {
            for d in s.def() {
                defs[d as usize] += 1;
                def[d as usize] = Some((bi, si));
            }
            for o in s.operands() {
                if let Operand::Var(v) = o {
                    uses[*v as usize] += 1;
                    user[*v as usize] = Some((bi, si));
                }
            }
        }
        let mut t = b.term.clone();
        for o in t.operands_mut() {
            if let Operand::Var(v) = o {
                uses[*v as usize] += 2; // terminators are never wrap sinks
            }
        }
    }
    for p in &f.params {
        defs[*p as usize] += 1;
    }
    let mut changed = false;
    let mut r = analyze(f);
    for t in 0..n {
        if defs[t] != 1 || uses[t] != 1 || f.vars[t].range.is_some() {
            continue;
        }
        let Some((bi, si)) = def[t] else { continue };
        let Stmt::Assign(_, Rv::Un(Un::Wrap(k), x)) = &f.blocks[bi].stmts[si] else { continue };
        let (k, x) = (*k, *x);
        if r.op(&x).mag() >= EXACT {
            continue;
        }
        // follow the single-use add/sub chain to a sink wrap
        let mut cur = t;
        let mut extra = r.op(&x).mag() - r.iv[t].map(|i| i.mag()).unwrap_or(0.0);
        let mut ok = false;
        for _ in 0..64 {
            if uses[cur] != 1 {
                break;
            }
            let Some((ub, us)) = user[cur] else { break };
            let Stmt::Assign(dst, rv) = &f.blocks[ub].stmts[us] else { break };
            let dst = *dst as usize;
            match rv {
                Rv::Un(Un::Wrap(k2) | Un::WrapNear(k2, ..), _) if k2.bits <= k.bits => {
                    ok = true;
                    break;
                }
                Rv::Bin(Bin::Add | Bin::Sub, _, _) if f.vars[dst].range.is_none() && defs[dst] == 1 => {
                    let m = r.iv[dst].map(|i| i.mag()).unwrap_or(INF);
                    if m + extra.max(0.0) >= EXACT {
                        break;
                    }
                    cur = dst;
                    extra = extra.max(0.0);
                }
                _ => break,
            }
        }
        if ok {
            f.blocks[bi].stmts[si] = Stmt::Assign(t as VarId, Rv::Use(x));
            changed = true;
            r = analyze(f);
        }
    }
    changed
}
