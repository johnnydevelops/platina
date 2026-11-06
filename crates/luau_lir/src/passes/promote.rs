//! Scalar promotion of shadow-stack frame slots ("mem2reg"): when every access to a frame is
//! a load/store at a constant offset (possibly through pointers to the frame that are
//! themselves stored only in that frame), each slot becomes a Luau local.

use std::collections::HashMap;

use crate::ir::*;

/// Fold `t = b + c` into load/store offsets, and expand small constant-size copies.
pub fn addr_fold(f: &mut Function) -> bool {
    let n = f.vars.len();
    let mut defs = vec![0u32; n];
    for p in &f.params {
        defs[*p as usize] += 1;
    }
    let mut def_rv: Vec<Option<Rv>> = vec![None; n];
    for b in &f.blocks {
        for s in &b.stmts {
            for d in s.def() {
                defs[d as usize] += 1;
            }
            if let Stmt::Assign(v, rv) = s {
                def_rv[*v as usize] = Some(rv.clone());
            }
        }
    }
    // var -> (base var, offset) for single-def `v = base + c`
    let split = |o: Operand, extra: i32| -> Option<(Operand, i32)> {
        let Operand::Var(mut v) = o else { return None };
        let mut off: i64 = extra as i64;
        let mut moved = false;
        for _ in 0..16 {
            if defs[v as usize] != 1 {
                break;
            }
            match &def_rv[v as usize] {
                Some(Rv::Bin(Bin::Add, Operand::Var(b), Operand::Num(c))) | Some(Rv::Bin(Bin::Add, Operand::Num(c), Operand::Var(b)))
                    if c.fract() == 0.0 && c.abs() < 1e9 && defs[*b as usize] == 1 =>
                {
                    off += *c as i64;
                    v = *b;
                    moved = true;
                }
                Some(Rv::Bin(Bin::Sub, Operand::Var(b), Operand::Num(c)))
                    if c.fract() == 0.0 && c.abs() < 1e9 && defs[*b as usize] == 1 =>
                {
                    off -= *c as i64;
                    v = *b;
                    moved = true;
                }
                Some(Rv::Use(Operand::Var(b))) if defs[*b as usize] == 1 => {
                    v = *b;
                    moved = true;
                }
                _ => break,
            }
        }
        if moved && off.abs() < (1 << 30) { Some((Operand::Var(v), off as i32)) } else { None }
    };
    let mut changed = false;
    // load through `t = select(c, p, q)` -> select(c, load p, load q)
    for bi in 0..f.blocks.len() {
        let mut i = 0;
        while i < f.blocks[bi].stmts.len() {
            if let Stmt::Assign(v, Rv::Load { mem, addr: Operand::Var(t), off }) = f.blocks[bi].stmts[i].clone() {
                if defs[t as usize] == 1 {
                    if let Some(Rv::Select(c, p @ Operand::Var(_), q @ Operand::Var(_))) = &def_rv[t as usize] {
                        let (c, p, q) = (*c, *p, *q);
                        let ty = f.vars[v as usize].ty;
                        let r = f.vars[v as usize].range;
                        f.vars.push(VarInfo { ty, name: None, range: r });
                        let x1 = (f.vars.len() - 1) as VarId;
                        f.vars.push(VarInfo { ty, name: None, range: r });
                        let x2 = (f.vars.len() - 1) as VarId;
                        f.blocks[bi].stmts.splice(
                            i..=i,
                            [
                                Stmt::Assign(x1, Rv::Load { mem, addr: p, off }),
                                Stmt::Assign(x2, Rv::Load { mem, addr: q, off }),
                                Stmt::Assign(v, Rv::Select(c, Operand::Var(x1), Operand::Var(x2))),
                            ],
                        );
                        changed = true;
                        i += 3;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }
    for b in &mut f.blocks {
        for s in &mut b.stmts {
            match s {
                Stmt::Assign(_, Rv::Load { addr, off, .. }) | Stmt::Store { addr, off, .. } => {
                    if let Some((na, no)) = split(*addr, *off) {
                        // keep frame pointers recognisable: only fold onto vars, never consts
                        *addr = na;
                        *off = no;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        // expand small fixed-size copies into loads + stores
        let mut i = 0;
        while i < b.stmts.len() {
            if let Stmt::Copy { dst, src, len: Operand::Num(n) } = b.stmts[i].clone() {
                let n = n as i64;
                if (1..=32).contains(&n) {
                    let mut parts = vec![];
                    let mut o = 0i64;
                    while o < n {
                        let rem = n - o;
                        let m = if rem >= 4 { Mem::U32 } else if rem >= 2 { Mem::U16 } else { Mem::U8 };
                        parts.push((o as i32, m));
                        o += m.size() as i64;
                    }
                    let mut loads = vec![];
                    let mut stores = vec![];
                    for (o, m) in parts {
                        f.vars.push(VarInfo { ty: Ty::Num, name: None, range: None });
                        let t = (f.vars.len() - 1) as VarId;
                        loads.push(Stmt::Assign(t, Rv::Load { mem: m, addr: src, off: o }));
                        stores.push(Stmt::Store { mem: m, addr: dst, off: o, val: Operand::Var(t) });
                    }
                    let k = loads.len() + stores.len();
                    b.stmts.splice(i..=i, loads.into_iter().chain(stores));
                    i += k;
                    changed = true;
                    continue;
                }
            }
            i += 1;
        }
    }
    changed
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Slot {
    off: i32,
    mem: Mem,
}

pub fn promote(f: &mut Function) -> bool {
    let n = f.vars.len();
    let mut defs = vec![0u32; n];
    let mut sp_vars = vec![false; n];
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
    // frame roots: shadow-stack snapshots (`v = SP`); frame pointers derive from them
    let mut root_of: Vec<Option<(VarId, i64)>> = vec![None; n];
    for v in 0..n {
        if sp_vars[v] && defs[v] == 1 {
            root_of[v] = Some((v as VarId, 0));
        }
    }
    if root_of.iter().all(|r| r.is_none()) {
        return false;
    }
    // derived pointers: single-def `v = d + c`, `v = d`, or loads of slots whose stores all
    // store the same derived pointer (fixpoint)
    let mut slot_ptr: HashMap<(VarId, i32), Option<(VarId, i64)>> = HashMap::new();
    // slots through which frame pointers were derived by loading
    let mut via: Vec<(VarId, i32)> = vec![];
    for _ in 0..8 {
        let mut changed = false;
        // slot stores of derived pointers
        let mut sp: HashMap<(VarId, i32), Option<(VarId, i64)>> = HashMap::new();
        for b in &f.blocks {
            for s in &b.stmts {
                if let Stmt::Store { mem: Mem::U32, addr: Operand::Var(a), off, val } = s {
                    if let Some((r, c)) = root_of[*a as usize] {
                        let key = (r, (c + *off as i64) as i32);
                        let v = match val {
                            Operand::Var(x) => root_of[*x as usize],
                            _ => None,
                        };
                        let e = sp.entry(key).or_insert(v);
                        if *e != v {
                            *e = None;
                        }
                    }
                }
            }
        }
        // every definition of a var must agree on the same (root, offset)
        let mut cand: Vec<Option<Option<(VarId, i64)>>> = vec![None; n];
        for b in &f.blocks {
            for s in &b.stmts {
                match s {
                    Stmt::Assign(v, rv) => {
                        if sp_vars[*v as usize] {
                            continue;
                        }
                        let d = match rv {
                            Rv::Use(Operand::Var(x)) => root_of[*x as usize],
                            Rv::Bin(Bin::Add, Operand::Var(x), Operand::Num(c)) | Rv::Bin(Bin::Add, Operand::Num(c), Operand::Var(x))
                                if c.fract() == 0.0 =>
                            {
                                root_of[*x as usize].map(|(r, o)| (r, o + *c as i64))
                            }
                            Rv::Bin(Bin::Sub, Operand::Var(x), Operand::Num(c)) if c.fract() == 0.0 => {
                                root_of[*x as usize].map(|(r, o)| (r, o - *c as i64))
                            }
                            Rv::Load { mem: Mem::U32, addr: Operand::Var(a), off } => root_of[*a as usize].and_then(|(r, c)| {
                                let key = (r, (c + *off as i64) as i32);
                                let d = sp.get(&key).copied().flatten();
                                if d.is_some() {
                                    via.push(key);
                                }
                                d
                            }),
                            _ => None,
                        };
                        let e = &mut cand[*v as usize];
                        *e = match *e {
                            None => Some(d),
                            Some(prev) if prev == d => Some(d),
                            Some(_) => Some(None),
                        };
                    }
                    other => {
                        for d in other.def() {
                            cand[d as usize] = Some(None);
                        }
                    }
                }
            }
        }
        for p in &f.params {
            cand[*p as usize] = Some(None);
        }
        for v in 0..n {
            if sp_vars[v] {
                continue;
            }
            if let Some(Some(d)) = cand[v] {
                if root_of[v] != Some(d) {
                    root_of[v] = Some(d);
                    changed = true;
                }
            }
        }
        slot_ptr = sp;
        if !changed {
            break;
        }
    }
    let _ = &slot_ptr;
    // escape analysis per root, and slot inventory
    let roots: Vec<VarId> = (0..n as u32).filter(|v| root_of[*v as usize] == Some((*v, 0))).collect();
    let mut escaped: HashMap<VarId, bool> = roots.iter().map(|r| (*r, false)).collect();
    let mut slots: HashMap<VarId, HashMap<i32, Mem>> = HashMap::new();
    let dbg = std::env::var("LUAU_DEBUG_PROMOTE").map(|v| f.name.contains(&v)).unwrap_or(false);
    let mut bad = |r: VarId, escaped: &mut HashMap<VarId, bool>| {
        escaped.insert(r, true);
    };
    // escaped objects per root: offsets that escaped (None = whole frame)
    let mut esc_at: HashMap<VarId, Vec<Option<i64>>> = HashMap::new();
    macro_rules! why {
        ($r:expr, $off:expr, $s:expr) => {
            if dbg {
                eprintln!("promote: root v{} escapes (off {:?}) at {:?}", $r, $off, $s);
            }
            esc_at.entry($r).or_default().push($off);
        };
    }
    let _ = &mut bad;
    let mut ptr_stores: Vec<((VarId, i32), (VarId, i64))> = vec![];
    for b in &f.blocks {
        for s in &b.stmts {
            match s {
                Stmt::Assign(v, rv) => {
                    let derived_def = root_of[*v as usize].is_some();
                    match rv {
                        Rv::Load { mem, addr: Operand::Var(a), off } if root_of[*a as usize].is_some() => {
                            let (r, c) = root_of[*a as usize].unwrap();
                            let o = (c + *off as i64) as i32;
                            let e = slots.entry(r).or_default();
                            match e.get(&o) {
                                Some(m) if m != mem => { why!(r, Some(o as i64), s); }
                                _ => {
                                    e.insert(o, *mem);
                                }
                            }
                        }
                        _ => {
                            // derived pointers may only flow into other derived defs
                            for o in rv.operands() {
                                if let Operand::Var(x) = o {
                                    if let Some((r, c)) = root_of[*x as usize] {
                                        if !derived_def {
                                            why!(r, Some(c), s);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Stmt::Store { mem, addr, off, val } => {
                    let addr_root = match addr {
                        Operand::Var(a) => root_of[*a as usize],
                        _ => None,
                    };
                    if let Some((r, c)) = addr_root {
                        let o = (c + *off as i64) as i32;
                        let e = slots.entry(r).or_default();
                        match e.get(&o) {
                            Some(m) if m != mem => { why!(r, Some(o as i64), s); }
                            _ => {
                                e.insert(o, *mem);
                            }
                        }
                    }
                    if let Operand::Var(x) = val {
                        if let Some((vr, vc)) = root_of[*x as usize] {
                            // storing a frame pointer: fine only into a slot of the same frame
                            // (and only while that slot itself does not escape; checked below)
                            let consistent = addr_root
                                .map(|(r, c)| matches!(slot_ptr.get(&(r, (c + *off as i64) as i32)), Some(Some(_))))
                                .unwrap_or(false);
                            if addr_root.map(|(r, _)| r) != Some(vr) || *mem != Mem::U32 || !consistent {
                                // loads from that slot can't be traced back to this frame
                                why!(vr, Some(vc), s);
                            } else if let Some((r, c)) = addr_root {
                                ptr_stores.push(((r, (c + *off as i64) as i32), (vr, vc)));
                            }
                        }
                    }
                }
                Stmt::SetSp(_) => {}
                _ => {
                    for o in s.operands() {
                        if let Operand::Var(x) = o {
                            if let Some((r, c)) = root_of[*x as usize] {
                                why!(r, Some(c), s);
                            }
                        }
                    }
                }
            }
        }
        let mut t = b.term.clone();
        for o in t.operands_mut() {
            if let Operand::Var(x) = o {
                if let Some((r, c)) = root_of[*x as usize] {
                    why!(r, Some(c), &b.term);
                }
            }
        }
    }
    // overlapping slots of different sizes -> not promotable
    for (r, sl) in &slots {
        let mut v: Vec<(i32, Mem)> = sl.iter().map(|(o, m)| (*o, *m)).collect();
        v.sort_by_key(|x| x.0);
        for w in v.windows(2) {
            if w[0].0 + w[0].1.size() as i32 > w[1].0 {
                if dbg {
                    eprintln!("promote: root v{r} overlapping slots {:?} {:?}", w[0], w[1]);
                }
                esc_at.entry(*r).or_default().push(Some(w[0].0 as i64));
                esc_at.entry(*r).or_default().push(Some(w[1].0 as i64));
            }
        }
    }
    // object of a frame offset
    let object_of = |r: VarId, o: i64| -> Option<(i64, i64)> {
        f.frame_objects.iter().find(|(v, a, b)| *v == r && o >= *a && o < *b).map(|(_, a, b)| (*a, *b))
    };
    // slots that may be accessed through escaped pointers; pointers stored in escaped slots
    // escape too (fixpoint)
    let mut slot_ok: HashMap<(VarId, i32), bool> = HashMap::new();
    for _round in 0..16 {
    slot_ok.clear();
    for r in &roots {
        let Some(sl) = slots.get(r) else { continue };
        let esc = esc_at.get(r).cloned().unwrap_or_default();
        for (o, m) in sl {
            let o64 = *o as i64;
            let end = o64 + m.size() as i64;
            let tainted = escaped[r]
                || esc.iter().any(|e| match e {
                    None => true,
                    Some(eo) => match (object_of(*r, *eo), object_of(*r, o64)) {
                        // provenance: an escaped pointer reaches its whole object only
                        (Some(ea), Some(sa)) => ea == sa,
                        // unknown object layout: assume it reaches everything
                        _ => true,
                    },
                })
                || (object_of(*r, o64).is_none() && !esc.is_empty())
                || object_of(*r, o64).is_some_and(|(_, b)| end > b);
            slot_ok.insert((*r, *o), !tainted);
        }
    }
    let mut grew = false;
    for (slot, (vr, vc)) in &ptr_stores {
        if !slot_ok.get(slot).copied().unwrap_or(false) {
            let e = esc_at.entry(*vr).or_default();
            if !e.contains(&Some(*vc)) {
                e.push(Some(*vc));
                grew = true;
            }
        }
    }
    if !grew {
        break;
    }
    }
    // a derivation through a slot that unknown stores may modify is unsound: give up on that root
    let mut poisoned: Vec<VarId> = vec![];
    for (r, o) in &via {
        if !slot_ok.get(&(*r, *o)).copied().unwrap_or(false) {
            poisoned.push(*r);
        }
    }
    let promotable: Vec<VarId> = roots
        .iter()
        .copied()
        .filter(|r| !poisoned.contains(r) && slot_ok.iter().any(|((rr, _), ok)| rr == r && *ok))
        .collect();
    if promotable.is_empty() {
        return false;
    }
    // create vars
    let mut var_of: HashMap<(VarId, i32), VarId> = HashMap::new();
    let mut inits = vec![];
    for r in &promotable {
        for (o, m) in &slots[r] {
            if !slot_ok[&(*r, *o)] {
                continue;
            }
            let (ty, range) = match m {
                Mem::Bool => (Ty::Bool, None),
                Mem::F32 | Mem::F64 => (Ty::Num, None),
                Mem::U8 => (Ty::Num, Some((0.0, 255.0))),
                Mem::I8 => (Ty::Num, Some((-128.0, 127.0))),
                Mem::U16 => (Ty::Num, Some((0.0, 65535.0))),
                Mem::I16 => (Ty::Num, Some((-32768.0, 32767.0))),
                Mem::U32 => (Ty::Num, Some((0.0, 4294967295.0))),
                Mem::I32 => (Ty::Num, Some((-2147483648.0, 2147483647.0))),
            };
            f.vars.push(VarInfo { ty, name: None, range });
            let v = (f.vars.len() - 1) as VarId;
            var_of.insert((*r, *o), v);
            inits.push(Stmt::Assign(v, Rv::Use(if ty == Ty::Bool { Operand::Bool(false) } else { Operand::Num(0.0) })));
        }
    }
    let is_prom = |r: VarId| promotable.contains(&r);
    for b in &mut f.blocks {
        for s in &mut b.stmts {
            match s {
                Stmt::Assign(v, Rv::Load { addr: Operand::Var(a), off, .. }) => {
                    if let Some((r, c)) = root_of[*a as usize] {
                        if let Some(pv) = var_of.get(&(r, (c + *off as i64) as i32)).filter(|_| is_prom(r)) {
                            *s = Stmt::Assign(*v, Rv::Use(Operand::Var(*pv)));
                        }
                    }
                }
                Stmt::Store { addr: Operand::Var(a), off, val, .. } => {
                    if let Some((r, c)) = root_of[*a as usize] {
                        if let Some(pv) = var_of.get(&(r, (c + *off as i64) as i32)).filter(|_| is_prom(r)) {
                            *s = Stmt::Assign(*pv, Rv::Use(*val));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let entry = f.entry as usize;
    let mut new = inits;
    new.extend(std::mem::take(&mut f.blocks[entry].stmts));
    f.blocks[entry].stmts = new;
    true
}
