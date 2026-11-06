//! CFG -> structured control flow.
//!
//! Stage 1 is Ramsey's "Beyond Relooper" (dominator-tree based) algorithm, producing a
//! WebAssembly-like tree of `block`/`loop`/`if`/`br`. Stage 2 maps that onto Luau:
//! branches in tail position become fallthrough, blocks nobody branches to are flattened,
//! exits of loops become `break`, back edges `continue`, and only the leftovers turn into
//! `repeat ... until true` blocks or (rarely) a multi-level exit via a label variable.

use std::collections::HashMap;

use crate::ir::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    /// Operand is a boolean; true when it is true.
    Truthy(Operand),
    Falsy(Operand),
    /// Operand (a number) equals any of the values.
    EqAny(Operand, Vec<f64>),
    NotEqAny(Operand, Vec<f64>),
    /// The multi-level-exit label variable equals this id.
    Label(u32),
    NotLabel(u32),
    LabelSet,
    LabelUnset,
}

/// Luau-level structured statement.
#[derive(Clone, Debug, PartialEq)]
pub enum S {
    Code(BlockId),
    If(Cond, Vec<S>, Vec<S>),
    /// `while true do ... end` (or `repeat ... until true` if `once`).
    Loop { once: bool, body: Vec<S> },
    Break,
    Continue,
    SetLabel(u32),
    Return(Vec<Operand>),
    Trap(Vec<TrapPart>),
    Unreachable,
}

#[derive(Clone, Debug)]
enum W {
    Block(u32, Vec<W>),
    Loop(u32, Vec<W>),
    If(Cond, Vec<W>, Vec<W>),
    Code(BlockId),
    Br(u32),
    Return(Vec<Operand>),
    Trap(Vec<TrapPart>),
    Unreachable,
}

pub struct Cfg {
    pub rpo: Vec<BlockId>,
    pub rpo_index: Vec<usize>, // usize::MAX if unreachable
    pub preds: Vec<Vec<BlockId>>,
    pub idom: Vec<BlockId>,
}

pub fn succs_dedup(t: &Term) -> Vec<BlockId> {
    let mut v = t.successors();
    let mut seen = Vec::new();
    v.retain(|b| {
        if seen.contains(b) {
            false
        } else {
            seen.push(*b);
            true
        }
    });
    v
}

impl Cfg {
    pub fn new(f: &Function) -> Cfg {
        let n = f.blocks.len();
        // iterative DFS postorder
        let mut visited = vec![false; n];
        let mut post = Vec::with_capacity(n);
        let mut stack: Vec<(BlockId, usize)> = vec![(f.entry, 0)];
        visited[f.entry as usize] = true;
        while let Some((b, i)) = stack.pop() {
            let succ = succs_dedup(&f.blocks[b as usize].term);
            if i < succ.len() {
                stack.push((b, i + 1));
                let s = succ[i];
                if !visited[s as usize] {
                    visited[s as usize] = true;
                    stack.push((s, 0));
                }
            } else {
                post.push(b);
            }
        }
        let rpo: Vec<BlockId> = post.into_iter().rev().collect();
        let mut rpo_index = vec![usize::MAX; n];
        for (i, b) in rpo.iter().enumerate() {
            rpo_index[*b as usize] = i;
        }
        let mut preds = vec![Vec::new(); n];
        for &b in &rpo {
            for s in succs_dedup(&f.blocks[b as usize].term) {
                preds[s as usize].push(b);
            }
        }
        // Cooper-Harvey-Kennedy dominators
        let undef = u32::MAX;
        let mut idom = vec![undef; n];
        idom[f.entry as usize] = f.entry;
        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter().skip(1) {
                let mut new_idom = undef;
                for &p in &preds[b as usize] {
                    if idom[p as usize] == undef {
                        continue;
                    }
                    new_idom = if new_idom == undef {
                        p
                    } else {
                        let (mut x, mut y) = (p, new_idom);
                        while x != y {
                            while rpo_index[x as usize] > rpo_index[y as usize] {
                                x = idom[x as usize];
                            }
                            while rpo_index[y as usize] > rpo_index[x as usize] {
                                y = idom[y as usize];
                            }
                        }
                        x
                    };
                }
                if idom[b as usize] != new_idom {
                    idom[b as usize] = new_idom;
                    changed = true;
                }
            }
        }
        Cfg { rpo, rpo_index, preds, idom }
    }

    pub fn dominates(&self, a: BlockId, mut b: BlockId) -> bool {
        loop {
            if a == b {
                return true;
            }
            let d = self.idom[b as usize];
            if d == b || d == u32::MAX {
                return false;
            }
            b = d;
        }
    }
}

struct Ramsey<'a> {
    f: &'a Function,
    cfg: Cfg,
    children: Vec<Vec<BlockId>>,
    is_merge: Vec<bool>,
    is_loop: Vec<bool>,
    next_label: u32,
    // ctx: (block, label, is_loop)
    ctx: Vec<(BlockId, u32, bool)>,
}

#[derive(Debug)]
pub struct Irreducible;

impl<'a> Ramsey<'a> {
    fn new(f: &'a Function) -> Result<Self, Irreducible> {
        let cfg = Cfg::new(f);
        let n = f.blocks.len();
        let mut children = vec![Vec::new(); n];
        for &b in cfg.rpo.iter().skip(1) {
            children[cfg.idom[b as usize] as usize].push(b);
        }
        for c in &mut children {
            c.sort_by_key(|b| cfg.rpo_index[*b as usize]);
        }
        let mut is_merge = vec![false; n];
        let mut is_loop = vec![false; n];
        for &b in &cfg.rpo {
            let mut fwd = 0;
            for &p in &cfg.preds[b as usize] {
                if cfg.rpo_index[p as usize] >= cfg.rpo_index[b as usize] {
                    // back edge: must target a dominator for reducibility
                    if !cfg.dominates(b, p) {
                        return Err(Irreducible);
                    }
                    is_loop[b as usize] = true;
                } else {
                    fwd += 1;
                }
            }
            is_merge[b as usize] = fwd >= 2;
        }
        Ok(Ramsey { f, cfg, children, is_merge, is_loop, next_label: 0, ctx: vec![] })
    }

    fn label(&mut self) -> u32 {
        self.next_label += 1;
        self.next_label
    }

    fn do_tree(&mut self, x: BlockId) -> Vec<W> {
        let mut merges: Vec<BlockId> =
            self.children[x as usize].iter().copied().filter(|c| self.is_merge[*c as usize]).collect();
        // highest rpo first
        merges.reverse();
        if self.is_loop[x as usize] {
            let l = self.label();
            self.ctx.push((x, l, true));
            let body = self.node_within(x, &merges);
            self.ctx.pop();
            vec![W::Loop(l, body)]
        } else {
            self.node_within(x, &merges)
        }
    }

    fn node_within(&mut self, x: BlockId, ys: &[BlockId]) -> Vec<W> {
        if let Some((&y0, rest)) = ys.split_first() {
            let l = self.label();
            self.ctx.push((y0, l, false));
            let inner = self.node_within(x, rest);
            self.ctx.pop();
            let mut out = vec![W::Block(l, inner)];
            out.extend(self.do_tree(y0));
            out
        } else {
            let mut out = vec![W::Code(x)];
            out.extend(self.terminator(x));
            out
        }
    }

    fn do_branch(&mut self, x: BlockId, t: BlockId) -> Vec<W> {
        let ri = &self.cfg.rpo_index;
        if ri[t as usize] <= ri[x as usize] {
            let l = self.ctx.iter().rev().find(|c| c.0 == t && c.2).expect("loop ctx").1;
            vec![W::Br(l)]
        } else if self.is_merge[t as usize] {
            let l = self.ctx.iter().rev().find(|c| c.0 == t && !c.2).expect("block ctx").1;
            vec![W::Br(l)]
        } else {
            self.do_tree(t)
        }
    }

    fn terminator(&mut self, x: BlockId) -> Vec<W> {
        match self.f.blocks[x as usize].term.clone() {
            Term::Goto(t) => self.do_branch(x, t),
            Term::Br(c, a, b) => {
                if a == b {
                    return self.do_branch(x, a);
                }
                let ta = self.do_branch(x, a);
                let tb = self.do_branch(x, b);
                vec![W::If(Cond::Truthy(c), ta, tb)]
            }
            Term::Switch(v, cases, default) => {
                // group by target, preserving first-appearance order
                let mut groups: Vec<(BlockId, Vec<f64>)> = Vec::new();
                for (val, t) in cases {
                    if t == default {
                        continue;
                    }
                    if let Some(g) = groups.iter_mut().find(|g| g.0 == t) {
                        g.1.push(val);
                    } else {
                        groups.push((t, vec![val]));
                    }
                }
                let mut tail = self.do_branch(x, default);
                for (t, vals) in groups.into_iter().rev() {
                    let arm = self.do_branch(x, t);
                    tail = vec![W::If(Cond::EqAny(v, vals), arm, tail)];
                }
                tail
            }
            Term::Return(v) => vec![W::Return(v)],
            Term::Trap(p) => vec![W::Trap(p)],
            Term::Unreachable => vec![W::Unreachable],
        }
    }
}

/// Union-find over continuations. A continuation is `exit(l) = 2l` (just after construct `l`)
/// or `head(l) = 2l+1` (the top of loop `l`).
struct Uf(HashMap<u32, u32>);
impl Uf {
    fn find(&mut self, x: u32) -> u32 {
        let p = *self.0.get(&x).unwrap_or(&x);
        if p == x {
            x
        } else {
            let r = self.find(p);
            self.0.insert(x, r);
            r
        }
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0.insert(ra, rb);
        }
    }
}

fn exit(l: u32) -> u32 {
    l * 2
}
fn head(l: u32) -> u32 {
    l * 2 + 1
}

type Labels = std::collections::HashSet<u32>;

/// Continuation a `Br(l)` jumps to.
fn target(l: u32, loops: &Labels) -> u32 {
    if loops.contains(&l) { head(l) } else { exit(l) }
}

/// Remove statements after unconditional jumps.
fn truncate_dead(ws: &mut Vec<W>) {
    if let Some(i) = ws.iter().position(|w| matches!(w, W::Br(_) | W::Return(_) | W::Trap(_) | W::Unreachable)) {
        ws.truncate(i + 1);
    }
    for w in ws.iter_mut() {
        match w {
            W::Block(_, b) | W::Loop(_, b) => truncate_dead(b),
            W::If(_, a, b) => {
                truncate_dead(a);
                truncate_dead(b);
            }
            _ => {}
        }
    }
}

/// Pass 1: continuation equivalences ("falling off the end of X is the same as jumping to
/// Y") and removal of branches that only jump to where control would fall anyway.
fn normalize(ws: &mut Vec<W>, fall: Option<u32>, uf: &mut Uf, loops: &Labels) {
    let n = ws.len();
    let mut i = 0;
    while i < ws.len() {
        let at_tail = i + 1 == n;
        let tail_fall = if at_tail { fall } else { None };
        match &mut ws[i] {
            W::Block(l, body) => {
                if let Some(f) = tail_fall {
                    uf.union(exit(*l), f);
                }
                let l = *l;
                normalize(body, Some(exit(l)), uf, loops);
            }
            W::Loop(l, body) => {
                if let Some(f) = tail_fall {
                    uf.union(exit(*l), f);
                }
                let l = *l;
                // falling off the end of a Luau loop body repeats the loop
                normalize(body, Some(head(l)), uf, loops);
            }
            W::If(_, a, b) => {
                normalize(a, tail_fall, uf, loops);
                normalize(b, tail_fall, uf, loops);
            }
            W::Br(l) => {
                if let Some(f) = tail_fall {
                    let t = target(*l, loops);
                    if uf.find(t) == uf.find(f) {
                        ws.remove(i);
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn count_refs(ws: &[W], counts: &mut HashMap<u32, u32>) {
    for w in ws {
        match w {
            W::Br(l) => *counts.entry(*l).or_default() += 1,
            W::Block(_, b) | W::Loop(_, b) => count_refs(b, counts),
            W::If(_, a, b) => {
                count_refs(a, counts);
                count_refs(b, counts);
            }
            _ => {}
        }
    }
}

fn flatten_unused(ws: Vec<W>, counts: &HashMap<u32, u32>) -> Vec<W> {
    let mut out = Vec::new();
    for w in ws {
        match w {
            W::Block(l, b) => {
                let b = flatten_unused(b, counts);
                if counts.get(&l).copied().unwrap_or(0) == 0 {
                    out.extend(b);
                } else {
                    out.push(W::Block(l, b));
                }
            }
            W::Loop(l, b) => out.push(W::Loop(l, flatten_unused(b, counts))),
            W::If(c, a, b) => out.push(W::If(c, flatten_unused(a, counts), flatten_unused(b, counts))),
            other => out.push(other),
        }
    }
    out
}

#[derive(Clone, Copy)]
struct LuauLoop {
    label: u32,
    /// `while` loop (has a head); otherwise a `repeat ... until true` block
    is_loop: bool,
}

struct Lower<'a> {
    uf: &'a mut Uf,
    loops: &'a Labels,
    repeat_blocks: Labels,
    stack: Vec<LuauLoop>,
    unresolved: Labels,
    next_flag: u32,
    uses_label_var: bool,
    /// per luau loop: (flag id, Some(continue?) when the next outer loop is the target, None = propagate)
    dispatch: HashMap<u32, Vec<(u32, Option<bool>)>>,
}

impl<'a> Lower<'a> {
    /// How luau loop `ll` can reach continuation `t`: Some(false)=break, Some(true)=continue.
    fn reach(&mut self, ll: LuauLoop, t: u32) -> Option<bool> {
        let t = self.uf.find(t);
        if self.uf.find(exit(ll.label)) == t {
            Some(false)
        } else if ll.is_loop && self.uf.find(head(ll.label)) == t {
            Some(true)
        } else {
            None
        }
    }

    fn branch(&mut self, l: u32) -> Vec<S> {
        let t = target(l, self.loops);
        let mut found = None;
        for i in (0..self.stack.len()).rev() {
            if let Some(cont) = self.reach(self.stack[i], t) {
                found = Some((i, cont));
                break;
            }
        }
        let Some((ti, cont)) = found else {
            self.unresolved.insert(l);
            return vec![S::Break];
        };
        let innermost = self.stack.len() - 1;
        if ti == innermost {
            return vec![if cont { S::Continue } else { S::Break }];
        }
        self.uses_label_var = true;
        self.next_flag += 1;
        let id = self.next_flag;
        for j in (ti + 1)..=innermost {
            let crossed = self.stack[j].label;
            let action = if j == ti + 1 { Some(cont) } else { None };
            self.dispatch.entry(crossed).or_default().push((id, action));
        }
        vec![S::SetLabel(id), S::Break]
    }

    fn seq(&mut self, ws: Vec<W>) -> Vec<S> {
        let mut out = Vec::new();
        for w in ws {
            match w {
                W::Code(b) => out.push(S::Code(b)),
                W::Return(v) => out.push(S::Return(v)),
                W::Trap(p) => out.push(S::Trap(p)),
                W::Unreachable => out.push(S::Unreachable),
                W::Br(l) => out.extend(self.branch(l)),
                W::If(c, a, b) => {
                    let a = self.seq(a);
                    let b = self.seq(b);
                    out.push(make_if(c, a, b));
                }
                W::Loop(l, body) => {
                    self.stack.push(LuauLoop { label: l, is_loop: true });
                    let body = self.seq(body);
                    self.stack.pop();
                    out.push(S::Loop { once: false, body });
                    self.emit_dispatch(l, &mut out);
                }
                W::Block(l, body) => {
                    if self.repeat_blocks.contains(&l) {
                        self.stack.push(LuauLoop { label: l, is_loop: false });
                        let body = self.seq(body);
                        self.stack.pop();
                        out.push(S::Loop { once: true, body });
                        self.emit_dispatch(l, &mut out);
                    } else {
                        out.extend(self.seq(body));
                    }
                }
            }
        }
        out
    }

    fn emit_dispatch(&mut self, l: u32, out: &mut Vec<S>) {
        if let Some(entries) = self.dispatch.remove(&l) {
            // all labels take the same action at this level: one check suffices
            if let Some(first) = entries.first().map(|e| e.1) {
                if entries.iter().all(|e| e.1 == first) {
                    let body = match first {
                        Some(cont) => vec![S::SetLabel(0), if cont { S::Continue } else { S::Break }],
                        None => vec![S::Break],
                    };
                    out.push(S::If(Cond::LabelSet, body, vec![]));
                    return;
                }
            }
            let mut inner = Vec::new();
            let mut propagate = false;
            for (id, action) in entries {
                match action {
                    Some(cont) => inner.push(S::If(
                        Cond::Label(id),
                        vec![S::SetLabel(0), if cont { S::Continue } else { S::Break }],
                        vec![],
                    )),
                    None => propagate = true,
                }
            }
            if propagate {
                inner.push(S::Break);
            }
            out.push(S::If(Cond::LabelSet, inner, vec![]));
        }
    }
}

fn make_if(c: Cond, a: Vec<S>, b: Vec<S>) -> S {
    if a.is_empty() && !b.is_empty() {
        S::If(negate_cond(c), b, vec![])
    } else {
        S::If(c, a, b)
    }
}

fn negate_cond(c: Cond) -> Cond {
    match c {
        Cond::Truthy(o) => Cond::Falsy(o),
        Cond::Falsy(o) => Cond::Truthy(o),
        Cond::EqAny(o, v) => Cond::NotEqAny(o, v),
        Cond::NotEqAny(o, v) => Cond::EqAny(o, v),
        Cond::Label(id) => Cond::NotLabel(id),
        Cond::NotLabel(id) => Cond::Label(id),
        Cond::LabelSet => Cond::LabelUnset,
        Cond::LabelUnset => Cond::LabelSet,
    }
}

/// Whether the CFG is reducible (structurable without node splitting).
pub fn is_reducible(f: &Function) -> bool {
    Ramsey::new(f).is_ok()
}

pub struct Structured {
    pub body: Vec<S>,
    pub uses_label_var: bool,
}

pub fn structure(f: &Function) -> Result<Structured, Irreducible> {
    let mut r = Ramsey::new(f)?;
    let mut ws = r.do_tree(f.entry);
    truncate_dead(&mut ws);
    let mut loops = Labels::new();
    collect_loop_labels(&ws, &mut loops);
    let mut uf = Uf(HashMap::new());
    normalize(&mut ws, None, &mut uf, &loops);
    let mut counts = HashMap::new();
    count_refs(&ws, &mut counts);
    let ws = flatten_unused(ws, &counts);

    let mut lower = Lower {
        uf: &mut uf,
        loops: &loops,
        repeat_blocks: Default::default(),
        stack: vec![],
        unresolved: Default::default(),
        next_flag: 0,
        uses_label_var: false,
        dispatch: HashMap::new(),
    };
    // Blocks that no enclosing `while` can exit to become `repeat ... until true`.
    let repeat = blocks_needing_repeat(&ws, &mut lower);
    lower.repeat_blocks = repeat;
    let body = lower.seq(ws);
    assert!(lower.unresolved.is_empty(), "unresolved branch targets");
    let mut body = body;
    drop_empty_code(&mut body, f);
    polish(&mut body);
    Ok(Structured { body, uses_label_var: lower.uses_label_var })
}

fn collect_loop_labels(ws: &[W], set: &mut Labels) {
    for w in ws {
        match w {
            W::Loop(l, b) => {
                set.insert(*l);
                collect_loop_labels(b, set);
            }
            W::Block(_, b) => collect_loop_labels(b, set),
            W::If(_, a, b) => {
                collect_loop_labels(a, set);
                collect_loop_labels(b, set);
            }
            _ => {}
        }
    }
}

/// Branches to blocks that the innermost enclosing `while` can't reach by break/continue.
fn blocks_needing_repeat(ws: &[W], lw: &mut Lower<'_>) -> Labels {
    fn walk(ws: &[W], lw: &mut Lower<'_>, out: &mut Labels) {
        for w in ws {
            match w {
                W::Br(l) if !lw.loops.contains(l) => {
                    let ok = match lw.stack.last() {
                        Some(ll) => lw.reach(*ll, exit(*l)).is_some(),
                        None => false,
                    };
                    if !ok {
                        out.insert(*l);
                    }
                }
                W::Loop(l, b) => {
                    lw.stack.push(LuauLoop { label: *l, is_loop: true });
                    walk(b, lw, out);
                    lw.stack.pop();
                }
                W::Block(_, b) => walk(b, lw, out),
                W::If(_, a, b) => {
                    walk(a, lw, out);
                    walk(b, lw, out);
                }
                _ => {}
            }
        }
    }
    let mut out = Labels::new();
    walk(ws, lw, &mut out);
    out
}

fn drop_empty_code(ss: &mut Vec<S>, f: &Function) {
    ss.retain(|s| !matches!(s, S::Code(b) if f.blocks[*b as usize].stmts.is_empty()));
    for s in ss.iter_mut() {
        match s {
            S::If(c, a, b) => {
                drop_empty_code(a, f);
                drop_empty_code(b, f);
                if a.is_empty() && !b.is_empty() {
                    *s = make_if(c.clone(), std::mem::take(a), std::mem::take(b));
                }
            }
            S::Loop { body, .. } => drop_empty_code(body, f),
            _ => {}
        }
    }
}

fn ends_in_jump(ss: &[S]) -> bool {
    match ss.last() {
        Some(S::Break | S::Continue | S::Return(_) | S::Trap(_) | S::Unreachable) => true,
        Some(S::If(_, a, b)) => !b.is_empty() && ends_in_jump(a) && ends_in_jump(b),
        _ => false,
    }
}

fn size(ss: &[S]) -> usize {
    ss.iter()
        .map(|s| match s {
            S::If(_, a, b) => 1 + size(a) + size(b),
            S::Loop { body, .. } => 1 + size(body),
            _ => 1,
        })
        .sum()
}

/// Cosmetic + register-pressure friendly rewrites:
/// - `if c then A(jumps) else B end` -> `if c then A end; B` (guard clauses, flatter hot path)
/// - `while true do if not c then break end ... end` -> `while c do ... end`
fn polish(ss: &mut Vec<S>) {
    let mut i = 0;
    while i < ss.len() {
        match &mut ss[i] {
            S::If(_, a, b) => {
                polish(a);
                polish(b);
            }
            S::Loop { body, .. } => polish(body),
            _ => {}
        }
        // guard clause flattening
        if let S::If(c, a, b) = &ss[i] {
            if !b.is_empty() {
                let (ja, jb) = (ends_in_jump(a), ends_in_jump(b));
                let pick_a = ja && (!jb || size(a) <= size(b));
                if pick_a || jb {
                    let (c, a, b) = (c.clone(), a.clone(), b.clone());
                    let (cond, guard, rest) = if pick_a { (c, a, b) } else { (negate_cond(c), b, a) };
                    ss[i] = S::If(cond, guard, vec![]);
                    let tail: Vec<S> = ss.drain(i + 1..).collect();
                    ss.extend(rest);
                    // statements after an If that jumped in both arms are dead
                    if !(ja && jb) {
                        ss.extend(tail);
                    }
                    continue;
                }
            }
        }
        i += 1;
    }
    // dead code after jumps
    if let Some(p) = ss.iter().position(|s| matches!(s, S::Break | S::Continue | S::Return(_) | S::Trap(_) | S::Unreachable)) {
        ss.truncate(p + 1);
    }
}
