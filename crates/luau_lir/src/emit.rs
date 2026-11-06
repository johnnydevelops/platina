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
}

impl<'a> FnEmitter<'a> {
    pub fn new(f: &'a Function, names: &'a dyn Names) -> Self {
        FnEmitter { f, names, out: String::new(), indent: 1, top_decl: vec![], foldable: vec![], pending: HashMap::new() }
    }

    fn vname(&self, v: VarId) -> String {
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
                self.foldable[v] = i.uses == 1;
            } else {
                self.top_decl[v] = true;
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
                    Bin::IMul32 => call("imul32", vec![a, b]),
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
                    v = E::IfElse(Box::new(v), Box::new(E::Num(1.0)), Box::new(E::Num(0.0)));
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
                self.line(&format!("local {} = {}", self.vname(k), e.to_string()));
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
            Cond::Label(id) => bin("==", P_CMP, E::Name("lbl".into()), E::Num(*id as f64)),
            Cond::LabelSet => bin("~=", P_CMP, E::Name("lbl".into()), E::Num(0.0)),
        }
    }

    fn seq(&mut self, ss: &[S]) {
        for (i, s) in ss.iter().enumerate() {
            let last = i + 1 == ss.len();
            self.s(s, last);
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
            S::Unreachable => self.line("error(\"entered unreachable code\", 0)"),
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
        let decls: Vec<String> = (0..self.f.vars.len())
            .filter(|v| self.top_decl[*v])
            .map(|v| format!("{}: {}", self.vname(v as VarId), ty_name(self.f.vars[v].ty)))
            .collect();
        for chunk in decls.chunks(16) {
            self.line(&format!("local {}", chunk.join(", ")));
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
                // use 3-digit decimal escapes so a following digit can't extend them
                let _ = i;
                write!(s, "\\{:03}", b).unwrap();
            }
        }
    }
    s.push('"');
    s
}
