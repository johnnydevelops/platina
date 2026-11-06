//! Structured LIR -> Luau source text.
//!
//! Single-use temporaries are folded into the expression that consumes them, single-block
//! temporaries become `local`s at their definition, and everything else is declared once at
//! the top of the function.

use std::collections::HashMap;
use std::fmt::Write;

use crate::ir::*;
use crate::structure::{Cond, S, Structured};

#[derive(Clone, Debug)]
pub enum E {
    Num(f64),
    Bool(bool),
    Name(String),
    Bin(&'static str, u8, Box<E>, Box<E>),
    Un(&'static str, Box<E>),
    Call(String, Vec<E>),
    IfElse(Box<E>, Box<E>, Box<E>),
}

// Luau precedence (higher binds tighter)
const P_OR: u8 = 1;
const P_AND: u8 = 2;
const P_CMP: u8 = 3;
const P_ADD: u8 = 5;
const P_MUL: u8 = 6;
const P_UN: u8 = 7;
const P_POW: u8 = 8;
const P_ATOM: u8 = 10;

pub fn fmt_num(n: f64) -> String {
    if n.is_nan() {
        "(0/0)".into()
    } else if n.is_infinite() {
        if n > 0.0 { "math.huge".into() } else { "-math.huge".into() }
    } else if n == 0.0 && n.is_sign_negative() {
        "-0.0".into()
    } else if n.fract() == 0.0 && n.abs() < 1e16 {
        if n >= 65536.0 && (n as u64).is_power_of_two() || n >= 0xFFFF as f64 && ((n as u64) + 1).is_power_of_two() {
            format!("0x{:X}", n as u64)
        } else {
            format!("{}", n as i64)
        }
    } else {
        format!("{n:?}")
    }
}

impl E {
    fn prec(&self) -> u8 {
        match self {
            E::Num(n) if *n < 0.0 || (*n == 0.0 && n.is_sign_negative()) => P_UN,
            E::Num(n) if !n.is_finite() => P_UN,
            E::Num(_) | E::Bool(_) | E::Name(_) | E::Call(..) => P_ATOM,
            E::Bin(_, p, ..) => *p,
            E::Un(..) => P_UN,
            E::IfElse(..) => 0,
        }
    }
    pub fn write(&self, out: &mut String) {
        match self {
            E::Num(n) => out.push_str(&fmt_num(*n)),
            E::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            E::Name(s) => out.push_str(s),
            E::Bin(op, p, a, b) => {
                let right_assoc = *p == P_POW;
                let (pa, pb) = if right_assoc { (*p + 1, *p) } else { (*p, *p + 1) };
                a.write_prec(out, pa);
                out.push(' ');
                out.push_str(op);
                out.push(' ');
                b.write_prec(out, pb);
            }
            E::Un(op, a) => {
                out.push_str(op);
                if op.chars().all(|c| c.is_alphabetic()) {
                    out.push(' ');
                }
                // avoid `--x` comment
                if *op == "-" && matches!(**a, E::Un("-", _) | E::Num(_)) && a.prec() == P_UN {
                    out.push('(');
                    a.write(out);
                    out.push(')');
                } else {
                    a.write_prec(out, P_UN);
                }
            }
            E::Call(f, args) => {
                out.push_str(f);
                out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    a.write(out);
                }
                out.push(')');
            }
            E::IfElse(c, a, b) => {
                out.push_str("if ");
                c.write(out);
                out.push_str(" then ");
                a.write(out);
                out.push_str(" else ");
                b.write(out);
            }
        }
    }
    fn write_prec(&self, out: &mut String, min: u8) {
        if self.prec() < min {
            out.push('(');
            self.write(out);
            out.push(')');
        } else {
            self.write(out);
        }
    }
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }
}

fn is_simple(e: &E) -> bool {
    matches!(e, E::Name(_) | E::Num(_))
}

/// `(if a*b < 2^53 then bit32.bor(a*b, 0) else <16-bit split>)`: the product is exact (and
/// `bor` reduces it mod 2^32) unless it reaches 2^53; only the taken branch is evaluated.
fn imul32_inline(a: E, b: E) -> E {
    let n = E::Num;
    let prod = bin("*", P_MUL, a.clone(), b.clone());
    let fast = call("bit32.bor", vec![prod.clone(), n(0.0)]);
    let lo = bin("*", P_MUL, call("bit32.band", vec![a.clone(), n(65535.0)]), b.clone());
    let mid = call(
        "bit32.band",
        vec![bin("*", P_MUL, call("bit32.rshift", vec![a, n(16.0)]), call("bit32.band", vec![b, n(65535.0)])), n(65535.0)],
    );
    let slow = call("bit32.bor", vec![bin("+", P_ADD, lo, bin("*", P_MUL, mid, n(65536.0))), n(0.0)]);
    E::IfElse(Box::new(bin("<", P_CMP, prod, n(9007199254740992.0))), Box::new(fast), Box::new(slow))
}

fn bin(op: &'static str, p: u8, a: E, b: E) -> E {
    E::Bin(op, p, Box::new(a), Box::new(b))
}
fn call(f: &str, args: Vec<E>) -> E {
    E::Call(f.to_string(), args)
}

pub fn mem_suffix(m: Mem) -> &'static str {
    match m {
        Mem::U8 | Mem::Bool => "u8",
        Mem::I8 => "i8",
        Mem::U16 => "u16",
        Mem::I16 => "i16",
        Mem::U32 => "u32",
        Mem::I32 => "i32",
        Mem::F32 => "f32",
        Mem::F64 => "f64",
    }
}

/// Wrap expression `a` into the canonical range of `k`.
pub fn wrap_expr(k: IntKind, a: E) -> E {
    if k.bits == 32 && !k.signed {
        return call("bit32.bor", vec![a, E::Num(0.0)]);
    }
    let m = E::Num(k.modulus());
    if k.signed {
        let h = E::Num(k.modulus() / 2.0);
        bin("-", P_ADD, bin("%", P_MUL, bin("+", P_ADD, a, h.clone()), m), h)
    } else {
        bin("%", P_MUL, a, m)
    }
}

/// Callback for naming functions.
pub trait Names {
    fn func(&self, id: u32) -> String;
}

struct VarUse {
    defs: u32,
    uses: u32,
    def_block: Option<BlockId>,
    use_blocks_same: bool,
    first_use_block: Option<BlockId>,
}

pub struct FnEmitter<'a> {
    f: &'a Function,
    names: &'a dyn Names,
    out: String,
    indent: usize,
    /// vars declared at function top
    top_decl: Vec<bool>,
    /// vars that may be folded into their single use
    foldable: Vec<bool>,
    pending: HashMap<VarId, E>,
    wrapnear_defs: std::collections::HashSet<VarId>,
    /// register color of function-level vars (shared Luau locals)
    color: Vec<Option<u32>>,
    ncolors: Vec<Ty>,
    /// nesting depth of emitted numeric `for` loops (names their control variables)
    for_depth: usize,
}

impl<'a> FnEmitter<'a> {
    pub fn new(f: &'a Function, names: &'a dyn Names) -> Self {
        FnEmitter {
            f,
            names,
            out: String::new(),
            indent: 1,
            top_decl: vec![],
            foldable: vec![],
            pending: HashMap::new(),
            color: vec![],
            ncolors: vec![],
            for_depth: 0,
            wrapnear_defs: f
                .blocks
                .iter()
                .flat_map(|b| b.stmts.iter())
                .filter_map(|s| match s {
                    Stmt::Assign(v, Rv::Un(Un::WrapNear(..), _)) => Some(*v),
                    _ => None,
                })
                .collect(),
        }
    }

    fn vname(&self, v: VarId) -> String {
        if let Some(Some(c)) = self.color.get(v as usize) {
            return format!("r{c}");
        }
        match &self.f.vars[v as usize].name {
            Some(n) => format!("{n}_{v}"),
            None => format!("v{v}"),
        }
    }

    fn analyze(&mut self) {
        let n = self.f.vars.len();
        let mut info: Vec<VarUse> = (0..n)
            .map(|_| VarUse { defs: 0, uses: 0, def_block: None, use_blocks_same: true, first_use_block: None })
            .collect();
        let note_use = |info: &mut Vec<VarUse>, o: &Operand, b: BlockId, k: u32| {
            if let Operand::Var(v) = o {
                let i = &mut info[*v as usize];
                i.uses += k;
                match i.first_use_block {
                    None => i.first_use_block = Some(b),
                    Some(fb) if fb != b => i.use_blocks_same = false,
                    _ => {}
                }
            }
        };
        for (bi, blk) in self.f.blocks.iter().enumerate() {
            let b = bi as BlockId;
            for s in &blk.stmts {
                for o in s.operands() {
                    note_use(&mut info, o, b, 1);
                }
                for d in s.def() {
                    let i = &mut info[d as usize];
                    i.defs += 1;
                    i.def_block = Some(b);
                }
            }
            match &blk.term {
                Term::Switch(o, cases, _) => note_use(&mut info, o, b, cases.len().max(1) as u32 + 1),
                t => {
                    let mut t = t.clone();
                    for o in t.operands_mut() {
                        note_use(&mut info, o, b, 1);
                    }
                }
            }
        }
        for p in &self.f.params {
            info[*p as usize].defs += 100; // never local/foldable
        }
        self.top_decl = vec![false; n];
        self.foldable = vec![false; n];
        for v in 0..n {
            let i = &info[v];
            let single_block = i.defs == 1 && i.use_blocks_same && (i.first_use_block.is_none() || i.first_use_block == i.def_block);
            if self.f.params.contains(&(v as VarId)) {
                continue;
            }
            if i.defs == 0 {
                // used but never defined (shouldn't happen) -> declare
                self.top_decl[v] = i.uses > 0;
                continue;
            }
            if single_block {
                self.foldable[v] = i.uses == 1 && !self.wrapnear_defs.contains(&(v as VarId));
            } else {
                self.top_decl[v] = true;
            }
        }
        // Large functions: block-scoped `local`s would pile up past Luau's 200-local limit, so
        // route every non-folded var through the colored function-level registers instead.
        let block_locals = (0..n).filter(|v| !self.top_decl[*v] && !self.foldable[*v] && info[*v].defs > 0 && !self.f.params.contains(&(*v as VarId))).count();
        let top = self.top_decl.iter().filter(|t| **t).count();
        if block_locals + top > 120 {
            // foldable vars get a register too: used only if they must be materialized early
            for v in 0..n {
                if info[v].defs > 0 && !self.f.params.contains(&(v as VarId)) {
                    self.top_decl[v] = true;
                }
            }
        }
        // Multi-result calls declare all dests together: either all `local` or all top-level.
        for blk in &self.f.blocks {
            for s in &blk.stmts {
                if let Stmt::Call { dests, .. } = s {
                    if dests.iter().any(|d| self.top_decl[*d as usize]) {
                        for d in dests {
                            self.top_decl[*d as usize] = true;
                        }
                    }
                }
            }
        }
    }

    /// Liveness-based coloring of function-level vars so that vars with disjoint lifetimes
    /// share one Luau local (Luau allows at most 200 active locals).
    fn color_vars(&mut self) {
        let f = self.f;
        let n = f.vars.len();
        let tracked: Vec<bool> = (0..n).map(|v| self.top_decl[v]).collect();
        let nb = f.blocks.len();
        let uses_of = |b: &Block| -> (Vec<VarId>, Vec<VarId>) {
            // (upward-exposed uses, defs)
            let mut ue = vec![];
            let mut defs: Vec<VarId> = vec![];
            for s in &b.stmts {
                for o in s.operands() {
                    if let Operand::Var(v) = o {
                        if tracked[*v as usize] && !defs.contains(v) && !ue.contains(v) {
                            ue.push(*v);
                        }
                    }
                }
                for d in s.def() {
                    if tracked[d as usize] {
                        defs.push(d);
                    }
                }
            }
            let mut t = b.term.clone();
            for o in t.operands_mut() {
                if let Operand::Var(v) = o {
                    if tracked[*v as usize] && !defs.contains(v) && !ue.contains(v) {
                        ue.push(*v);
                    }
                }
            }
            (ue, defs)
        };
        let info: Vec<(Vec<VarId>, Vec<VarId>)> = f.blocks.iter().map(uses_of).collect();
        let mut live_in: Vec<std::collections::BTreeSet<VarId>> = vec![Default::default(); nb];
        let mut live_out: Vec<std::collections::BTreeSet<VarId>> = vec![Default::default(); nb];
        let mut changed = true;
        while changed {
            changed = false;
            for b in (0..nb).rev() {
                let mut out = std::collections::BTreeSet::new();
                for s in f.blocks[b].term.successors() {
                    out.extend(live_in[s as usize].iter().copied());
                }
                let mut inn: std::collections::BTreeSet<VarId> = out.clone();
                for d in &info[b].1 {
                    inn.remove(d);
                }
                inn.extend(info[b].0.iter().copied());
                if inn != live_in[b] || out != live_out[b] {
                    live_in[b] = inn;
                    live_out[b] = out;
                    changed = true;
                }
            }
        }
        // interference
        let mut adj: Vec<std::collections::BTreeSet<VarId>> = vec![Default::default(); n];
        let add = |a: VarId, b: VarId, adj: &mut Vec<std::collections::BTreeSet<VarId>>| {
            if a != b {
                adj[a as usize].insert(b);
                adj[b as usize].insert(a);
            }
        };
        for b in 0..nb {
            let mut live = live_out[b].clone();
            let mut t = f.blocks[b].term.clone();
            for o in t.operands_mut() {
                if let Operand::Var(v) = o {
                    if tracked[*v as usize] {
                        live.insert(*v);
                    }
                }
            }
            for s in f.blocks[b].stmts.iter().rev() {
                let defs: Vec<VarId> = s.def().into_iter().filter(|d| tracked[*d as usize]).collect();
                for d in &defs {
                    for l in &live {
                        add(*d, *l, &mut adj);
                    }
                    for d2 in &defs {
                        add(*d, *d2, &mut adj);
                    }
                }
                for d in &defs {
                    live.remove(d);
                }
                for o in s.operands() {
                    if let Operand::Var(v) = o {
                        if tracked[*v as usize] {
                            live.insert(*v);
                        }
                    }
                }
            }
            // everything live at block entry interferes pairwise (conservative)
            let l: Vec<VarId> = live.iter().copied().collect();
            for i in 0..l.len() {
                for j in i + 1..l.len() {
                    add(l[i], l[j], &mut adj);
                }
            }
        }
        // greedy coloring, most-constrained first
        let mut order: Vec<VarId> = (0..n as u32).filter(|v| tracked[*v as usize]).collect();
        order.sort_by_key(|v| std::cmp::Reverse(adj[*v as usize].len()));
        let mut color: Vec<Option<u32>> = vec![None; n];
        let mut ctys: Vec<Ty> = vec![];
        for v in order {
            let ty = f.vars[v as usize].ty;
            let taken: std::collections::HashSet<u32> = adj[v as usize].iter().filter_map(|u| color[*u as usize]).collect();
            let c = (0..ctys.len() as u32).find(|c| ctys[*c as usize] == ty && !taken.contains(c)).unwrap_or_else(|| {
                ctys.push(ty);
                (ctys.len() - 1) as u32
            });
            color[v as usize] = Some(c);
        }
        self.color = color;
        self.ncolors = ctys;
    }

    fn operand(&mut self, o: &Operand) -> E {
        match o {
            Operand::Num(n) => E::Num(*n),
            Operand::Bool(b) => E::Bool(*b),
            Operand::Var(v) => {
                if let Some(e) = self.pending.remove(v) {
                    e
                } else {
                    E::Name(self.vname(*v))
                }
            }
        }
    }

    fn addr(&mut self, addr: &Operand, off: i32) -> E {
        let a = self.operand(addr);
        if off == 0 {
            a
        } else if let E::Num(n) = a {
            E::Num(n + off as f64)
        } else {
            bin("+", P_ADD, a, E::Num(off as f64))
        }
    }

    fn rv(&mut self, rv: &Rv) -> E {
        match rv {
            Rv::Use(o) => self.operand(o),
            Rv::Bin(op, a, b) => {
                let a = self.operand(a);
                let b = self.operand(b);
                match op {
                    Bin::Add => bin("+", P_ADD, a, b),
                    Bin::Sub => bin("-", P_ADD, a, b),
                    Bin::Mul => bin("*", P_MUL, a, b),
                    Bin::Div => bin("/", P_MUL, a, b),
                    Bin::FloorDiv => bin("//", P_MUL, a, b),
                    Bin::Mod => bin("%", P_MUL, a, b),
                    Bin::Pow => bin("^", P_POW, a, b),
                    Bin::FMod => call("math.fmod", vec![a, b]),
                    Bin::Eq => bin("==", P_CMP, a, b),
                    Bin::Ne => bin("~=", P_CMP, a, b),
                    Bin::Lt => bin("<", P_CMP, a, b),
                    Bin::Le => bin("<=", P_CMP, a, b),
                    Bin::And => bin("and", P_AND, a, b),
                    Bin::Or => bin("or", P_OR, a, b),
                    Bin::BAnd => call("bit32.band", vec![a, b]),
                    Bin::BOr => call("bit32.bor", vec![a, b]),
                    Bin::BXor => call("bit32.bxor", vec![a, b]),
                    Bin::Shl => call("bit32.lshift", vec![a, b]),
                    Bin::Shr => call("bit32.rshift", vec![a, b]),
                    Bin::Sar => call("bit32.arshift", vec![a, b]),
                    Bin::Rol => call("bit32.lrotate", vec![a, b]),
                    Bin::Ror => call("bit32.rrotate", vec![a, b]),
                    Bin::IMul32 if is_simple(&a) && is_simple(&b) => imul32_inline(a, b),
                    Bin::IMul32 => call("imul32f", vec![a, b]),
                    Bin::IDivTrunc => call("idivt", vec![a, b]),
                    Bin::Min => call("math.min", vec![a, b]),
                    Bin::Max => call("math.max", vec![a, b]),
                }
            }
            Rv::Un(op, a) => {
                let a = self.operand(a);
                match op {
                    Un::Neg => E::Un("-", Box::new(a)),
                    Un::Not => E::Un("not", Box::new(a)),
                    Un::BNot => call("bit32.bnot", vec![a]),
                    Un::Wrap(k) => wrap_expr(*k, a),
                    // only reached when the value had to be folded into an expression
                    Un::WrapNear(k, ..) => wrap_expr(*k, a),
                    Un::F32Round => call("f32", vec![a]),
                    Un::Floor => call("math.floor", vec![a]),
                    Un::Ceil => call("math.ceil", vec![a]),
                    Un::Trunc => call("trunc", vec![a]),
                    Un::Sqrt => call("math.sqrt", vec![a]),
                    Un::Abs => call("math.abs", vec![a]),
                    Un::Clz => call("bit32.countlz", vec![a]),
                    Un::Ctz => call("bit32.countrz", vec![a]),
                    Un::Popcnt => call("popcnt", vec![a]),
                    Un::BSwap => call("bit32.byteswap", vec![a]),
                    Un::BoolToNum => E::IfElse(Box::new(a), Box::new(E::Num(1.0)), Box::new(E::Num(0.0))),
                    Un::NumToBool => bin("~=", P_CMP, a, E::Num(0.0)),
                    Un::FToI(k) => call("ftoi", vec![a, E::Num(k.min()), E::Num(k.max())]),
                }
            }
            Rv::Load { mem, addr, off } => {
                let a = self.addr(addr, *off);
                let e = call(&format!("buffer.read{}", mem_suffix(*mem)), vec![E::Name("MEM".into()), a]);
                if *mem == Mem::Bool { bin("~=", P_CMP, e, E::Num(0.0)) } else { e }
            }
            Rv::Builtin(name, args) => {
                let args = args.iter().map(|a| self.operand(a)).collect();
                call(name, args)
            }
            Rv::Select(c, a, b) => {
                let c = self.operand(c);
                let a = self.operand(a);
                let b = self.operand(b);
                E::IfElse(Box::new(c), Box::new(a), Box::new(b))
            }
        }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push('\t');
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn assign_lhs(&self, vars: &[VarId]) -> String {
        let local = vars.iter().all(|v| !self.top_decl[*v as usize]);
        let names: Vec<String> = vars.iter().map(|v| self.vname(*v)).collect();
        if local { format!("local {}", names.join(", ")) } else { names.join(", ") }
    }

    fn callee(&mut self, c: &Callee) -> String {
        match c {
            Callee::Func(i) => self.names.func(*i),
            Callee::Extern(s) => s.clone(),
            Callee::Indirect(o) => {
                let e = self.operand(o);
                format!("FN[{}]", e.to_string())
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Assign(v, Rv::Un(Un::WrapNear(k, above, below), x)) => {
                let e = self.operand(x);
                let lhs = self.assign_lhs(&[*v]);
                let name = self.vname(*v);
                self.line(&format!("{lhs} = {}", e.to_string()));
                let m = fmt_num(k.modulus());
                if *above {
                    self.line(&format!("if {name} > {} then {name} -= {m} end", fmt_num(k.max())));
                }
                if *below {
                    self.line(&format!("if {name} < {} then {name} += {m} end", fmt_num(k.min())));
                }
            }
            Stmt::Assign(v, rv) => {
                let e = self.rv(rv);
                if self.foldable[*v as usize] {
                    self.pending.insert(*v, e);
                } else {
                    let lhs = self.assign_lhs(&[*v]);
                    self.line(&format!("{lhs} = {}", e.to_string()));
                }
            }
            Stmt::Store { mem, addr, off, val } => {
                let a = self.addr(addr, *off);
                let mut v = self.operand(val);
                if *mem == Mem::Bool {
                    v = match v {
                        E::Bool(b) => E::Num(if b { 1.0 } else { 0.0 }),
                        v => E::IfElse(Box::new(v), Box::new(E::Num(1.0)), Box::new(E::Num(0.0))),
                    };
                }
                let e = call(&format!("buffer.write{}", mem_suffix(*mem)), vec![E::Name("MEM".into()), a, v]);
                self.line(&e.to_string());
            }
            Stmt::Call { dests, callee, args } => {
                let args: Vec<E> = args.iter().map(|a| self.operand(a)).collect();
                let f = self.callee(callee);
                let e = E::Call(f, args).to_string();
                if dests.is_empty() {
                    self.line(&e);
                } else {
                    // results are never folded (calls have effects)
                    let lhs = self.assign_lhs(dests);
                    self.line(&format!("{lhs} = {e}"));
                }
            }
            Stmt::Copy { dst, src, len } => {
                let d = self.operand(dst).to_string();
                let s = self.operand(src).to_string();
                let l = self.operand(len).to_string();
                self.line(&format!("buffer.copy(MEM, {d}, MEM, {s}, {l})"));
            }
            Stmt::Fill { dst, val, len } => {
                let d = self.operand(dst).to_string();
                let v = self.operand(val).to_string();
                let l = self.operand(len).to_string();
                self.line(&format!("buffer.fill(MEM, {d}, {v}, {l})"));
            }
            Stmt::SetSp(o) => {
                let e = self.operand(o).to_string();
                self.line(&format!("SP = {e}"));
            }
            Stmt::GetSp(v) => {
                let lhs = self.assign_lhs(&[*v]);
                self.line(&format!("{lhs} = SP"));
            }
        }
    }

    /// Before emitting a statement with effects, folded loads that are still pending must not
    /// move across it. We flush pending expressions that read memory by materializing them.
    fn flush_pending_before(&mut self, s: &Stmt) {
        if self.pending.is_empty() {
            return;
        }
        let used: Vec<VarId> = s.operands().iter().filter_map(|o| o.as_var()).collect();
        let defs = s.def();
        let effects = s.has_side_effects();
        let keys: Vec<VarId> = self.pending.keys().copied().collect();
        for k in keys {
            if used.contains(&k) {
                continue;
            }
            let e = &self.pending[&k];
            let reads_mem = expr_reads_mem(e);
            let reads_def = defs.iter().any(|d| expr_reads_name(e, &self.vname(*d))) || expr_reads_name(e, "SP");
            if (effects && reads_mem) || reads_def {
                let e = self.pending.remove(&k).unwrap();
                let lhs = self.assign_lhs(&[k]);
                self.line(&format!("{lhs} = {}", e.to_string()));
            }
        }
    }

    fn cond(&mut self, c: &Cond) -> E {
        match c {
            Cond::Truthy(o) => self.operand(o),
            Cond::Falsy(o) => {
                let e = self.operand(o);
                negate(e)
            }
            Cond::EqAny(o, vals) => {
                let e = self.operand(o);
                let mut acc: Option<E> = None;
                for v in vals {
                    let t = bin("==", P_CMP, e.clone(), E::Num(*v));
                    acc = Some(match acc {
                        None => t,
                        Some(a) => bin("or", P_OR, a, t),
                    });
                }
                acc.unwrap()
            }
            Cond::NotEqAny(o, vals) => {
                let e = self.operand(o);
                let mut acc: Option<E> = None;
                for v in vals {
                    let t = bin("~=", P_CMP, e.clone(), E::Num(*v));
                    acc = Some(match acc {
                        None => t,
                        Some(a) => bin("and", P_AND, a, t),
                    });
                }
                acc.unwrap()
            }
            Cond::Label(id) => bin("==", P_CMP, E::Name("lbl".into()), E::Num(*id as f64)),
            Cond::NotLabel(id) => bin("~=", P_CMP, E::Name("lbl".into()), E::Num(*id as f64)),
            Cond::LabelSet => bin("~=", P_CMP, E::Name("lbl".into()), E::Num(0.0)),
            Cond::LabelUnset => bin("==", P_CMP, E::Name("lbl".into()), E::Num(0.0)),
        }
    }

    fn seq(&mut self, ss: &[S]) {
        for (i, s) in ss.iter().enumerate() {
            let last = i + 1 == ss.len();
            self.s(s, last);
        }
    }

    /// Counted loop -> numeric `for`:
    /// `while true do [hdr] if not (i < n) then break end; v = i; i = i + 1 [wrap]; body end`
    /// becomes `for fK = i, n - 1 do v = fK; body end` when `n` is loop-invariant and `i` is
    /// used nowhere else (it is the loop counter only). `i + 1` cannot wrap under `i < n`.
    fn try_for(&mut self, body: &[S]) -> bool {
        if self.ncolors.len() > 150 {
            return false; // keep clear of Luau's register limit
        }
        let ncode = body.iter().take_while(|s| matches!(s, S::Code(_))).count();
        let (hdr, rest) = body.split_at(ncode);
        let (Some(S::If(c, a, e)), Some(S::Code(b0))) = (rest.first(), rest.get(1)) else { return false };
        if !(matches!(a.as_slice(), [S::Break]) && e.is_empty()) {
            return false;
        }
        let b0 = *b0;
        // b0: v = i; i = i + 1 | t = i + 1; i = t ; optional wrap of i
        let st = &self.f.blocks[b0 as usize].stmts;
        let Some(Stmt::Assign(v, Rv::Use(Operand::Var(i)))) = st.first() else { return false };
        let (v, i) = (*v, *i);
        if v == i {
            return false;
        }
        let mut k = 1;
        let mut tmp = None;
        match st.get(1) {
            Some(Stmt::Assign(d, Rv::Bin(Bin::Add, Operand::Var(x), Operand::Num(one)))) if *x == i && *one == 1.0 => {
                if *d == i {
                    k = 2;
                } else {
                    match st.get(2) {
                        Some(Stmt::Assign(d2, Rv::Use(Operand::Var(y)) | Rv::Un(Un::Wrap(_) | Un::WrapNear(..), Operand::Var(y))))
                            if *d2 == i && y == d =>
                        {
                            tmp = Some(*d);
                            k = 3;
                        }
                        _ => return false,
                    }
                }
            }
            _ => return false,
        }
        if k == 2 {
            if let Some(Stmt::Assign(d2, Rv::Un(Un::Wrap(_) | Un::WrapNear(..), Operand::Var(y)))) = st.get(2) {
                if *d2 == i && *y == i {
                    k = 3;
                }
            }
        }
        // the loop's blocks
        fn blocks(ss: &[S], out: &mut Vec<BlockId>) {
            for s in ss {
                match s {
                    S::Code(b) => out.push(*b),
                    S::If(_, a, b) => {
                        blocks(a, out);
                        blocks(b, out);
                    }
                    S::Loop { body, .. } => blocks(body, out),
                    _ => {}
                }
            }
        }
        let mut lb = vec![];
        blocks(body, &mut lb);
        // i: used only by the pattern (and the condition); tmp: only by the pattern
        let count_uses = |var: VarId| -> usize {
            let mut n = 0;
            for b in &self.f.blocks {
                for s in &b.stmts {
                    n += s.operands().iter().filter(|o| ***o == Operand::Var(var)).count();
                }
                let mut t = b.term.clone();
                n += t.operands_mut().iter().filter(|o| ***o == Operand::Var(var)).count();
            }
            n
        };
        // pattern uses of i: s0, the add, and (if k == 3 with in-place wrap) the wrap
        let pat_uses = 2 + usize::from(k == 3 && tmp.is_none());
        let mut cond_var_uses = 0;
        for &b in &lb {
            if b == b0 {
                continue;
            }
            for s in &self.f.blocks[b as usize].stmts {
                if s.def().contains(&i) {
                    return false;
                }
                cond_var_uses += s.operands().iter().filter(|o| ***o == Operand::Var(i)).count();
            }
        }
        for s in &self.f.blocks[b0 as usize].stmts[k..] {
            if s.def().contains(&i) || s.operands().iter().any(|o| **o == Operand::Var(i)) {
                return false;
            }
        }
        if cond_var_uses != 1 || count_uses(i) != pat_uses + 1 {
            return false;
        }
        if let Some(t) = tmp {
            if count_uses(t) != 1 {
                return false;
            }
        }
        // the condition must be exactly `i < n`, with n a constant or a loop-invariant var
        let mut lim: Option<Operand> = None;
        for s in hdr {
            let S::Code(b) = s else { unreachable!() };
            for st in &self.f.blocks[*b as usize].stmts {
                if let Stmt::Assign(_, Rv::Bin(Bin::Lt, Operand::Var(x), n)) = st {
                    if *x == i {
                        lim = Some(*n);
                    }
                }
            }
        }
        let Some(lim) = lim else { return false };
        if let Operand::Var(nv) = lim {
            if lb.iter().any(|&b| self.f.blocks[b as usize].stmts.iter().any(|s| s.def().contains(&nv))) {
                return false;
            }
        }
        // header code must fold entirely into the condition (as in try_while)
        let saved_out = std::mem::take(&mut self.out);
        let saved_pending = self.pending.clone();
        for s in hdr {
            let S::Code(b) = s else { unreachable!() };
            for st in &self.f.blocks[*b as usize].stmts {
                self.flush_pending_before(st);
                self.stmt(st);
            }
        }
        let header_ok = self.out.is_empty();
        let cond = if header_ok { Some(negate(self.cond(c))) } else { None };
        let iname = self.vname(i);
        let shape_ok = match &cond {
            Some(E::Bin("<", _, x, n)) => matches!(&**x, E::Name(s) if *s == iname) && matches!(&**n, E::Name(_) | E::Num(_)),
            _ => false,
        };
        if !(header_ok && shape_ok) {
            self.out = saved_out;
            self.pending = saved_pending;
            return false;
        }
        self.out = saved_out;
        let Some(E::Bin(_, _, _, n)) = cond else { unreachable!() };
        let limit = bin("-", P_ADD, *n, E::Num(1.0)).to_string();
        let fv = format!("fi{}", self.for_depth);
        self.line(&format!("for {fv} = {iname}, {limit} do"));
        self.indent += 1;
        self.for_depth += 1;
        // v = fK, then the rest of b0 without the increment
        self.pending.insert(i, E::Name(fv));
        let s0 = self.f.blocks[b0 as usize].stmts[0].clone();
        self.stmt(&s0);
        self.pending.remove(&i);
        let blk = &self.f.blocks[b0 as usize];
        for st in &blk.stmts[k..] {
            self.flush_pending_before(st);
            self.stmt(st);
        }
        self.seq(&rest[2..]);
        self.for_depth -= 1;
        self.indent -= 1;
        self.line("end");
        true
    }

    /// `while true do [code] if c then break end ... end` -> `while not c do ... end` when the
    /// header code folds entirely into the condition.
    fn try_while(&mut self, body: &[S]) -> bool {
        let ncode = body.iter().take_while(|s| matches!(s, S::Code(_))).count();
        let (code, rest) = body.split_at(ncode);
        let Some(S::If(c, a, e)) = rest.first() else { return false };
        if !(matches!(a.as_slice(), [S::Break]) && e.is_empty()) {
            return false;
        }
        // emit header code into a scratch buffer; it must produce no statements
        let saved_out = std::mem::take(&mut self.out);
        let saved_pending = self.pending.clone();
        for s in code {
            let S::Code(b) = s else { unreachable!() };
            let blk = &self.f.blocks[*b as usize];
            for st in &blk.stmts {
                self.flush_pending_before(st);
                self.stmt(st);
            }
        }
        if !self.out.is_empty() {
            self.out = saved_out;
            self.pending = saved_pending;
            return false;
        }
        self.out = saved_out;
        let cond = negate(self.cond(c)).to_string();
        self.line(&format!("while {cond} do"));
        self.indent += 1;
        self.seq(&rest[1..]);
        self.indent -= 1;
        self.line("end");
        true
    }

    /// Compact trap forms: `oob(len, idx, file, line, col)` for bounds checks and
    /// `trap(msg, file, line, col)` for constant messages with a location.
    fn compact_trap(&mut self, parts: &[TrapPart]) -> bool {
        fn split_loc(s: &str) -> Option<(&str, &str, u32, u32)> {
            let at = s.rfind(" at ")?;
            let (msg, loc) = (&s[..at], &s[at + 4..]);
            let mut it = loc.rsplitn(3, ':');
            let col: u32 = it.next()?.parse().ok()?;
            let line: u32 = it.next()?.parse().ok()?;
            let file = it.next()?;
            Some((msg, file, line, col))
        }
        // merge adjacent constant parts
        let mut merged: Vec<TrapPart> = vec![];
        for p in parts {
            match (merged.last_mut(), p) {
                (Some(TrapPart::Str(a)), TrapPart::Str(b)) => a.push_str(b),
                _ => merged.push(p.clone()),
            }
        }
        match merged.as_slice() {
            [TrapPart::Str(a), TrapPart::Val(len), TrapPart::Str(b), TrapPart::Val(idx), TrapPart::Str(c)]
                if a == "index out of bounds: the len is " && b == " but the index is " =>
            {
                let Some(("", file, line, col)) = split_loc(c) else { return false };
                let (len, idx) = (self.operand(len).to_string(), self.operand(idx).to_string());
                self.line(&format!("oob({len}, {idx}, {}, {line}, {col})", lua_string(file.as_bytes())));
                true
            }
            [TrapPart::Str(m)] => match split_loc(m) {
                Some((msg, file, line, col)) => {
                    self.line(&format!("trap({}, {}, {line}, {col})", lua_string(msg.as_bytes()), lua_string(file.as_bytes())));
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    fn jump(&mut self, text: &str, last: bool) {
        if last {
            self.line(text);
        } else {
            self.line(&format!("do {text} end"));
        }
    }

    fn s(&mut self, s: &S, last: bool) {
        match s {
            S::Code(b) => {
                let blk = &self.f.blocks[*b as usize];
                for st in &blk.stmts {
                    self.flush_pending_before(st);
                    self.stmt(st);
                }
            }
            S::If(c, a, b) => {
                let c = self.cond(c).to_string();
                self.line(&format!("if {c} then"));
                self.indent += 1;
                self.seq(a);
                self.indent -= 1;
                let mut els = b.as_slice();
                loop {
                    if els.is_empty() {
                        break;
                    }
                    if let [S::If(c2, a2, b2)] = els {
                        let c2 = self.cond(c2).to_string();
                        self.line(&format!("elseif {c2} then"));
                        self.indent += 1;
                        self.seq(a2);
                        self.indent -= 1;
                        els = b2.as_slice();
                    } else {
                        self.line("else");
                        self.indent += 1;
                        self.seq(els);
                        self.indent -= 1;
                        break;
                    }
                }
                self.line("end");
            }
            S::Loop { once: false, body } if self.try_for(body) => {}
            S::Loop { once: false, body } if self.try_while(body) => {}
            S::Loop { once, body } => {
                self.line(if *once { "repeat" } else { "while true do" });
                self.indent += 1;
                self.seq(body);
                self.indent -= 1;
                self.line(if *once { "until true" } else { "end" });
            }
            S::Break => self.jump("break", last),
            S::Continue => self.jump("continue", last),
            S::SetLabel(id) => self.line(&format!("lbl = {id}")),
            S::Return(v) => {
                let es: Vec<String> = v.iter().map(|o| self.operand(o).to_string()).collect();
                let t = if es.is_empty() { "return".to_string() } else { format!("return {}", es.join(", ")) };
                self.jump(&t, last);
            }
            S::Trap(parts) if self.compact_trap(parts) => {}
            S::Trap(parts) => {
                let mut es = Vec::new();
                for p in parts {
                    es.push(match p {
                        TrapPart::Str(s) => lua_string(s.as_bytes()),
                        TrapPart::Val(o) => format!("tostring({})", self.operand(o).to_string()),
                        TrapPart::MemStr(p, l) => {
                            let p = self.operand(p).to_string();
                            let l = self.operand(l).to_string();
                            format!("buffer.readstring(MEM, {p}, {l})")
                        }
                    });
                }
                let msg = if es.is_empty() { "\"panic\"".to_string() } else { es.join(" .. ") };
                self.line(&format!("error({msg}, 0)"));
            }
            // only reached after diverging calls (empty unreachable blocks are pruned)
            S::Unreachable => {}
        }
    }

    pub fn emit(mut self, st: &Structured, header: &str) -> String {
        self.analyze();
        let params: Vec<String> = self
            .f
            .params
            .iter()
            .map(|p| format!("{}: {}", self.vname(*p), ty_name(self.f.ty(*p))))
            .collect();
        let mut out = format!("{header}({})\n", params.join(", "));
        self.color_vars();
        // initialized so reads of uninitialized Rust bytes (e.g. the payload of a `None`) see a
        // number/boolean rather than nil
        let decls: Vec<(String, &str)> = self
            .ncolors
            .iter()
            .enumerate()
            .map(|(c, t)| (format!("r{c}: {}", ty_name(*t)), if *t == Ty::Bool { "false" } else { "0" }))
            .collect();
        for chunk in decls.chunks(16) {
            let names: Vec<&str> = chunk.iter().map(|d| d.0.as_str()).collect();
            let vals: Vec<&str> = chunk.iter().map(|d| d.1).collect();
            self.line(&format!("local {} = {}", names.join(", "), vals.join(", ")));
        }
        if st.uses_label_var {
            self.line("local lbl = 0");
        }
        self.seq(&st.body);
        out.push_str(&self.out);
        out.push_str("end\n");
        out
    }
}

fn negate(e: E) -> E {
    match e {
        E::Bin("==", p, a, b) => E::Bin("~=", p, a, b),
        E::Bin("~=", p, a, b) => E::Bin("==", p, a, b),
        E::Un("not", a) => *a,
        E::Bool(b) => E::Bool(!b),
        e => E::Un("not", Box::new(e)),
    }
}

fn ty_name(t: Ty) -> &'static str {
    match t {
        Ty::Num => "number",
        Ty::Bool => "boolean",
    }
}

fn expr_reads_mem(e: &E) -> bool {
    match e {
        E::Call(f, args) => f.starts_with("buffer.read") || args.iter().any(expr_reads_mem),
        E::Bin(_, _, a, b) => expr_reads_mem(a) || expr_reads_mem(b),
        E::Un(_, a) => expr_reads_mem(a),
        E::IfElse(a, b, c) => expr_reads_mem(a) || expr_reads_mem(b) || expr_reads_mem(c),
        _ => false,
    }
}

fn expr_reads_name(e: &E, n: &str) -> bool {
    match e {
        E::Name(s) => s == n,
        E::Call(_, args) => args.iter().any(|a| expr_reads_name(a, n)),
        E::Bin(_, _, a, b) => expr_reads_name(a, n) || expr_reads_name(b, n),
        E::Un(_, a) => expr_reads_name(a, n),
        E::IfElse(a, b, c) => expr_reads_name(a, n) || expr_reads_name(b, n) || expr_reads_name(c, n),
        _ => false,
    }
}

pub fn lua_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() + 2);
    s.push('"');
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(b as char),
            _ => {
                // shortest decimal escape that a following digit can't extend
                let next_digit = bytes.get(i + 1).is_some_and(|n| n.is_ascii_digit());
                if next_digit {
                    write!(s, "\\{:03}", b).unwrap();
                } else {
                    write!(s, "\\{}", b).unwrap();
                }
            }
        }
    }
    s.push('"');
    s
}

/// Base64 (standard alphabet, no padding).
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        let k = c.len() + 1;
        for j in 0..k {
            out.push(A[((n >> (18 - 6 * j)) & 63) as usize] as char);
        }
    }
    out
}
