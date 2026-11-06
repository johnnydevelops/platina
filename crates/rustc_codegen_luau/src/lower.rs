//! MIR -> LIR lowering for one function instance.


use rustc_abi::{FieldIdx, Size, TagEncoding, VariantIdx, Variants};
use rustc_middle::mir::interpret::{AllocId, Scalar, alloc_range};
use rustc_middle::mir::visit::{MutatingUseContext, NonMutatingUseContext, PlaceContext, Visitor};
use rustc_middle::mir::{
    self, AggregateKind, AssertKind, BasicBlock, BinOp, Body, CastKind, ConstValue, Local, Operand as MOp, Place,
    PlaceElem, Rvalue, StatementKind, TerminatorKind, UnOp,
};
use rustc_middle::ty::adjustment::PointerCoercion;
use rustc_middle::ty::layout::{FnAbiOf, LayoutOf, TyAndLayout};
use rustc_middle::ty::{self, Instance, InstanceKind, ShimKind, Ty, TyCtxt};
use rustc_span::Spanned;
use rustc_span::Span;
use rustc_target::callconv::FnAbi;

use luau_lir::ir::{self as l, Bin, BlockId, Callee, IntKind, Mem, Operand, Rv, Stmt, Term, TrapPart, Un, VarId};

use crate::abi::{self, LCx, Leaf, LeafKind};
use crate::cx::{Cx, typing_env};

#[derive(Clone, Copy, Debug)]
enum Base {
    Frame,
    Var(VarId),
}

#[derive(Clone)]
enum LocalRep<'tcx> {
    Regs { vars: Vec<VarId>, leaves: Vec<Leaf>, layout: TyAndLayout<'tcx> },
    Mem { base: Base, off: u64, layout: TyAndLayout<'tcx> },
}

#[derive(Clone)]
pub(crate) enum PlaceV<'tcx> {
    /// Part of a register local: byte range `[off, off+size)` of the local.
    Reg { local: Local, off: u64, layout: TyAndLayout<'tcx> },
    Mem { addr: Operand, off: i64, layout: TyAndLayout<'tcx>, meta: Option<Operand> },
}

impl<'tcx> PlaceV<'tcx> {
    fn layout(&self) -> TyAndLayout<'tcx> {
        match self {
            PlaceV::Reg { layout, .. } | PlaceV::Mem { layout, .. } => *layout,
        }
    }
    pub(crate) fn layout_pub(&self) -> TyAndLayout<'tcx> {
        self.layout()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Val {
    Regs(Vec<Operand>),
    /// Address of a memory-only value.
    Mem(Operand),
}

pub struct FnLower<'a, 'tcx> {
    cx: &'a mut Cx<'tcx>,
    tcx: TyCtxt<'tcx>,
    lcx: LCx<'tcx>,
    instance: Instance<'tcx>,
    mir: &'tcx Body<'tcx>,
    f: l::Function,
    cur: BlockId,
    bbs: Vec<BlockId>,
    locals: Vec<LocalRep<'tcx>>,
    frame_size: u64,
    frame_objs: Vec<(u64, u64)>,
    frame_align: u64,
    fp: VarId,
    sp_top: VarId,
    caller_loc: Option<VarId>,
    ret_leaves: Option<Vec<Leaf>>,
    span: Span,
}

fn fatal(tcx: TyCtxt<'_>, span: Span, msg: impl Into<String>) -> ! {
    tcx.dcx().span_fatal(span, msg.into())
}

/// Locals whose address is taken (or that are indexed) must live in memory.
struct AddrTaken {
    mem: Vec<bool>,
}
impl<'tcx> Visitor<'tcx> for AddrTaken {
    fn visit_place(&mut self, place: &Place<'tcx>, ctx: PlaceContext, _loc: mir::Location) {
        let borrow = matches!(
            ctx,
            PlaceContext::NonMutatingUse(NonMutatingUseContext::SharedBorrow | NonMutatingUseContext::FakeBorrow | NonMutatingUseContext::RawBorrow)
                | PlaceContext::MutatingUse(MutatingUseContext::Borrow | MutatingUseContext::RawBorrow)
        );
        // Borrowing through a deref (`&(*p).f`) reads `p`; only a borrow of the local's own
        // storage takes its address.
        if borrow && !place.projection.iter().any(|e| matches!(e, PlaceElem::Deref)) {
            self.mem[place.local.as_usize()] = true;
        }
        for elem in place.projection.iter() {
            match elem {
                PlaceElem::Deref => break,
                PlaceElem::Index(_) | PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. } => {
                    self.mem[place.local.as_usize()] = true;
                }
                _ => {}
            }
        }
    }
}

impl<'a, 'tcx> FnLower<'a, 'tcx> {
    pub fn lower(cx: &'a mut Cx<'tcx>, instance: Instance<'tcx>, name: String) -> l::Function {
        let tcx = cx.tcx;
        let mir = tcx.instance_mir(instance.def);
        if let Ok(filter) = std::env::var("LUAU_DUMP_MIR") {
            if name.contains(&filter) {
                let mut out = Vec::new();
                let _ = rustc_middle::mir::pretty::MirWriter::new(tcx).write_mir_fn(mir, &mut out);
                eprintln!("{}", String::from_utf8_lossy(&out));
            }
        }
        let mut f = l::Function::new(name);
        let entry = f.new_block();
        f.entry = entry;
        let fp = f.new_var(l::Ty::Num);
        f.vars[fp as usize].name = Some("fp".into());
        let sp_top = f.new_var(l::Ty::Num);
        let mut this = FnLower {
            cx,
            tcx,
            lcx: LCx(tcx),
            instance,
            mir,
            f,
            cur: entry,
            bbs: vec![],
            locals: vec![],
            frame_size: 0,
            frame_objs: vec![],
            frame_align: 8,
            fp,
            sp_top,
            caller_loc: None,
            ret_leaves: None,
            span: mir.span,
        };
        this.run();
        this.f
    }

    fn monomorphize<T: ty::TypeFoldable<TyCtxt<'tcx>> + Copy>(&self, v: T) -> T {
        self.instance.instantiate_mir_and_normalize_erasing_regions(self.tcx, typing_env(), ty::EarlyBinder::bind(self.tcx, v))
    }

    fn layout(&self, ty: Ty<'tcx>) -> TyAndLayout<'tcx> {
        self.lcx.layout_of(ty)
    }

    fn fatal(&self, msg: impl Into<String>) -> ! {
        fatal(self.tcx, self.span, msg)
    }

    // ---------------------------------------------------------------- emission helpers

    fn push(&mut self, s: Stmt) {
        self.f.blocks[self.cur as usize].stmts.push(s);
    }
    fn term(&mut self, t: Term) {
        self.f.blocks[self.cur as usize].term = t;
    }
    fn var(&mut self, ty: l::Ty) -> VarId {
        self.f.new_var(ty)
    }
    /// Fresh vars for the leaves of a value, with declared integer ranges.
    fn leaf_vars(&mut self, leaves: &[Leaf]) -> Vec<VarId> {
        abi::ltys(leaves)
            .into_iter()
            .zip(abi::ranges(leaves))
            .map(|(t, r)| {
                let v = self.var(t);
                self.f.vars[v as usize].range = r;
                v
            })
            .collect()
    }
    fn assign(&mut self, ty: l::Ty, rv: Rv) -> Operand {
        // constant folding of trivial cases
        if let Rv::Use(o) = rv {
            if !matches!(o, Operand::Var(_)) {
                return o;
            }
        }
        if let Some(c) = fold(&rv) {
            return c;
        }
        let v = self.var(ty);
        self.push(Stmt::Assign(v, rv));
        Operand::Var(v)
    }
    fn num(&mut self, rv: Rv) -> Operand {
        self.assign(l::Ty::Num, rv)
    }
    fn bin(&mut self, op: Bin, a: Operand, b: Operand) -> Operand {
        let ty = match op {
            Bin::Eq | Bin::Ne | Bin::Lt | Bin::Le | Bin::And | Bin::Or => l::Ty::Bool,
            _ => l::Ty::Num,
        };
        self.assign(ty, Rv::Bin(op, a, b))
    }
    fn un(&mut self, op: Un, a: Operand) -> Operand {
        let ty = match op {
            Un::Not | Un::NumToBool => l::Ty::Bool,
            _ => l::Ty::Num,
        };
        self.assign(ty, Rv::Un(op, a))
    }
    fn add_off(&mut self, a: Operand, off: i64) -> Operand {
        if off == 0 { a } else { self.bin(Bin::Add, a, Operand::Num(off as f64)) }
    }
    fn alloc_slot(&mut self, size: u64, align: u64) -> u64 {
        let align = align.max(1);
        self.frame_align = self.frame_align.max(align);
        let off = (self.frame_size + align - 1) / align * align;
        self.frame_size = off + size;
        self.frame_objs.push((off, off + size));
        off
    }
    /// Temporary memory slot; returns its address operand.
    fn temp_slot(&mut self, layout: TyAndLayout<'tcx>) -> Operand {
        let off = self.alloc_slot(layout.size.bytes(), layout.align.bytes());
        self.add_off(Operand::Var(self.fp), off as i64)
    }

    // ---------------------------------------------------------------- setup

    fn run(&mut self) {
        let tcx = self.tcx;
        let mir = self.mir;
        let fn_abi = self.lcx.fn_abi_of_instance(self.instance, ty::List::empty());

        // which locals live in memory
        let mut at = AddrTaken { mem: vec![false; mir.local_decls.len()] };
        at.visit_body(mir);

        for b in mir.basic_blocks.indices() {
            let _ = b;
            let lb = self.f.new_block();
            self.bbs.push(lb);
        }

        // return value
        let ret_layout = fn_abi.ret.layout;
        let ret_leaves = abi::leaves(self.lcx, ret_layout);
        let ret_ptr = if ret_leaves.is_none() {
            let v = self.var(l::Ty::Num);
            self.f.vars[v as usize].name = Some("ret".into());
            self.f.params.push(v);
            Some(v)
        } else {
            for t in abi::ltys(ret_leaves.as_ref().unwrap()) {
                self.f.rets.push(t);
            }
            None
        };
        self.ret_leaves = ret_leaves;

        // create local reps
        for (local, decl) in mir.local_decls.iter_enumerated() {
            let ty = self.monomorphize(decl.ty);
            let layout = self.layout(ty);
            let lv = if local == mir::RETURN_PLACE && ret_ptr.is_some() {
                None
            } else if at.mem[local.as_usize()] {
                None
            } else {
                abi::leaves(self.lcx, layout)
            };
            let rep = match lv {
                Some(leaves) => {
                    let vars = self.leaf_vars(&leaves);
                    LocalRep::Regs { vars, leaves, layout }
                }
                None => {
                    if local == mir::RETURN_PLACE && ret_ptr.is_some() {
                        LocalRep::Mem { base: Base::Var(ret_ptr.unwrap()), off: 0, layout }
                    } else if layout.is_unsized() {
                        self.fatal(format!("unsized local of type {ty}"))
                    } else {
                        // frame slot assigned below (args may use their incoming pointer)
                        LocalRep::Mem { base: Base::Frame, off: u64::MAX, layout }
                    }
                }
            };
            self.locals.push(rep);
        }

        // parameters
        let mut abi_args = fn_abi.args.iter();
        let mut prologue: Vec<Stmt> = vec![];
        for i in 0..mir.arg_count {
            let local = Local::from_usize(i + 1);
            let decl_ty = self.monomorphize(mir.local_decls[local].ty);
            let parts: Vec<TyAndLayout<'tcx>> = if Some(local) == mir.spread_arg {
                match decl_ty.kind() {
                    ty::Tuple(tys) => tys.iter().map(|t| self.layout(t)).collect(),
                    _ => self.fatal("spread arg is not a tuple"),
                }
            } else {
                vec![self.layout(decl_ty)]
            };
            let local_layout = self.layout(decl_ty);
            for (pi, part) in parts.iter().enumerate() {
                let arg_abi = abi_args.next().expect("abi arg");
                let _ = arg_abi;
                let field_off = if Some(local) == mir.spread_arg {
                    local_layout.fields.offset(pi).bytes()
                } else {
                    0
                };
                match abi::leaves(self.lcx, *part) {
                    Some(pl) => {
                        // register param(s)
                        let pvars = self.leaf_vars(&pl);
                        self.f.params.extend(pvars.iter().copied());
                        match self.locals[local.as_usize()].clone() {
                            LocalRep::Regs { vars, leaves, .. } => {
                                // find var range of this part in the local
                                let (offs, _) = abi::var_offsets(&leaves);
                                let li = leaves.iter().position(|l| l.off >= field_off).unwrap_or(leaves.len());
                                let start = if li < offs.len() { offs[li] } else { vars.len() };
                                for (k, pv) in pvars.iter().enumerate() {
                                    prologue.push(Stmt::Assign(vars[start + k], Rv::Use(Operand::Var(*pv))));
                                }
                            }
                            LocalRep::Mem { .. } => {
                                // stored after frame setup
                                prologue.push(Stmt::Call {
                                    dests: vec![],
                                    callee: Callee::Extern(format!("__store_param:{}:{}", local.as_usize(), field_off)),
                                    args: pvars.iter().map(|v| Operand::Var(*v)).collect(),
                                });
                            }
                        }
                    }
                    None => {
                        let p = self.var(l::Ty::Num);
                        self.f.params.push(p);
                        if Some(local) != mir.spread_arg {
                            if let LocalRep::Mem { base, off, .. } = &mut self.locals[local.as_usize()] {
                                *base = Base::Var(p);
                                *off = 0;
                            }
                        } else {
                            prologue.push(Stmt::Call {
                                dests: vec![],
                                callee: Callee::Extern(format!("__copy_param:{}:{}:{}", local.as_usize(), field_off, part.size.bytes())),
                                args: vec![Operand::Var(p)],
                            });
                        }
                    }
                }
            }
        }
        if self.instance.def.requires_caller_location(tcx) {
            let _ = abi_args.next();
            let v = self.var(l::Ty::Num);
            self.f.params.push(v);
            self.caller_loc = Some(v);
        }

        // assign frame slots to remaining memory locals
        for i in 0..self.locals.len() {
            if let LocalRep::Mem { base: Base::Frame, off, layout } = self.locals[i].clone() {
                if off == u64::MAX {
                    let o = self.alloc_slot(layout.size.bytes(), layout.align.bytes());
                    self.locals[i] = LocalRep::Mem { base: Base::Frame, off: o, layout };
                }
            }
        }

        // lower reachable blocks (constant-folding switches on compile-time-known conditions)
        let mut reachable = self.reachable_blocks();
        if crate::platform::is_body_override(tcx, self.instance) {
            reachable.iter_mut().for_each(|r| *r = false);
            self.cur = self.bbs[0];
            crate::platform::override_body(self);
        }
        for (bb, data) in mir.basic_blocks.iter_enumerated() {
            if data.is_cleanup || !reachable[bb.as_usize()] {
                self.cur = self.bbs[bb.as_usize()];
                self.term(Term::Unreachable);
                continue;
            }
            self.cur = self.bbs[bb.as_usize()];
            for stmt in &data.statements {
                self.span = stmt.source_info.span;
                self.stmt(stmt);
            }
            let t = data.terminator();
            self.span = t.source_info.span;
            self.terminator(bb, t);
        }

        // entry block: frame setup + param stores
        self.cur = self.f.entry;
        let fp = self.fp;
        let fp_used = self.f.blocks.iter().any(|b| {
            b.stmts.iter().any(|s| s.operands().iter().any(|o| **o == Operand::Var(fp)))
                || b.term.clone().operands_mut().iter().any(|o| **o == Operand::Var(fp))
        });
        if fp_used && self.frame_size == 0 {
            // only zero-sized locals: any address will do
            self.push(Stmt::GetSp(self.fp));
        }
        let uses_frame = self.frame_size > 0;
        if uses_frame {
            let size = (self.frame_size + self.frame_align - 1) / self.frame_align * self.frame_align;
            self.push(Stmt::GetSp(self.sp_top));
            let align = self.frame_align as f64;
            // fp = floor((top - size) / align) * align  (top is always 8-aligned)
            let fp_rv = if self.frame_align <= 8 {
                Rv::Bin(Bin::Sub, Operand::Var(self.sp_top), Operand::Num(size as f64))
            } else {
                let t = self.num(Rv::Bin(Bin::Sub, Operand::Var(self.sp_top), Operand::Num(size as f64)));
                let t2 = self.num(Rv::Bin(Bin::FloorDiv, t, Operand::Num(align)));
                Rv::Bin(Bin::Mul, t2, Operand::Num(align))
            };
            self.push(Stmt::Assign(self.fp, fp_rv));
            self.push(Stmt::SetSp(Operand::Var(self.fp)));
            if self.frame_align <= 8 {
                // fp = sp - size, so object offsets relative to the sp snapshot are known
                for (a, b) in &self.frame_objs {
                    self.f.frame_objects.push((self.sp_top, *a as i64 - size as i64, *b as i64 - size as i64));
                }
            }
        }
        for s in prologue {
            match s {
                Stmt::Call { callee: Callee::Extern(name), args, .. } if name.starts_with("__store_param:") => {
                    let mut it = name.split(':').skip(1);
                    let local: usize = it.next().unwrap().parse().unwrap();
                    let foff: u64 = it.next().unwrap().parse().unwrap();
                    let LocalRep::Mem { base, off, layout } = self.locals[local].clone() else { unreachable!() };
                    let addr = self.base_addr(base);
                    let local_layout = layout;
                    // the part's layout is the field at foff (or the whole local)
                    let part_layout = if foff == 0 && args.len() == abi::ltys(&abi::leaves(self.lcx, local_layout).unwrap_or_default()).len() {
                        local_layout
                    } else {
                        let idx = (0..local_layout.fields.count())
                            .find(|i| local_layout.fields.offset(*i).bytes() == foff)
                            .unwrap();
                        local_layout.field(&self.lcx, idx)
                    };
                    self.store_val(addr, (off + foff) as i64, part_layout, &args);
                }
                Stmt::Call { callee: Callee::Extern(name), args, .. } if name.starts_with("__copy_param:") => {
                    let mut it = name.split(':').skip(1);
                    let local: usize = it.next().unwrap().parse().unwrap();
                    let foff: u64 = it.next().unwrap().parse().unwrap();
                    let size: u64 = it.next().unwrap().parse().unwrap();
                    let LocalRep::Mem { base, off, .. } = self.locals[local].clone() else { unreachable!() };
                    let addr = self.base_addr(base);
                    let dst = self.add_off(addr, (off + foff) as i64);
                    self.push(Stmt::Copy { dst, src: args[0], len: Operand::Num(size as f64) });
                }
                s => self.push(s),
            }
        }
        let first = self.bbs[0];
        self.term(Term::Goto(first));

        if !uses_frame {
            let top = self.sp_top;
            for b in &mut self.f.blocks {
                b.stmts.retain(|s| !matches!(s, Stmt::SetSp(Operand::Var(v)) if *v == top));
            }
        }
    }

    /// Known value of a switch discriminant (runtime-check flags and constants).
    fn const_discr(&self, op: &MOp<'tcx>) -> Option<u128> {
        match op {
            MOp::RuntimeChecks(rc) => Some(rc.value(self.tcx.sess) as u128),
            MOp::Constant(c) => {
                let cv = self.monomorphize(c.const_);
                cv.try_eval_bits(self.tcx, typing_env())
            }
            _ => None,
        }
    }

    fn reachable_blocks(&self) -> Vec<bool> {
        let mir = self.mir;
        let mut seen = vec![false; mir.basic_blocks.len()];
        let mut stack = vec![mir::START_BLOCK];
        while let Some(bb) = stack.pop() {
            if std::mem::replace(&mut seen[bb.as_usize()], true) {
                continue;
            }
            let t = mir.basic_blocks[bb].terminator();
            if let TerminatorKind::SwitchInt { discr, targets } = &t.kind {
                if let Some(v) = self.const_discr(discr) {
                    stack.push(targets.target_for_value(v));
                    continue;
                }
            }
            for s in t.successors() {
                stack.push(s);
            }
        }
        seen
    }

    fn base_addr(&mut self, base: Base) -> Operand {
        match base {
            Base::Frame => Operand::Var(self.fp),
            Base::Var(v) => Operand::Var(v),
        }
    }

    // ---------------------------------------------------------------- places

    /// Values returned in registers (the return place may live in memory if its address is taken).
    fn return_values(&mut self) -> Vec<Operand> {
        if self.ret_leaves.is_none() {
            return vec![];
        }
        match &self.locals[mir::RETURN_PLACE.as_usize()] {
            LocalRep::Regs { vars, .. } => vars.iter().map(|v| Operand::Var(*v)).collect(),
            LocalRep::Mem { .. } => {
                let pv = self.place(Place::return_place());
                match self.read_place(&pv) {
                    Val::Regs(v) => v,
                    Val::Mem(_) => unreachable!(),
                }
            }
        }
    }

    fn local_leaves(&self, local: Local) -> (&[VarId], &[Leaf]) {
        match &self.locals[local.as_usize()] {
            LocalRep::Regs { vars, leaves, .. } => (vars, leaves),
            _ => unreachable!(),
        }
    }

    fn place(&mut self, place: Place<'tcx>) -> PlaceV<'tcx> {
        let local = place.local;
        let mut pt = mir::PlaceTy::from_ty(self.monomorphize(self.mir.local_decls[local].ty));
        let mut pv = match self.locals[local.as_usize()].clone() {
            LocalRep::Regs { layout, .. } => PlaceV::Reg { local, off: 0, layout },
            LocalRep::Mem { base, off, layout } => {
                let a = self.base_addr(base);
                PlaceV::Mem { addr: a, off: off as i64, layout, meta: None }
            }
        };
        for elem in place.projection.iter() {
            let elem = self.monomorphize(elem);
            let next_pt = pt.projection_ty(self.tcx, elem);
            pv = match elem {
                PlaceElem::Deref => {
                    let v = self.read_place(&pv);
                    let Val::Regs(ops) = v else { self.fatal("deref of memory pointer value") };
                    let pointee = self.layout(next_pt.ty);
                    PlaceV::Mem { addr: ops[0], off: 0, layout: pointee, meta: ops.get(1).copied() }
                }
                PlaceElem::Field(fi, fty) => {
                    let fty = self.monomorphize(fty);
                    match pv {
                        PlaceV::Reg { local, off, layout } => {
                            let foff = layout.fields.offset(fi.as_usize()).bytes();
                            let fl = layout.field(&self.lcx, fi.as_usize());
                            let fl = if fl.ty == fty { fl } else { self.layout(fty) };
                            PlaceV::Reg { local, off: off + foff, layout: fl }
                        }
                        PlaceV::Mem { addr, off, layout, meta } => {
                            let foff = layout.fields.offset(fi.as_usize()).bytes() as i64;
                            let fl = self.layout(fty);
                            let meta = if fl.is_unsized() { meta } else { None };
                            // An unsized field whose tail is `dyn Trait` is placed at an offset
                            // aligned to the *dynamic* alignment (from the vtable).
                            let tail = self.tcx.struct_tail_for_codegen(fty, typing_env());
                            if fl.is_unsized() && matches!(tail.kind(), ty::Dynamic(..)) && foff > 0 {
                                let vt = meta.expect("dyn field without vtable");
                                let align = self.num(Rv::Load { mem: Mem::U32, addr: vt, off: 8 });
                                let base = self.add_off(addr, off);
                                let unaligned = self.bin(Bin::Add, base, Operand::Num(foff as f64));
                                let am1 = self.bin(Bin::Sub, align, Operand::Num(1.0));
                                let t = self.bin(Bin::Add, unaligned, am1);
                                let q = self.bin(Bin::FloorDiv, t, align);
                                let a = self.bin(Bin::Mul, q, align);
                                PlaceV::Mem { addr: a, off: 0, layout: fl, meta: Some(vt) }
                            } else {
                                PlaceV::Mem { addr, off: off + foff, layout: fl, meta }
                            }
                        }
                    }
                }
                PlaceElem::Downcast(_, v) => match pv {
                    PlaceV::Reg { local, off, layout } => PlaceV::Reg { local, off, layout: layout.for_variant(&self.lcx, v) },
                    PlaceV::Mem { addr, off, layout, meta } => {
                        PlaceV::Mem { addr, off, layout: layout.for_variant(&self.lcx, v), meta }
                    }
                },
                PlaceElem::Index(idx_local) => {
                    let ip = self.place(Place::from(idx_local));
                    let idx = self.read_place(&ip);
                    let Val::Regs(i) = idx else { unreachable!() };
                    self.index_place(pv, i[0], next_pt.ty)
                }
                PlaceElem::ConstantIndex { offset, from_end, .. } => {
                    let i = if from_end {
                        let len = self.place_len(&pv);
                        self.bin(Bin::Sub, len, Operand::Num(offset as f64))
                    } else {
                        Operand::Num(offset as f64)
                    };
                    self.index_place(pv, i, next_pt.ty)
                }
                PlaceElem::Subslice { from, to, from_end } => {
                    let PlaceV::Mem { addr, off, layout, meta } = pv else { self.fatal("subslice of register") };
                    let elem = layout.field(&self.lcx, 0);
                    let new_off = off + (from * elem.size.bytes()) as i64;
                    let new_layout = self.layout(next_pt.ty);
                    let meta = if from_end {
                        let len = meta.expect("slice len");
                        Some(self.bin(Bin::Sub, len, Operand::Num((from + to) as f64)))
                    } else {
                        None
                    };
                    PlaceV::Mem { addr, off: new_off, layout: new_layout, meta }
                }
                PlaceElem::OpaqueCast(_) | PlaceElem::UnwrapUnsafeBinder(_) => {
                    let lay = self.layout(next_pt.ty);
                    match pv {
                        PlaceV::Reg { local, off, .. } => PlaceV::Reg { local, off, layout: lay },
                        PlaceV::Mem { addr, off, meta, .. } => PlaceV::Mem { addr, off, layout: lay, meta },
                    }
                }
            };
            pt = next_pt;
        }
        pv
    }

    fn place_len(&mut self, pv: &PlaceV<'tcx>) -> Operand {
        match pv {
            PlaceV::Mem { layout, meta, .. } => match layout.ty.kind() {
                ty::Array(_, n) => Operand::Num(n.try_to_target_usize(self.tcx).unwrap() as f64),
                _ => meta.expect("slice length"),
            },
            _ => self.fatal("len of register place"),
        }
    }

    fn index_place(&mut self, pv: PlaceV<'tcx>, idx: Operand, elem_ty: Ty<'tcx>) -> PlaceV<'tcx> {
        let PlaceV::Mem { addr, off, .. } = pv else { self.fatal("index of register place") };
        let el = self.layout(elem_ty);
        let stride = el.size.bytes();
        if let Operand::Num(i) = idx {
            return PlaceV::Mem { addr, off: off + (i as i64) * stride as i64, layout: el, meta: None };
        }
        // base + offset first: the dynamic pointer must point into its own object (provenance)
        let base = self.add_off(addr, off);
        let scaled = if stride == 1 { idx } else { self.bin(Bin::Mul, idx, Operand::Num(stride as f64)) };
        let a = self.bin(Bin::Add, base, scaled);
        PlaceV::Mem { addr: a, off: 0, layout: el, meta: None }
    }

    /// Address of a memory place (for references).
    fn place_addr(&mut self, pv: &PlaceV<'tcx>) -> (Operand, Option<Operand>) {
        match pv {
            PlaceV::Mem { addr, off, meta, .. } => (self.add_off(*addr, *off), *meta),
            PlaceV::Reg { .. } => self.fatal("address of register place"),
        }
    }

    fn reg_vars(&self, local: Local, off: u64, layout: TyAndLayout<'tcx>) -> (Vec<VarId>, Vec<Leaf>) {
        let (vars, leaves) = self.local_leaves(local);
        let (offs, _) = abi::var_offsets(leaves);
        let lo = off;
        let hi = off + layout.size.bytes();
        let mut vs = vec![];
        let mut ls = vec![];
        for (i, lf) in leaves.iter().enumerate() {
            if lf.off >= lo && lf.off < hi {
                for k in 0..lf.kind.nvars() {
                    vs.push(vars[offs[i] + k]);
                }
                ls.push(Leaf { off: lf.off - lo, kind: lf.kind });
            }
        }
        (vs, ls)
    }

    fn natural_leaves(&self, layout: TyAndLayout<'tcx>) -> Option<Vec<Leaf>> {
        abi::leaves(self.lcx, layout)
    }

    fn read_place(&mut self, pv: &PlaceV<'tcx>) -> Val {
        match pv {
            PlaceV::Reg { local, off, layout } => {
                let (vars, have) = self.reg_vars(*local, *off, *layout);
                let want = self.natural_leaves(*layout).unwrap_or_else(|| have.clone());
                let mut ops = vec![];
                let mut vi = 0;
                for (h, w) in have.iter().zip(want.iter()) {
                    let n = h.kind.nvars();
                    let src: Vec<Operand> = vars[vi..vi + n].iter().map(|v| Operand::Var(*v)).collect();
                    vi += n;
                    let conv = self.convert_leaf(h.kind, w.kind, &src);
                    for c in conv {
                        // copy so later writes to the local don't affect this value
                        let t = self.copy_op(c);
                        ops.push(t);
                    }
                }
                Val::Regs(ops)
            }
            PlaceV::Mem { addr, off, layout, .. } => match self.natural_leaves(*layout) {
                Some(leaves) => Val::Regs(self.load_val(*addr, *off, &leaves)),
                None => Val::Mem(self.add_off(*addr, *off)),
            },
        }
    }

    fn copy_op(&mut self, o: Operand) -> Operand {
        match o {
            Operand::Var(v) => {
                let ty = self.f.ty(v);
                let t = self.var(ty);
                self.push(Stmt::Assign(t, Rv::Use(o)));
                Operand::Var(t)
            }
            c => c,
        }
    }

    fn convert_leaf(&mut self, from: LeafKind, to: LeafKind, src: &[Operand]) -> Vec<Operand> {
        if from == to {
            return src.to_vec();
        }
        match (from, to) {
            (LeafKind::Int(_), LeafKind::Bool) => vec![self.un(Un::NumToBool, src[0])],
            (LeafKind::Bool, LeafKind::Int(_)) => vec![self.un(Un::BoolToNum, src[0])],
            (LeafKind::Int(a), LeafKind::Int(b)) if a.bits == b.bits => vec![self.un(Un::Wrap(b), src[0])],
            (LeafKind::I64 { .. }, LeafKind::I64 { .. }) | (LeafKind::I128 { .. }, LeafKind::I128 { .. }) => src.to_vec(),
            _ => self.fatal(format!("cannot convert leaf {from:?} to {to:?}")),
        }
    }

    fn load_val(&mut self, addr: Operand, off: i64, leaves: &[Leaf]) -> Vec<Operand> {
        let mut out = vec![];
        for lf in leaves {
            for (mem, po) in lf.kind.mem_parts() {
                let ty = if mem == Mem::Bool { l::Ty::Bool } else { l::Ty::Num };
                let v = self.var(ty);
                self.push(Stmt::Assign(v, Rv::Load { mem, addr, off: (off + lf.off as i64 + po as i64) as i32 }));
                out.push(Operand::Var(v));
            }
        }
        out
    }

    fn store_val(&mut self, addr: Operand, off: i64, layout: TyAndLayout<'tcx>, vals: &[Operand]) {
        let leaves = self.natural_leaves(layout).expect("store of memory type as regs");
        let mut vi = 0;
        for lf in &leaves {
            for (mem, po) in lf.kind.mem_parts() {
                self.push(Stmt::Store { mem, addr, off: (off + lf.off as i64 + po as i64) as i32, val: vals[vi] });
                vi += 1;
            }
        }
    }

    fn write_place(&mut self, pv: &PlaceV<'tcx>, v: Val) {
        match (pv, v) {
            (PlaceV::Reg { local, off, layout }, Val::Regs(ops)) => {
                let (vars, have) = self.reg_vars(*local, *off, *layout);
                let want = self.natural_leaves(*layout).unwrap_or_else(|| have.clone());
                let mut vi = 0;
                let mut oi = 0;
                for (h, w) in have.iter().zip(want.iter()) {
                    let src = &ops[oi..oi + w.kind.nvars()];
                    oi += w.kind.nvars();
                    let conv = self.convert_leaf(w.kind, h.kind, src);
                    for c in conv {
                        self.push(Stmt::Assign(vars[vi], Rv::Use(c)));
                        vi += 1;
                    }
                }
            }
            (PlaceV::Reg { local, off, layout }, Val::Mem(a)) => {
                let (vars, have) = self.reg_vars(*local, *off, *layout);
                let ops = self.load_val(a, 0, &have);
                for (v, o) in vars.iter().zip(ops) {
                    self.push(Stmt::Assign(*v, Rv::Use(o)));
                }
            }
            (PlaceV::Mem { addr, off, layout, .. }, Val::Regs(ops)) => {
                if layout.size.bytes() == 0 {
                    return;
                }
                self.store_val(*addr, *off, *layout, &ops);
            }
            (PlaceV::Mem { addr, off, layout, .. }, Val::Mem(src)) => {
                let size = layout.size.bytes();
                if size > 0 {
                    let dst = self.add_off(*addr, *off);
                    self.push(Stmt::Copy { dst, src, len: Operand::Num(size as f64) });
                }
            }
        }
    }

    /// Materialize any value in memory, returning its address.
    fn spill(&mut self, v: Val, layout: TyAndLayout<'tcx>) -> Operand {
        match v {
            Val::Mem(a) => a,
            Val::Regs(ops) => {
                let a = self.temp_slot(layout);
                self.store_val(a, 0, layout, &ops);
                a
            }
        }
    }

    // ---------------------------------------------------------------- operands & constants

    fn operand(&mut self, op: &MOp<'tcx>) -> (Val, TyAndLayout<'tcx>) {
        match op {
            MOp::Copy(p) | MOp::Move(p) => {
                let pv = self.place(*p);
                let lay = pv.layout();
                (self.read_place(&pv), lay)
            }
            MOp::Constant(c) => {
                let ty = self.monomorphize(c.ty());
                let lay = self.layout(ty);
                let cv = self.monomorphize(c.const_);
                let val = match cv.eval(self.tcx, typing_env(), c.span) {
                    Ok(v) => v,
                    Err(_) => self.fatal("constant evaluation failed"),
                };
                (self.const_value(val, lay), lay)
            }
            MOp::RuntimeChecks(rc) => {
                let b = rc.value(self.tcx.sess);
                (Val::Regs(vec![Operand::Bool(b)]), self.layout(self.tcx.types.bool))
            }
        }
    }

    fn operand_regs(&mut self, op: &MOp<'tcx>) -> (Vec<Operand>, TyAndLayout<'tcx>) {
        let (v, l) = self.operand(op);
        match v {
            Val::Regs(r) => (r, l),
            Val::Mem(_) => self.fatal(format!("expected register value of {}", l.ty)),
        }
    }

    fn const_value(&mut self, val: ConstValue, lay: TyAndLayout<'tcx>) -> Val {
        let leaves = self.natural_leaves(lay);
        match val {
            ConstValue::ZeroSized => Val::Regs(vec![]),
            ConstValue::Scalar(s) => {
                let leaves = leaves.expect("scalar const of memory type");
                if leaves.is_empty() {
                    return Val::Regs(vec![]);
                }
                Val::Regs(self.scalar_ops(s, leaves[0].kind))
            }
            ConstValue::Slice { alloc_id, meta } => {
                let a = self.cx.alloc_value(alloc_id);
                Val::Regs(vec![Operand::Num(a as f64), Operand::Num(meta as f64)])
            }
            ConstValue::Indirect { alloc_id, offset } => {
                let alloc = self.tcx.global_alloc(alloc_id).unwrap_memory();
                match leaves {
                    Some(leaves) => {
                        let mut ops = vec![];
                        for lf in &leaves {
                            let size = Size::from_bytes(lf.kind.size());
                            let start = offset + Size::from_bytes(lf.off);
                            let ptr_sized = size == self.tcx.data_layout.pointer_size();
                            let s = alloc
                                .inner()
                                .read_scalar(&self.lcx, alloc_range(start, size), ptr_sized)
                                .unwrap_or(Scalar::from_uint(0u128, size));
                            ops.extend(self.scalar_ops(s, lf.kind));
                        }
                        Val::Regs(ops)
                    }
                    None => {
                        let a = self.cx.alloc_value(alloc_id);
                        Val::Mem(Operand::Num((a as u64 + offset.bytes()) as f64))
                    }
                }
            }
        }
    }

    fn scalar_ops(&mut self, s: Scalar, kind: LeafKind) -> Vec<Operand> {
        match s {
            Scalar::Int(i) => {
                let bits = i.to_bits(i.size());
                const_bits(bits, kind)
            }
            Scalar::Ptr(p, _) => {
                let (prov, off) = p.into_raw_parts();
                let base = self.cx.alloc_value(prov.alloc_id());
                vec![Operand::Num(base.wrapping_add(off.bytes() as u32) as f64)]
            }
        }
    }

    fn alloc_addr(&mut self, id: AllocId) -> Operand {
        Operand::Num(self.cx.alloc_value(id) as f64)
    }

    // ---------------------------------------------------------------- statements

    fn stmt(&mut self, stmt: &mir::Statement<'tcx>) {
        match &stmt.kind {
            StatementKind::Assign(b) => {
                let (place, rv) = &**b;
                let dest = self.place(*place);
                self.rvalue(&dest, rv);
            }
            StatementKind::SetDiscriminant { place, variant_index } => {
                let pv = self.place(**place);
                self.set_discriminant(&pv, *variant_index);
            }
            StatementKind::Intrinsic(i) => match &**i {
                mir::NonDivergingIntrinsic::Assume(_) => {}
                mir::NonDivergingIntrinsic::CopyNonOverlapping(c) => {
                    let (src, _) = self.operand_regs(&c.src);
                    let (dst, dl) = self.operand_regs(&c.dst);
                    let (count, _) = self.operand_regs(&c.count);
                    let pointee = dl.ty.builtin_deref(true).unwrap();
                    let size = self.layout(pointee).size.bytes();
                    let len = if size == 1 { count[0] } else { self.bin(Bin::Mul, count[0], Operand::Num(size as f64)) };
                    self.push(Stmt::Copy { dst: dst[0], src: src[0], len });
                }
            },
            _ => {}
        }
    }

    fn rvalue(&mut self, dest: &PlaceV<'tcx>, rv: &Rvalue<'tcx>) {
        let dl = dest.layout();
        match rv {
            Rvalue::Use(op, _) | Rvalue::WrapUnsafeBinder(op, _) => {
                let (v, _) = self.operand(op);
                self.write_place(dest, v);
            }
            Rvalue::CopyForDeref(p) | Rvalue::Reborrow(_, _, p) => {
                let pv = self.place(*p);
                let v = self.read_place(&pv);
                self.write_place(dest, v);
            }
            Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) => {
                let pv = self.place(*p);
                let (a, meta) = self.place_addr(&pv);
                let mut ops = vec![a];
                if let Some(m) = meta {
                    ops.push(m);
                }
                let want = abi::ltys(&self.natural_leaves(dl).unwrap()).len();
                ops.truncate(want);
                self.write_place(dest, Val::Regs(ops));
            }
            Rvalue::Repeat(op, count) => {
                let n = self.monomorphize(*count).try_to_target_usize(self.tcx).unwrap();
                let (v, el) = self.operand(op);
                let (base, _) = self.place_addr(dest);
                let stride = el.size.bytes();
                if stride == 0 || n == 0 {
                    return;
                }
                if let (Val::Regs(ops), 1) = (&v, stride) {
                    if let [o] = ops.as_slice() {
                        let o = if self.f.ty_of(o) == l::Ty::Bool { self.un(Un::BoolToNum, *o) } else { *o };
                        self.push(Stmt::Fill { dst: base, val: o, len: Operand::Num(n as f64) });
                        return;
                    }
                }
                if n <= 8 {
                    for i in 0..n {
                        let p = PlaceV::Mem { addr: base, off: (i * stride) as i64, layout: el, meta: None };
                        self.write_place(&p, v.clone());
                    }
                } else {
                    // loop: i from 0 to n-1
                    let i = self.var(l::Ty::Num);
                    self.push(Stmt::Assign(i, Rv::Use(Operand::Num(0.0))));
                    let head = self.f.new_block();
                    let body = self.f.new_block();
                    let exit = self.f.new_block();
                    self.term(Term::Goto(head));
                    self.cur = head;
                    let c = self.bin(Bin::Lt, Operand::Var(i), Operand::Num(n as f64));
                    self.term(Term::Br(c, body, exit));
                    self.cur = body;
                    let off = self.bin(Bin::Mul, Operand::Var(i), Operand::Num(stride as f64));
                    let a = self.bin(Bin::Add, base, off);
                    let p = PlaceV::Mem { addr: a, off: 0, layout: el, meta: None };
                    self.write_place(&p, v.clone());
                    self.push(Stmt::Assign(i, Rv::Bin(Bin::Add, Operand::Var(i), Operand::Num(1.0))));
                    self.term(Term::Goto(head));
                    self.cur = exit;
                }
            }
            Rvalue::ThreadLocalRef(_) => self.fatal("thread locals are not supported"),
            Rvalue::Cast(kind, op, to) => {
                let to = self.monomorphize(*to);
                let v = self.cast(*kind, op, to);
                self.write_place(dest, v);
            }
            Rvalue::BinaryOp(op, ab) => {
                let (a, al) = self.operand_regs(&ab.0);
                let (b, bl) = self.operand_regs(&ab.1);
                let r = self.binop(*op, &a, al, &b, bl, dl);
                self.write_place(dest, Val::Regs(r));
            }
            Rvalue::UnaryOp(op, a) => {
                let (a, al) = self.operand_regs(a);
                let r = match op {
                    UnOp::Not => match leaf_of(al.ty) {
                        Some(LeafKind::Bool) => vec![self.un(Un::Not, a[0])],
                        Some(LeafKind::Int(k)) => {
                            if k.signed {
                                let n = self.un(Un::Neg, a[0]);
                                vec![self.bin(Bin::Sub, n, Operand::Num(1.0))]
                            } else {
                                vec![self.bin(Bin::Sub, Operand::Num(k.max()), a[0])]
                            }
                        }
                        Some(LeafKind::I64 { .. } | LeafKind::I128 { .. }) => {
                            a.iter().map(|x| self.bin(Bin::Sub, Operand::Num(4294967295.0), *x)).collect()
                        }
                        _ => self.fatal("bad Not"),
                    },
                    UnOp::Neg => match leaf_of(al.ty) {
                        Some(LeafKind::Int(k)) => {
                            let n = self.un(Un::Neg, a[0]);
                            vec![self.un(Un::Wrap(k), n)]
                        }
                        Some(LeafKind::F32 | LeafKind::F64) => vec![self.un(Un::Neg, a[0])],
                        Some(LeafKind::I64 { .. }) => self.call_helper("neg64", &a, 2),
                        Some(LeafKind::I128 { .. }) => self.call_helper("neg128", &a, 4),
                        _ => self.fatal("bad Neg"),
                    },
                    UnOp::PtrMetadata => {
                        if a.len() >= 2 {
                            vec![a[1]]
                        } else {
                            vec![]
                        }
                    }
                };
                self.write_place(dest, Val::Regs(r));
            }
            Rvalue::Discriminant(p) => {
                let pv = self.place(*p);
                let d = self.get_discriminant(&pv, dl);
                self.write_place(dest, Val::Regs(d));
            }
            Rvalue::Aggregate(kind, ops) => self.aggregate(dest, kind, ops),
        }
    }

    fn call_helper(&mut self, name: &str, args: &[Operand], nret: usize) -> Vec<Operand> {
        let dests: Vec<VarId> = (0..nret).map(|_| self.var(l::Ty::Num)).collect();
        self.push(Stmt::Call { dests: dests.clone(), callee: Callee::Extern(name.into()), args: args.to_vec() });
        dests.into_iter().map(Operand::Var).collect()
    }

    fn aggregate(&mut self, dest: &PlaceV<'tcx>, kind: &AggregateKind<'tcx>, ops: &rustc_index::IndexSlice<FieldIdx, MOp<'tcx>>) {
        match kind {
            AggregateKind::RawPtr(..) => {
                let mut all = vec![];
                for op in ops.iter() {
                    let (v, _) = self.operand_regs(op);
                    all.extend(v);
                }
                let want = abi::ltys(&self.natural_leaves(dest.layout()).unwrap()).len();
                all.truncate(want);
                self.write_place(dest, Val::Regs(all));
            }
            AggregateKind::Array(_) => {
                let (base, _) = self.place_addr(dest);
                let el = dest.layout().field(&self.lcx, 0);
                for (i, op) in ops.iter().enumerate() {
                    let (v, _) = self.operand(op);
                    let p = PlaceV::Mem { addr: base, off: (i as u64 * el.size.bytes()) as i64, layout: el, meta: None };
                    self.write_place(&p, v);
                }
            }
            AggregateKind::Adt(def_id, variant, _, _, active) => {
                let adt = self.tcx.adt_def(*def_id);
                let (target, variant) = if adt.is_enum() {
                    (self.downcast(dest, *variant), Some(*variant))
                } else {
                    (dest.clone(), None)
                };
                // evaluate all operands first (they may read the destination)
                let vals: Vec<(Val, TyAndLayout<'tcx>)> = ops.iter().map(|op| self.operand(op)).collect();
                for (i, (v, _)) in vals.into_iter().enumerate() {
                    let fi = match active {
                        Some(a) => *a,
                        None => FieldIdx::from_usize(i),
                    };
                    let fp = self.field(&target, fi);
                    self.write_place(&fp, v);
                }
                if let Some(v) = variant {
                    self.set_discriminant(dest, v);
                }
            }
            AggregateKind::Tuple | AggregateKind::Closure(..) => {
                let vals: Vec<(Val, TyAndLayout<'tcx>)> = ops.iter().map(|op| self.operand(op)).collect();
                for (i, (v, _)) in vals.into_iter().enumerate() {
                    let fp = self.field(dest, FieldIdx::from_usize(i));
                    self.write_place(&fp, v);
                }
            }
            _ => self.fatal("coroutines are not supported"),
        }
    }

    fn downcast(&self, pv: &PlaceV<'tcx>, v: VariantIdx) -> PlaceV<'tcx> {
        match pv.clone() {
            PlaceV::Reg { local, off, layout } => PlaceV::Reg { local, off, layout: layout.for_variant(&self.lcx, v) },
            PlaceV::Mem { addr, off, layout, meta } => PlaceV::Mem { addr, off, layout: layout.for_variant(&self.lcx, v), meta },
        }
    }

    fn field(&self, pv: &PlaceV<'tcx>, fi: FieldIdx) -> PlaceV<'tcx> {
        let lay = pv.layout();
        let foff = lay.fields.offset(fi.as_usize()).bytes();
        let fl = lay.field(&self.lcx, fi.as_usize());
        match pv.clone() {
            PlaceV::Reg { local, off, .. } => PlaceV::Reg { local, off: off + foff, layout: fl },
            PlaceV::Mem { addr, off, .. } => PlaceV::Mem { addr, off: off + foff as i64, layout: fl, meta: None },
        }
    }

    // ---------------------------------------------------------------- enums

    fn tag_place(&self, pv: &PlaceV<'tcx>, tag_field: FieldIdx) -> PlaceV<'tcx> {
        self.field(pv, tag_field)
    }

    fn set_discriminant(&mut self, pv: &PlaceV<'tcx>, v: VariantIdx) {
        let layout = pv.layout();
        // use the enum's full layout (pv may already be a variant view)
        if layout.for_variant(&self.lcx, v).is_uninhabited() {
            return;
        }
        match layout.variants {
            Variants::Empty | Variants::Single { .. } => {}
            Variants::Multiple { tag_field, tag_encoding: TagEncoding::Direct, .. } => {
                let tp = self.tag_place(pv, tag_field);
                let to = layout.ty.discriminant_for_variant(self.tcx, v).unwrap().val;
                let kind = self.natural_leaves(tp.layout()).unwrap()[0].kind;
                let raw = tp.layout().size.truncate(to);
                self.write_place(&tp, Val::Regs(const_bits(raw, kind)));
            }
            Variants::Multiple { tag_field, tag_encoding: TagEncoding::Niche { untagged_variant, ref niche_variants, niche_start }, .. } => {
                if v != untagged_variant {
                    let tp = self.tag_place(pv, tag_field);
                    let niche_value = (v.as_u32() - niche_variants.start.as_u32()) as u128;
                    let niche_value = niche_value.wrapping_add(niche_start);
                    let raw = tp.layout().size.truncate(niche_value);
                    let kind = self.tag_kind(&tp);
                    self.write_place(&tp, Val::Regs(const_bits(raw, kind)));
                }
            }
        }
    }

    fn tag_kind(&self, tp: &PlaceV<'tcx>) -> LeafKind {
        self.natural_leaves(tp.layout()).and_then(|l| l.first().map(|x| x.kind)).unwrap_or(LeafKind::Int(IntKind::U32))
    }

    fn get_discriminant(&mut self, pv: &PlaceV<'tcx>, dl: TyAndLayout<'tcx>) -> Vec<Operand> {
        let layout = pv.layout();
        let dk = leaf_of(dl.ty).unwrap_or(LeafKind::Int(IntKind::U32));
        if layout.is_uninhabited() {
            return const_bits(0, dk);
        }
        match layout.variants {
            Variants::Empty => const_bits(0, dk),
            Variants::Single { index } => {
                let d = layout.ty.discriminant_for_variant(self.tcx, index).map_or(index.as_u32() as u128, |d| d.val);
                const_bits(dl.size.truncate(d), dk)
            }
            Variants::Multiple { tag, tag_field, ref tag_encoding, .. } => {
                let tp = self.tag_place(pv, tag_field);
                let Val::Regs(t) = self.read_place(&tp) else { unreachable!() };
                let tk = self.tag_kind(&tp);
                let t0 = t[0];
                let bits = tag.size(&self.lcx).bits() as u8;
                match *tag_encoding {
                    TagEncoding::Direct => {
                        // tag (canonical in its own kind) -> discriminant type
                        self.int_cast(&t, tk, dk)
                    }
                    TagEncoding::Niche { untagged_variant, ref niche_variants, niche_start } => {
                        let relative_max = niche_variants.last.as_u32() - niche_variants.start.as_u32();
                        if bits > 32 {
                            // wide tag: rel = (tag - niche_start) mod 2^bits via limbs
                            let wk = if bits == 64 { LeafKind::I64 { signed: false } } else { LeafKind::I128 { signed: false } };
                            let start = const_bits(niche_start, wk);
                            let mut args = t.clone();
                            args.extend(start);
                            let rel = self.call_helper(if bits == 64 { "sub64" } else { "sub128" }, &args, t.len());
                            let mut upper_zero = Operand::Bool(true);
                            for limb in &rel[1..] {
                                let z = self.bin(Bin::Eq, *limb, Operand::Num(0.0));
                                upper_zero = self.bin(Bin::And, upper_zero, z);
                            }
                            let le = self.bin(Bin::Le, rel[0], Operand::Num(relative_max as f64));
                            let is_niche = self.bin(Bin::And, upper_zero, le);
                            let ns = niche_variants.start.as_u32() as f64;
                            let tagged = self.bin(Bin::Add, rel[0], Operand::Num(ns));
                            let r = self.num(Rv::Select(is_niche, tagged, Operand::Num(untagged_variant.as_u32() as f64)));
                            return match dk {
                                LeafKind::I64 { .. } => vec![r, Operand::Num(0.0)],
                                LeafKind::I128 { .. } => vec![r, Operand::Num(0.0), Operand::Num(0.0), Operand::Num(0.0)],
                                _ => vec![r],
                            };
                        }
                        let start = niche_start as f64;
                        let ns = niche_variants.start.as_u32() as f64;
                        let uk = IntKind { bits, signed: false };
                        let t0 = if tk == LeafKind::Bool { self.un(Un::BoolToNum, t0) } else { t0 };
                        let (is_niche, tagged) = if relative_max == 0 {
                            let c = Operand::Num(uk.wrap_const(start));
                            let t0u = if matches!(tk, LeafKind::Int(k) if k.signed) { self.un(Un::Wrap(uk), t0) } else { t0 };
                            (self.bin(Bin::Eq, t0u, c), Operand::Num(ns))
                        } else {
                            let d = self.bin(Bin::Sub, t0, Operand::Num(start));
                            let rel = self.un(Un::Wrap(uk), d);
                            let isn = self.bin(Bin::Le, rel, Operand::Num(relative_max as f64));
                            let tagged = self.bin(Bin::Add, rel, Operand::Num(ns));
                            (isn, tagged)
                        };
                        let r = self.num(Rv::Select(is_niche, tagged, Operand::Num(untagged_variant.as_u32() as f64)));
                        match dk {
                            LeafKind::I64 { .. } => vec![r, Operand::Num(0.0)],
                            _ => vec![r],
                        }
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- arithmetic

    fn int_cast(&mut self, v: &[Operand], from: LeafKind, to: LeafKind) -> Vec<Operand> {
        match (from, to) {
            (a, b) if a == b => v.to_vec(),
            (LeafKind::Bool, _) => {
                let n = self.un(Un::BoolToNum, v[0]);
                self.int_cast(&[n], LeafKind::Int(IntKind::U8), to)
            }
            (LeafKind::Int(a), LeafKind::Int(b)) => {
                if a.min() >= b.min() && a.max() <= b.max() {
                    v.to_vec()
                } else {
                    vec![self.un(Un::Wrap(b), v[0])]
                }
            }
            // narrow or widen through u32 limbs
            _ => {
                let limbs = self.to_limbs(v, from);
                self.from_limbs(&limbs, to)
            }
        }
    }

    /// Value as little-endian u32 limbs plus a sign-extension limb.
    fn to_limbs(&mut self, v: &[Operand], k: LeafKind) -> (Vec<Operand>, Operand) {
        match k {
            LeafKind::Int(ik) => {
                let lo = if ik.signed { self.un(Un::Wrap(IntKind::U32), v[0]) } else { v[0] };
                let ext = if ik.signed {
                    let neg = self.bin(Bin::Lt, v[0], Operand::Num(0.0));
                    self.num(Rv::Select(neg, Operand::Num(4294967295.0), Operand::Num(0.0)))
                } else {
                    Operand::Num(0.0)
                };
                (vec![lo], ext)
            }
            LeafKind::I64 { signed } | LeafKind::I128 { signed } => {
                let top = *v.last().unwrap();
                let ext = if signed {
                    let neg = self.bin(Bin::Le, Operand::Num(2147483648.0), top);
                    self.num(Rv::Select(neg, Operand::Num(4294967295.0), Operand::Num(0.0)))
                } else {
                    Operand::Num(0.0)
                };
                (v.to_vec(), ext)
            }
            _ => self.fatal(format!("bad int cast from {k:?}")),
        }
    }

    fn from_limbs(&mut self, (limbs, ext): &(Vec<Operand>, Operand), k: LeafKind) -> Vec<Operand> {
        let get = |i: usize| limbs.get(i).copied().unwrap_or(*ext);
        match k {
            LeafKind::Int(ik) => {
                let lo = get(0);
                if ik.bits == 32 && !ik.signed { vec![lo] } else { vec![self.un(Un::Wrap(ik), lo)] }
            }
            LeafKind::I64 { .. } => vec![get(0), get(1)],
            LeafKind::I128 { .. } => vec![get(0), get(1), get(2), get(3)],
            _ => self.fatal(format!("bad int cast to {k:?}")),
        }
    }

    fn binop(
        &mut self,
        op: BinOp,
        a: &[Operand],
        al: TyAndLayout<'tcx>,
        b: &[Operand],
        bl: TyAndLayout<'tcx>,
        dl: TyAndLayout<'tcx>,
    ) -> Vec<Operand> {
        let ka = leaf_of(al.ty).unwrap_or(LeafKind::Int(IntKind::U32));
        if op == BinOp::Offset {
            let pointee = al.ty.builtin_deref(true).unwrap();
            let size = self.layout(pointee).size.bytes();
            let kb = leaf_of(bl.ty).unwrap();
            let n = if let LeafKind::Int(_) = kb { b[0] } else { self.fatal("offset by non-int") };
            let scaled = if size == 1 { n } else { self.bin(Bin::Mul, n, Operand::Num(size as f64)) };
            return vec![self.bin(Bin::Add, a[0], scaled)];
        }
        // fat pointer comparisons: compare (ptr, meta) lexicographically
        if a.len() == 2 && !matches!(ka, LeafKind::I64 { .. } | LeafKind::I128 { .. }) {
            return self.fat_cmp(op, a, b);
        }
        match ka {
            LeafKind::Bool => {
                let r = match op {
                    BinOp::BitAnd => self.bin(Bin::And, a[0], b[0]),
                    BinOp::BitOr => self.bin(Bin::Or, a[0], b[0]),
                    BinOp::BitXor | BinOp::Ne => self.bin(Bin::Ne, a[0], b[0]),
                    BinOp::Eq => self.bin(Bin::Eq, a[0], b[0]),
                    BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Cmp => {
                        let x = self.un(Un::BoolToNum, a[0]);
                        let y = self.un(Un::BoolToNum, b[0]);
                        return self.cmp_num(op, x, y);
                    }
                    _ => self.fatal(format!("bad bool op {op:?}")),
                };
                vec![r]
            }
            LeafKind::F32 | LeafKind::F64 => {
                let f32 = ka == LeafKind::F32;
                let r = match op {
                    BinOp::Add => self.bin(Bin::Add, a[0], b[0]),
                    BinOp::Sub => self.bin(Bin::Sub, a[0], b[0]),
                    BinOp::Mul => self.bin(Bin::Mul, a[0], b[0]),
                    BinOp::Div => self.bin(Bin::Div, a[0], b[0]),
                    BinOp::Rem => self.bin(Bin::FMod, a[0], b[0]),
                    _ => return self.cmp_num(op, a[0], b[0]),
                };
                vec![if f32 { self.un(Un::F32Round, r) } else { r }]
            }
            LeafKind::Int(k) => self.int_binop(op, k, a[0], b, bl, dl),
            LeafKind::I64 { signed } => self.i64_binop(op, signed, a, b, bl),
            LeafKind::I128 { signed } => self.i128_binop(op, signed, a, b),
        }
    }

    fn fat_cmp(&mut self, op: BinOp, a: &[Operand], b: &[Operand]) -> Vec<Operand> {
        let e0 = self.bin(Bin::Eq, a[0], b[0]);
        let e1 = self.bin(Bin::Eq, a[1], b[1]);
        match op {
            BinOp::Eq => vec![self.bin(Bin::And, e0, e1)],
            BinOp::Ne => {
                let e = self.bin(Bin::And, e0, e1);
                vec![self.un(Un::Not, e)]
            }
            _ => {
                // compare first by address, then by metadata
                let c0 = self.cmp_num(op, a[0], b[0]);
                let c1 = self.cmp_num(op, a[1], b[1]);
                vec![self.assign(l::Ty::Bool, Rv::Select(e0, c1[0], c0[0]))]
            }
        }
    }

    fn cmp_num(&mut self, op: BinOp, a: Operand, b: Operand) -> Vec<Operand> {
        vec![match op {
            BinOp::Eq => self.bin(Bin::Eq, a, b),
            BinOp::Ne => self.bin(Bin::Ne, a, b),
            BinOp::Lt => self.bin(Bin::Lt, a, b),
            BinOp::Le => self.bin(Bin::Le, a, b),
            BinOp::Gt => self.bin(Bin::Lt, b, a),
            BinOp::Ge => self.bin(Bin::Le, b, a),
            BinOp::Cmp => {
                let lt = self.bin(Bin::Lt, a, b);
                let gt = self.bin(Bin::Lt, b, a);
                let g = self.num(Rv::Select(gt, Operand::Num(1.0), Operand::Num(0.0)));
                self.num(Rv::Select(lt, Operand::Num(-1.0), g))
            }
            _ => self.fatal(format!("bad comparison {op:?}")),
        }]
    }

    fn shift_amount(&mut self, b: &[Operand], bl: TyAndLayout<'tcx>, bits: u8, masked: bool) -> Operand {
        // only the low part matters for 64-bit shift amounts
        let amt = b[0];
        let _ = bl;
        if masked {
            if let Operand::Num(n) = amt {
                return Operand::Num(((n as i64) & (bits as i64 - 1)) as f64);
            }
            self.bin(Bin::BAnd, amt, Operand::Num(bits as f64 - 1.0))
        } else {
            amt
        }
    }

    fn int_binop(&mut self, op: BinOp, k: IntKind, a: Operand, b: &[Operand], bl: TyAndLayout<'tcx>, dl: TyAndLayout<'tcx>) -> Vec<Operand> {
        let b0 = b[0];
        let wrap = |s: &mut Self, x: Operand| s.un(Un::Wrap(k), x);
        match op {
            BinOp::Add => {
                let r = self.bin(Bin::Add, a, b0);
                vec![wrap(self, r)]
            }
            BinOp::Sub => {
                let r = self.bin(Bin::Sub, a, b0);
                vec![wrap(self, r)]
            }
            BinOp::Mul => vec![self.mul_wrap(k, a, b0)],
            BinOp::AddUnchecked => vec![self.bin(Bin::Add, a, b0)],
            BinOp::SubUnchecked => vec![self.bin(Bin::Sub, a, b0)],
            BinOp::MulUnchecked => vec![self.bin(Bin::Mul, a, b0)],
            BinOp::AddWithOverflow | BinOp::SubWithOverflow | BinOp::MulWithOverflow => {
                let exact = match op {
                    BinOp::AddWithOverflow => self.bin(Bin::Add, a, b0),
                    BinOp::SubWithOverflow => self.bin(Bin::Sub, a, b0),
                    _ => self.bin(Bin::Mul, a, b0),
                };
                let wrapped = if op == BinOp::MulWithOverflow { self.mul_wrap(k, a, b0) } else { wrap(self, exact) };
                let ov = self.bin(Bin::Ne, wrapped, exact);
                let _ = dl;
                vec![wrapped, ov]
            }
            BinOp::Div => {
                if k.signed {
                    let q = self.bin(Bin::IDivTrunc, a, b0);
                    vec![wrap(self, q)]
                } else {
                    vec![self.bin(Bin::FloorDiv, a, b0)]
                }
            }
            BinOp::Rem => {
                if k.signed {
                    vec![self.bin(Bin::FMod, a, b0)]
                } else {
                    vec![self.bin(Bin::Mod, a, b0)]
                }
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let bop = match op {
                    BinOp::BitAnd => Bin::BAnd,
                    BinOp::BitOr => Bin::BOr,
                    _ => Bin::BXor,
                };
                let r = self.bin(bop, a, b0);
                if k.signed { vec![wrap(self, r)] } else { vec![r] }
            }
            BinOp::Shl | BinOp::ShlUnchecked => {
                let amt = self.shift_amount(b, bl, k.bits, op == BinOp::Shl);
                let r = self.bin(Bin::Shl, a, amt);
                if k.bits == 32 && !k.signed { vec![r] } else { vec![wrap(self, r)] }
            }
            BinOp::Shr | BinOp::ShrUnchecked => {
                let amt = self.shift_amount(b, bl, k.bits, op == BinOp::Shr);
                if k.signed {
                    let r = self.bin(Bin::Sar, a, amt);
                    vec![wrap(self, r)]
                } else {
                    vec![self.bin(Bin::Shr, a, amt)]
                }
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Cmp => self.cmp_num(op, a, b0),
            BinOp::Offset => unreachable!(),
        }
    }

    /// Low 32 bits of a*b for a, b in [0, 2^32), as straight-line exact double arithmetic.
    pub(crate) fn imul32_inline(&mut self, a: Operand, b: Operand) -> Operand {
        let a0 = self.bin(Bin::BAnd, a, Operand::Num(65535.0));
        let a1 = self.bin(Bin::Shr, a, Operand::Num(16.0));
        let b0 = self.bin(Bin::BAnd, b, Operand::Num(65535.0));
        let p0 = self.bin(Bin::Mul, a0, b); // < 2^48
        let p1 = self.bin(Bin::Mul, a1, b0); // < 2^32
        let p1 = self.bin(Bin::BAnd, p1, Operand::Num(65535.0));
        let p1 = self.bin(Bin::Mul, p1, Operand::Num(65536.0));
        let t = self.bin(Bin::Add, p0, p1);
        self.un(Un::Wrap(IntKind::U32), t)
    }

    /// Full 64-bit product (lo, hi) of 64-bit values given as u32 limbs.
    fn mul64_inline(&mut self, al: Operand, ah: Operand, bl: Operand, bh: Operand) -> Vec<Operand> {
        let a0 = self.bin(Bin::BAnd, al, Operand::Num(65535.0));
        let a1 = self.bin(Bin::Shr, al, Operand::Num(16.0));
        let b0 = self.bin(Bin::BAnd, bl, Operand::Num(65535.0));
        let b1 = self.bin(Bin::Shr, bl, Operand::Num(16.0));
        let m1 = self.bin(Bin::Mul, a0, b1);
        let m2 = self.bin(Bin::Mul, a1, b0);
        let mid = self.bin(Bin::Add, m1, m2); // < 2^33
        let midlo = self.bin(Bin::BAnd, mid, Operand::Num(65535.0));
        let p00 = self.bin(Bin::Mul, a0, b0);
        let ml = self.bin(Bin::Mul, midlo, Operand::Num(65536.0));
        let lofull = self.bin(Bin::Add, p00, ml); // < 2^33
        let lo = self.un(Un::Wrap(IntKind::U32), lofull);
        let d = self.bin(Bin::Sub, lofull, lo);
        let carry = self.bin(Bin::Mul, d, Operand::Num(1.0 / 4294967296.0));
        let mh = self.bin(Bin::Sub, mid, midlo);
        let mh = self.bin(Bin::Mul, mh, Operand::Num(1.0 / 65536.0));
        let p11 = self.bin(Bin::Mul, a1, b1);
        let h = self.bin(Bin::Add, p11, mh);
        let h = self.bin(Bin::Add, h, carry);
        let c1 = self.imul32_inline(al, bh);
        let c2 = self.imul32_inline(ah, bl);
        let h = self.bin(Bin::Add, h, c1);
        let h = self.bin(Bin::Add, h, c2);
        vec![lo, self.un(Un::Wrap(IntKind::U32), h)]
    }

    /// Population count of a u32 (SWAR, no multiply).
    pub(crate) fn popcnt_inline(&mut self, x: Operand) -> Operand {
        let t = self.bin(Bin::Shr, x, Operand::Num(1.0));
        let t = self.bin(Bin::BAnd, t, Operand::Num(0x55555555 as f64));
        let x = self.bin(Bin::Sub, x, t);
        let a = self.bin(Bin::BAnd, x, Operand::Num(0x33333333 as f64));
        let b = self.bin(Bin::Shr, x, Operand::Num(2.0));
        let b = self.bin(Bin::BAnd, b, Operand::Num(0x33333333 as f64));
        let x = self.bin(Bin::Add, a, b);
        let c = self.bin(Bin::Shr, x, Operand::Num(4.0));
        let x = self.bin(Bin::Add, x, c);
        let x = self.bin(Bin::BAnd, x, Operand::Num(0x0F0F0F0F as f64));
        let s8 = self.bin(Bin::Shr, x, Operand::Num(8.0));
        let s16 = self.bin(Bin::Shr, x, Operand::Num(16.0));
        let s24 = self.bin(Bin::Shr, x, Operand::Num(24.0));
        let t = self.bin(Bin::Add, x, s8);
        let t = self.bin(Bin::Add, t, s16);
        let t = self.bin(Bin::Add, t, s24);
        self.bin(Bin::BAnd, t, Operand::Num(63.0))
    }

    /// 64-bit shift by a variable amount n in [0, 63]: kind 0 = shl, 1 = shr, 2 = sar.
    fn shift64_var(&mut self, a: &[Operand], n: Operand, kind: u8) -> Vec<Operand> {
        let (lo, hi) = (a[0], a[1]);
        let small = self.bin(Bin::Lt, n, Operand::Num(32.0));
        let n32 = self.bin(Bin::Sub, n, Operand::Num(32.0));
        let inv = self.bin(Bin::Sub, Operand::Num(32.0), n); // 32 when n == 0: bit32 shifts give 0
        match kind {
            0 => {
                let l = self.bin(Bin::Shl, lo, n);
                let h1 = self.bin(Bin::Shl, hi, n);
                let h2 = self.bin(Bin::Shr, lo, inv);
                let h = self.bin(Bin::BOr, h1, h2);
                let hb = self.bin(Bin::Shl, lo, n32);
                vec![
                    self.num(Rv::Select(small, l, Operand::Num(0.0))),
                    self.num(Rv::Select(small, h, hb)),
                ]
            }
            _ => {
                let sar = kind == 2;
                let l1 = self.bin(Bin::Shr, lo, n);
                let l2 = self.bin(Bin::Shl, hi, inv);
                let l = self.bin(Bin::BOr, l1, l2);
                let h = self.bin(if sar { Bin::Sar } else { Bin::Shr }, hi, n);
                let lb = self.bin(if sar { Bin::Sar } else { Bin::Shr }, hi, n32);
                let hb = if sar { self.bin(Bin::Sar, hi, Operand::Num(31.0)) } else { Operand::Num(0.0) };
                vec![self.num(Rv::Select(small, l, lb)), self.num(Rv::Select(small, h, hb))]
            }
        }
    }

    pub(crate) fn i_shift64_var(&mut self, a: &[Operand], n: Operand, kind: u8) -> Vec<Operand> {
        if let Operand::Num(c) = n {
            return self.shift64_const(a, c as u32 & 63, kind, kind == 2);
        }
        self.shift64_var(a, n, kind)
    }

    fn mul_wrap(&mut self, k: IntKind, a: Operand, b: Operand) -> Operand {
        let small = |o: &Operand| matches!(o, Operand::Num(n) if n.abs() < 2097152.0);
        if k.bits <= 16 || small(&a) || small(&b) {
            let r = self.bin(Bin::Mul, a, b);
            self.un(Un::Wrap(k), r)
        } else {
            let (a, b) = if k.signed {
                (self.un(Un::Wrap(IntKind::U32), a), self.un(Un::Wrap(IntKind::U32), b))
            } else {
                (a, b)
            };
            let r = self.imul32_inline(a, b);
            if k.signed { self.un(Un::Wrap(k), r) } else { r }
        }
    }

    fn i64_binop(&mut self, op: BinOp, signed: bool, a: &[Operand], b: &[Operand], bl: TyAndLayout<'tcx>) -> Vec<Operand> {
        let s = if signed { "i" } else { "u" };
        let ab = [a[0], a[1], b[0], b.get(1).copied().unwrap_or(Operand::Num(0.0))];
        match op {
            BinOp::Add | BinOp::AddUnchecked => {
                let t = self.bin(Bin::Add, a[0], ab[2]);
                let lo = self.un(Un::Wrap(IntKind::U32), t);
                // carry = (t - lo) / 2^32, exact and branchless
                let d = self.bin(Bin::Sub, t, lo);
                let cn = self.bin(Bin::Mul, d, Operand::Num(1.0 / 4294967296.0));
                let h = self.bin(Bin::Add, a[1], ab[3]);
                let h = self.bin(Bin::Add, h, cn);
                vec![lo, self.un(Un::Wrap(IntKind::U32), h)]
            }
            BinOp::Sub | BinOp::SubUnchecked => {
                let t = self.bin(Bin::Sub, a[0], ab[2]);
                let lo = self.un(Un::Wrap(IntKind::U32), t);
                let d = self.bin(Bin::Sub, lo, t);
                let bn = self.bin(Bin::Mul, d, Operand::Num(1.0 / 4294967296.0));
                let h = self.bin(Bin::Sub, a[1], ab[3]);
                let h = self.bin(Bin::Sub, h, bn);
                vec![lo, self.un(Un::Wrap(IntKind::U32), h)]
            }
            BinOp::Mul | BinOp::MulUnchecked => self.mul64_inline(ab[0], ab[1], ab[2], ab[3]),
            BinOp::Div => self.call_helper(&format!("{s}div64"), &ab, 2),
            BinOp::Rem => self.call_helper(&format!("{s}rem64"), &ab, 2),
            BinOp::BitAnd => vec![self.bin(Bin::BAnd, a[0], b[0]), self.bin(Bin::BAnd, a[1], b[1])],
            BinOp::BitOr => vec![self.bin(Bin::BOr, a[0], b[0]), self.bin(Bin::BOr, a[1], b[1])],
            BinOp::BitXor => vec![self.bin(Bin::BXor, a[0], b[0]), self.bin(Bin::BXor, a[1], b[1])],
            BinOp::Shl | BinOp::ShlUnchecked => {
                let amt = self.shift_amount(b, bl, 64, true);
                if let Operand::Num(n) = amt {
                    return self.shift64_const(a, n as u32, 0, signed);
                }
                self.shift64_var(a, amt, 0)
            }
            BinOp::Shr | BinOp::ShrUnchecked => {
                let amt = self.shift_amount(b, bl, 64, true);
                if let Operand::Num(n) = amt {
                    return self.shift64_const(a, n as u32, if signed { 2 } else { 1 }, signed);
                }
                self.shift64_var(a, amt, if signed { 2 } else { 1 })
            }
            BinOp::AddWithOverflow | BinOp::SubWithOverflow | BinOp::MulWithOverflow => {
                let name = match op {
                    BinOp::AddWithOverflow => "addo64",
                    BinOp::SubWithOverflow => "subo64",
                    _ => "mulo64",
                };
                let mut args = ab.to_vec();
                args.push(Operand::Bool(signed));
                let r = self.call_helper(name, &args, 3);
                let ov = self.un(Un::NumToBool, r[2]);
                vec![r[0], r[1], ov]
            }
            BinOp::Eq => {
                let e0 = self.bin(Bin::Eq, a[0], b[0]);
                let e1 = self.bin(Bin::Eq, a[1], b[1]);
                vec![self.bin(Bin::And, e0, e1)]
            }
            BinOp::Ne => {
                let e0 = self.bin(Bin::Ne, a[0], b[0]);
                let e1 = self.bin(Bin::Ne, a[1], b[1]);
                vec![self.bin(Bin::Or, e0, e1)]
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Cmp => {
                // compare hi (signed for i64), then lo
                let (ahi, bhi) = if signed {
                    (self.un(Un::Wrap(IntKind::I32), a[1]), self.un(Un::Wrap(IntKind::I32), b[1]))
                } else {
                    (a[1], b[1])
                };
                let heq = self.bin(Bin::Eq, ahi, bhi);
                let ch = self.cmp_num(op, ahi, bhi)[0];
                let cl = self.cmp_num(op, a[0], b[0])[0];
                let ty = if op == BinOp::Cmp { l::Ty::Num } else { l::Ty::Bool };
                vec![self.assign(ty, Rv::Select(heq, cl, ch))]
            }
            BinOp::Offset => unreachable!(),
        }
    }

    /// 64-bit shift by a constant: kind 0 = shl, 1 = logical shr, 2 = arithmetic shr.
    fn shift64_const(&mut self, a: &[Operand], n: u32, kind: u8, _signed: bool) -> Vec<Operand> {
        let (lo, hi) = (a[0], a[1]);
        let num = |x: u32| Operand::Num(x as f64);
        if n == 0 {
            return vec![lo, hi];
        }
        match kind {
            0 if n < 32 => {
                let l = self.bin(Bin::Shl, lo, num(n));
                let h1 = self.bin(Bin::Shl, hi, num(n));
                let h2 = self.bin(Bin::Shr, lo, num(32 - n));
                vec![l, self.bin(Bin::BOr, h1, h2)]
            }
            0 => vec![Operand::Num(0.0), self.bin(Bin::Shl, lo, num(n - 32))],
            _ if n < 32 => {
                let l1 = self.bin(Bin::Shr, lo, num(n));
                let l2 = self.bin(Bin::Shl, hi, num(32 - n));
                let l = self.bin(Bin::BOr, l1, l2);
                let h = self.bin(if kind == 2 { Bin::Sar } else { Bin::Shr }, hi, num(n));
                vec![l, h]
            }
            1 => vec![self.bin(Bin::Shr, hi, num(n - 32)), Operand::Num(0.0)],
            _ => vec![self.bin(Bin::Sar, hi, num(n - 32)), self.bin(Bin::Sar, hi, num(31))],
        }
    }

    fn i128_binop(&mut self, op: BinOp, signed: bool, a: &[Operand], b: &[Operand]) -> Vec<Operand> {
        let s = if signed { "i" } else { "u" };
        let mut ab: Vec<Operand> = a.to_vec();
        match op {
            BinOp::Shl | BinOp::ShlUnchecked | BinOp::Shr | BinOp::ShrUnchecked => {
                let amt = self.bin(Bin::BAnd, b[0], Operand::Num(127.0));
                ab.push(amt);
                let name = match op {
                    BinOp::Shl | BinOp::ShlUnchecked => "shl128",
                    _ if signed => "sar128",
                    _ => "shr128",
                };
                return self.call_helper(name, &ab, 4);
            }
            _ => {}
        }
        ab.extend_from_slice(b);
        match op {
            BinOp::Add | BinOp::AddUnchecked => self.call_helper("add128", &ab, 4),
            BinOp::Sub | BinOp::SubUnchecked => self.call_helper("sub128", &ab, 4),
            BinOp::Mul | BinOp::MulUnchecked => self.call_helper("mul128", &ab, 4),
            BinOp::Div => self.call_helper(&format!("{s}div128"), &ab, 4),
            BinOp::Rem => self.call_helper(&format!("{s}rem128"), &ab, 4),
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                let bop = match op {
                    BinOp::BitAnd => Bin::BAnd,
                    BinOp::BitOr => Bin::BOr,
                    _ => Bin::BXor,
                };
                (0..4).map(|i| self.bin(bop, a[i], b[i])).collect()
            }
            BinOp::AddWithOverflow | BinOp::SubWithOverflow | BinOp::MulWithOverflow => {
                let name = match op {
                    BinOp::AddWithOverflow => "addo128",
                    BinOp::SubWithOverflow => "subo128",
                    _ => "mulo128",
                };
                ab.push(Operand::Bool(signed));
                let r = self.call_helper(name, &ab, 5);
                let ov = self.un(Un::NumToBool, r[4]);
                vec![r[0], r[1], r[2], r[3], ov]
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Cmp => {
                ab.push(Operand::Bool(signed));
                let c = self.call_helper("cmp128", &ab, 1)[0];
                vec![match op {
                    BinOp::Cmp => c,
                    _ => self.cmp_num(op, c, Operand::Num(0.0))[0],
                }]
            }
            BinOp::Offset | BinOp::Shl | BinOp::ShlUnchecked | BinOp::Shr | BinOp::ShrUnchecked => unreachable!(),
        }
    }

    // ---------------------------------------------------------------- casts

    fn cast(&mut self, kind: CastKind, op: &MOp<'tcx>, to: Ty<'tcx>) -> Val {
        let tl = self.layout(to);
        match kind {
            CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer(_), _) => {
                let from = self.monomorphize(op.ty(&self.mir.local_decls, self.tcx));
                let ty::FnDef(def, args) = *from.kind() else { self.fatal("reify of non-fndef") };
                let inst = Instance::resolve_for_fn_ptr(self.tcx, typing_env(), def, args).unwrap();
                let slot = self.cx.fn_ptr(inst);
                Val::Regs(vec![Operand::Num(slot as f64)])
            }
            CastKind::PointerCoercion(PointerCoercion::ClosureFnPointer(_), _) => {
                let from = self.monomorphize(op.ty(&self.mir.local_decls, self.tcx));
                let ty::Closure(def, args) = *from.kind() else { self.fatal("closure fn ptr of non-closure") };
                let inst = Instance::resolve_closure(self.tcx, def, args, ty::ClosureKind::FnOnce);
                let slot = self.cx.fn_ptr(inst);
                Val::Regs(vec![Operand::Num(slot as f64)])
            }
            CastKind::PointerCoercion(PointerCoercion::Unsize, _) => {
                let (v, fl) = self.operand(op);
                let Val::Regs(v) = v else { self.fatal("unsize of memory value") };
                Val::Regs(self.unsize_val(&v, fl, tl))
            }
            CastKind::PointerCoercion(_, _)
            | CastKind::PtrToPtr
            | CastKind::FnPtrToPtr
            | CastKind::PointerExposeProvenance
            | CastKind::PointerWithExposedProvenance => {
                let (v, _) = self.operand_regs(op);
                let want = abi::ltys(&self.natural_leaves(tl).unwrap()).len();
                let mut v = v;
                v.truncate(want);
                Val::Regs(v)
            }
            CastKind::Subtype => self.operand(op).0,
            CastKind::IntToInt => {
                let (v, fl) = self.operand_regs(op);
                let fk = leaf_of(fl.ty).or_else(|| self.natural_leaves(fl).map(|l| l[0].kind));
                let tk = leaf_of(tl.ty);
                let (Some(fk), Some(tk)) = (fk, tk) else {
                    self.fatal(format!("unsupported int cast {} -> {} in {}", fl.ty, tl.ty, self.instance))
                };
                Val::Regs(self.int_cast(&v, fk, tk))
            }
            CastKind::IntToFloat => {
                let (v, fl) = self.operand_regs(op);
                let fk = leaf_of(fl.ty).unwrap();
                let x = match fk {
                    LeafKind::I64 { signed } => {
                        self.call_helper(if signed { "i64tof" } else { "u64tof" }, &v, 1)[0]
                    }
                    LeafKind::I128 { signed } => {
                        let mut args = v.clone();
                        args.push(Operand::Bool(signed));
                        self.call_helper("i128tof", &args, 1)[0]
                    }
                    LeafKind::Bool => self.un(Un::BoolToNum, v[0]),
                    _ => v[0],
                };
                let r = if leaf_of(to) == Some(LeafKind::F32) { self.un(Un::F32Round, x) } else { x };
                Val::Regs(vec![r])
            }
            CastKind::FloatToInt => {
                let (v, _) = self.operand_regs(op);
                match leaf_of(to).unwrap() {
                    LeafKind::Int(k) => Val::Regs(vec![self.un(Un::FToI(k), v[0])]),
                    LeafKind::I64 { signed } => Val::Regs(self.call_helper(if signed { "ftoi64" } else { "ftou64" }, &v, 2)),
                    LeafKind::I128 { signed } => Val::Regs(self.call_helper("ftoi128", &[v[0], Operand::Bool(signed)], 4)),
                    _ => self.fatal("bad float to int"),
                }
            }
            CastKind::FloatToFloat => {
                let (v, _) = self.operand_regs(op);
                let r = if leaf_of(to) == Some(LeafKind::F32) { self.un(Un::F32Round, v[0]) } else { v[0] };
                Val::Regs(vec![r])
            }
            CastKind::Transmute => {
                let (v, fl) = self.operand(op);
                self.transmute(v, fl, tl)
            }
        }
    }

    fn transmute(&mut self, v: Val, fl: TyAndLayout<'tcx>, tl: TyAndLayout<'tcx>) -> Val {
        let fls = self.natural_leaves(fl);
        let tls = self.natural_leaves(tl);
        match (&v, fls, tls) {
            (Val::Regs(ops), Some(a), Some(b)) => {
                let same_shape = a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.off == y.off && x.kind.size() == y.kind.size());
                let simple = |k: LeafKind| matches!(k, LeafKind::Int(_) | LeafKind::Bool | LeafKind::I64 { .. } | LeafKind::I128 { .. });
                if same_shape && a.iter().zip(&b).all(|(x, y)| simple(x.kind) && simple(y.kind)) {
                    let mut out = vec![];
                    let mut i = 0;
                    for (x, y) in a.iter().zip(&b) {
                        let n = x.kind.nvars();
                        let c = self.convert_leaf_bits(x.kind, y.kind, &ops[i..i + n]);
                        out.extend(c);
                        i += n;
                    }
                    return Val::Regs(out);
                }
                let a = self.spill(v, fl);
                let lv = self.load_val(a, 0, &b);
                Val::Regs(lv)
            }
            (_, _, Some(b)) => {
                let a = self.spill(v, fl);
                Val::Regs(self.load_val(a, 0, &b))
            }
            (_, _, None) => Val::Mem(self.spill(v, fl)),
        }
    }

    fn convert_leaf_bits(&mut self, from: LeafKind, to: LeafKind, src: &[Operand]) -> Vec<Operand> {
        match (from, to) {
            (a, b) if a == b => src.to_vec(),
            (LeafKind::Int(_), LeafKind::Int(b)) => vec![self.un(Un::Wrap(b), src[0])],
            (LeafKind::Int(_), LeafKind::Bool) => vec![self.un(Un::NumToBool, src[0])],
            (LeafKind::Bool, LeafKind::Int(_)) => vec![self.un(Un::BoolToNum, src[0])],
            (LeafKind::I64 { .. }, LeafKind::I64 { .. }) | (LeafKind::I128 { .. }, LeafKind::I128 { .. }) => src.to_vec(),
            _ => self.fatal(format!("bad transmute {from:?} -> {to:?}")),
        }
    }

    fn unsize_val(&mut self, v: &[Operand], fl: TyAndLayout<'tcx>, tl: TyAndLayout<'tcx>) -> Vec<Operand> {
        let (fty, tty) = (fl.ty, tl.ty);
        match (fty.kind(), tty.kind()) {
            (ty::Ref(_, a, _) | ty::RawPtr(a, _), ty::Ref(_, b, _) | ty::RawPtr(b, _)) => {
                let (a, b) = (*a, *b);
                let old = v.get(1).copied();
                let info = self.unsized_info(a, b, old);
                vec![v[0], info]
            }
            (ty::Pat(a, _), ty::Pat(b, _)) => {
                let (al, bl) = (self.layout(*a), self.layout(*b));
                self.unsize_val(v, al, bl)
            }
            (ty::Adt(da, _), ty::Adt(db, _)) if da == db => {
                // field-wise coercion (Box, NonNull, Rc, ...)
                let mut out = vec![];
                let fleaves = self.natural_leaves(fl).unwrap();
                let mut vi = 0;
                let (offs, _) = abi::var_offsets(&fleaves);
                let _ = offs;
                for i in 0..tl.fields.count() {
                    let sf = fl.field(&self.lcx, i);
                    let tf = tl.field(&self.lcx, i);
                    let n = abi::ltys(&self.natural_leaves(sf).unwrap()).len();
                    let part = &v[vi..vi + n];
                    vi += n;
                    if sf.ty == tf.ty {
                        out.extend_from_slice(part);
                    } else {
                        out.extend(self.unsize_val(part, sf, tf));
                    }
                }
                out
            }
            _ => self.fatal(format!("unsupported unsize {fty} -> {tty}")),
        }
    }

    fn unsized_info(&mut self, source: Ty<'tcx>, target: Ty<'tcx>, old: Option<Operand>) -> Operand {
        let (source, target) = self.tcx.struct_lockstep_tails_for_codegen(source, target, typing_env());
        match (source.kind(), target.kind()) {
            (ty::Array(_, len), ty::Slice(_)) => Operand::Num(len.try_to_target_usize(self.tcx).unwrap() as f64),
            (ty::Dynamic(da, _), ty::Dynamic(db, _)) => {
                let old = old.expect("upcast without vtable");
                if da.principal_def_id() == db.principal_def_id() || db.principal_def_id().is_none() {
                    return old;
                }
                match self.tcx.supertrait_vtable_slot((source, target)) {
                    Some(idx) => {
                        let v = self.var(l::Ty::Num);
                        self.push(Stmt::Assign(v, Rv::Load { mem: Mem::U32, addr: old, off: (idx * 4) as i32 }));
                        Operand::Var(v)
                    }
                    None => old,
                }
            }
            (_, ty::Dynamic(data, _)) => {
                let tr = data.principal().map(|p| self.tcx.instantiate_bound_regions_with_erased(p));
                let id = self.tcx.vtable_allocation((source, tr));
                self.cx.vtables.insert(id, data.principal_def_id());
                self.alloc_addr(id)
            }
            _ => self.fatal(format!("unsized_info {source} -> {target}")),
        }
    }

    // ---------------------------------------------------------------- terminators

    fn goto(&mut self, bb: BasicBlock) {
        let t = self.bbs[bb.as_usize()];
        self.term(Term::Goto(t));
    }

    fn terminator(&mut self, _bb: BasicBlock, t: &mir::Terminator<'tcx>) {
        match &t.kind {
            TerminatorKind::Goto { target } => self.goto(*target),
            TerminatorKind::SwitchInt { discr, targets } if self.const_discr(discr).is_some() => {
                let v = self.const_discr(discr).unwrap();
                self.goto(targets.target_for_value(v));
            }
            TerminatorKind::SwitchInt { discr, targets } => {
                let (d, dl) = self.operand_regs(discr);
                if dl.ty.is_bool() {
                    let (v, then_bb) = targets.iter().next().unwrap();
                    let (tb, eb) = (self.bbs[then_bb.as_usize()], self.bbs[targets.otherwise().as_usize()]);
                    if v == 0 {
                        self.term(Term::Br(d[0], eb, tb));
                    } else {
                        self.term(Term::Br(d[0], tb, eb));
                    }
                } else {
                    let kind = leaf_of(dl.ty).or_else(|| self.natural_leaves(dl).map(|l| l[0].kind)).unwrap();
                    // wide int against a single value: compare limb by limb
                    if matches!(kind, LeafKind::I64 { .. } | LeafKind::I128 { .. }) && targets.iter().count() == 1 {
                        let (v, b) = targets.iter().next().unwrap();
                        let limbs = const_bits(v, kind);
                        let mut eq = Operand::Bool(true);
                        for (x, c) in d.iter().zip(limbs) {
                            let e = self.bin(Bin::Eq, *x, c);
                            eq = self.bin(Bin::And, eq, e);
                        }
                        let (tb, ob) = (self.bbs[b.as_usize()], self.bbs[targets.otherwise().as_usize()]);
                        self.term(Term::Br(eq, tb, ob));
                        return;
                    }
                    let x = match kind {
                        LeafKind::I64 { .. } | LeafKind::I128 { .. } => {
                            // switch on wide ints: values >= 2^32 never match (we use -2 for them)
                            let mut hi0 = Operand::Bool(true);
                            for limb in &d[1..] {
                                let z = self.bin(Bin::Eq, *limb, Operand::Num(0.0));
                                hi0 = self.bin(Bin::And, hi0, z);
                            }
                            self.num(Rv::Select(hi0, d[0], Operand::Num(-1.0)))
                        }
                        LeafKind::Bool => self.un(Un::BoolToNum, d[0]),
                        _ => d[0],
                    };
                    let cases: Vec<(f64, BlockId)> = targets
                        .iter()
                        .map(|(v, b)| {
                            let val = match kind {
                                LeafKind::Int(k) => const_bits(v, LeafKind::Int(k))[0].as_num().unwrap(),
                                LeafKind::I64 { .. } | LeafKind::I128 { .. } => {
                                    if v > u32::MAX as u128 {
                                        -2.0
                                    } else {
                                        v as f64
                                    }
                                }
                                _ => v as f64,
                            };
                            (val, self.bbs[b.as_usize()])
                        })
                        .collect();
                    let other = self.bbs[targets.otherwise().as_usize()];
                    if cases.len() == 1 {
                        let c = self.bin(Bin::Eq, x, Operand::Num(cases[0].0));
                        self.term(Term::Br(c, cases[0].1, other));
                    } else {
                        self.term(Term::Switch(x, cases, other));
                    }
                }
            }
            TerminatorKind::Return => {
                let ops = self.return_values();
                self.push(Stmt::SetSp(Operand::Var(self.sp_top)));
                self.term(Term::Return(ops));
            }
            TerminatorKind::Unreachable => self.term(Term::Unreachable),
            TerminatorKind::UnwindResume | TerminatorKind::UnwindTerminate(_) => self.term(Term::Unreachable),
            TerminatorKind::FalseEdge { real_target, .. } | TerminatorKind::FalseUnwind { real_target, .. } => {
                self.goto(*real_target)
            }
            TerminatorKind::Assert { cond, expected, msg, target, .. } => {
                if !self.tcx.sess.overflow_checks() && msg.is_optional_overflow_check() {
                    self.goto(*target);
                    return;
                }
                let (c, _) = self.operand_regs(cond);
                let ok = self.bbs[target.as_usize()];
                let fail = self.f.new_block();
                if *expected {
                    self.term(Term::Br(c[0], ok, fail));
                } else {
                    self.term(Term::Br(c[0], fail, ok));
                }
                self.cur = fail;
                let parts = self.assert_message(msg);
                self.term(Term::Trap(parts));
            }
            TerminatorKind::Drop { place, target, .. } => {
                let pv = self.place(*place);
                let ty = pv.layout().ty;
                let inst = Instance::resolve_drop_glue(self.tcx, ty);
                if let InstanceKind::Shim(ShimKind::DropGlue(_, None)) = inst.def {
                    self.goto(*target);
                    return;
                }
                if let ty::Dynamic(..) = ty.kind() {
                    let (ptr, vt) = self.place_addr(&pv);
                    let vt = vt.expect("dyn drop without vtable");
                    let f = self.num(Rv::Load { mem: Mem::U32, addr: vt, off: 0 });
                    let nz = self.bin(Bin::Ne, f, Operand::Num(0.0));
                    let call_b = self.f.new_block();
                    let next = self.bbs[target.as_usize()];
                    self.term(Term::Br(nz, call_b, next));
                    self.cur = call_b;
                    self.push(Stmt::Call { dests: vec![], callee: Callee::Indirect(f), args: vec![ptr] });
                    self.goto(*target);
                    return;
                }
                let addr = match &pv {
                    PlaceV::Mem { .. } => {
                        let (a, meta) = self.place_addr(&pv);
                        let mut v = vec![a];
                        v.extend(meta);
                        v
                    }
                    PlaceV::Reg { .. } => {
                        let v = self.read_place(&pv);
                        vec![self.spill(v, pv.layout())]
                    }
                };
                let id = self.cx.fn_id(inst);
                let mut args = addr;
                if inst.def.requires_caller_location(self.tcx) {
                    let loc = self.caller_location(t.source_info);
                    args.push(loc);
                }
                self.push(Stmt::Call { dests: vec![], callee: Callee::Func(id), args });
                self.goto(*target);
            }
            TerminatorKind::Call { func, args, destination, target, fn_span, .. } => {
                self.span = *fn_span;
                self.call(func, args, *destination, *target, t.source_info);
            }
            TerminatorKind::TailCall { func, args, fn_span } => {
                self.span = *fn_span;
                // lower as call into the return place followed by return
                let dest = Place::return_place();
                let ret_bb = self.f.new_block();
                let saved = self.bbs.clone();
                // fake target: a block that returns
                self.call_with_target(func, args, dest, Some(ret_bb), t.source_info);
                self.bbs = saved;
                self.cur = ret_bb;
                let ops = self.return_values();
                self.push(Stmt::SetSp(Operand::Var(self.sp_top)));
                self.term(Term::Return(ops));
            }
            TerminatorKind::InlineAsm { .. } => self.fatal("inline assembly is not supported"),
            TerminatorKind::Yield { .. } | TerminatorKind::CoroutineDrop => self.fatal("coroutines are not supported"),
        }
    }

    fn assert_message(&mut self, msg: &AssertKind<mir::Operand<'tcx>>) -> Vec<TrapPart> {
        let mut parts = vec![];
        match msg {
            AssertKind::BoundsCheck { len, index } => {
                let (l, _) = self.operand_regs(len);
                let (i, _) = self.operand_regs(index);
                parts.push(TrapPart::Str("index out of bounds: the len is ".into()));
                parts.push(TrapPart::Val(l[0]));
                parts.push(TrapPart::Str(" but the index is ".into()));
                parts.push(TrapPart::Val(i[0]));
            }
            AssertKind::Overflow(op, _, _) => {
                let what = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "subtract",
                    BinOp::Mul => "multiply",
                    BinOp::Div => "divide",
                    BinOp::Rem => "calculate the remainder",
                    BinOp::Shl => "shift left",
                    BinOp::Shr => "shift right",
                    _ => "compute",
                };
                parts.push(TrapPart::Str(format!("attempt to {what} with overflow")));
            }
            AssertKind::OverflowNeg(_) => parts.push(TrapPart::Str("attempt to negate with overflow".into())),
            AssertKind::DivisionByZero(_) => parts.push(TrapPart::Str("attempt to divide by zero".into())),
            AssertKind::RemainderByZero(_) => {
                parts.push(TrapPart::Str("attempt to calculate the remainder with a divisor of zero".into()))
            }
            AssertKind::NullPointerDereference => parts.push(TrapPart::Str("null pointer dereference occurred".into())),
            AssertKind::MisalignedPointerDereference { .. } => {
                parts.push(TrapPart::Str("misaligned pointer dereference".into()))
            }
            other => parts.push(TrapPart::Str(format!("assertion failed: {other:?}"))),
        }
        let loc = self.tcx.sess.source_map().span_to_location_info(self.span);
        if let (Some(file), line, col, _, _) = loc {
            parts.push(TrapPart::Str(format!(" at {}:{}:{}", file.name.prefer_local_unconditionally(), line, col)));
        }
        parts
    }

    fn caller_location(&mut self, si: mir::SourceInfo) -> Operand {
        enum Loc {
            Op(Operand),
            Const(ConstValue),
        }
        let tcx = self.tcx;
        let cl = self.caller_loc.map(|v| Loc::Op(Operand::Var(v)));
        match self.mir.caller_location_span(si, cl, tcx, |span| Loc::Const(tcx.span_as_caller_location(span))) {
            Loc::Op(o) => o,
            Loc::Const(c) => {
                let lay = self.layout(Ty::new_imm_ref(tcx, tcx.lifetimes.re_static, tcx.caller_location_ty()));
                match self.const_value(c, lay) {
                    Val::Regs(v) => v[0],
                    Val::Mem(a) => a,
                }
            }
        }
    }

    fn call(&mut self, func: &MOp<'tcx>, args: &[Spanned<MOp<'tcx>>], dest: Place<'tcx>, target: Option<BasicBlock>, si: mir::SourceInfo) {
        let t = target.map(|b| self.bbs[b.as_usize()]);
        self.call_with_target(func, args, dest, t, si);
    }

    fn call_with_target(
        &mut self,
        func: &MOp<'tcx>,
        args: &[Spanned<MOp<'tcx>>],
        dest: Place<'tcx>,
        target: Option<BlockId>,
        si: mir::SourceInfo,
    ) {
        let tcx = self.tcx;
        let fn_ty = self.monomorphize(func.ty(&self.mir.local_decls, tcx));
        let fn_sig = fn_ty.fn_sig(tcx);
        let finish = |s: &mut Self| match target {
            Some(b) => s.term(Term::Goto(b)),
            None => s.term(Term::Unreachable),
        };

        let mut callee_inst = None;
        if let ty::FnDef(def_id, fn_args) = *fn_ty.kind() {
            let inst = Instance::expect_resolve(tcx, typing_env(), def_id, fn_args, si.span);
            {
                let dest_pv = self.place(dest);
                if crate::platform::lower_override(self, inst, args, &dest_pv) {
                    finish(self);
                    return;
                }
            }
            match inst.def {
                InstanceKind::Intrinsic(_) => {
                    let dest_pv = self.place(dest);
                    if crate::intrinsics::lower_intrinsic(self, inst, args, &dest_pv, si) {
                        finish(self);
                        return;
                    }
                    // use the fallback body -- only if there is one: an intrinsic without a
                    // body gets a placeholder MIR of `unreachable`, which would silently make
                    // every caller dead code
                    if tcx.intrinsic(def_id).map(|i| i.must_be_overridden).unwrap_or(true) {
                        self.fatal(format!("unsupported intrinsic `{}` for {:?}", tcx.item_name(def_id), inst.args));
                    }
                    callee_inst = Some(Instance::new_raw(def_id, inst.args));
                }
                InstanceKind::Shim(ShimKind::DropGlue(_, None)) => {
                    finish(self);
                    return;
                }
                InstanceKind::Item(d) if tcx.is_foreign_item(d) => {
                    let name = tcx.symbol_name(inst).name.to_string();
                    if crate::intrinsics::lower_import(self, &name, args, dest) {
                        finish(self);
                        return;
                    }
                    if name == rustc_symbol_mangling::mangle_internal_symbol(tcx, "__rust_no_alloc_shim_is_unstable_v2") {
                        finish(self);
                        return;
                    }
                    // default allocator (no #[global_allocator]): the Luau runtime heap
                    if self.cx.resolve_symbol(&name).is_none() {
                        let item = tcx.item_name(d);
                        let rt = match item.as_str() {
                            "__rust_alloc" => Some(("rt_alloc", 1)),
                            "__rust_alloc_zeroed" => Some(("rt_alloc_zeroed", 1)),
                            "__rust_dealloc" => Some(("rt_dealloc", 0)),
                            "__rust_realloc" => Some(("rt_realloc", 1)),
                            _ => None,
                        };
                        if let Some((h, nret)) = rt {
                            let mut flat = vec![];
                            for a in args {
                                let (v, _) = self.operand_regs(&a.node);
                                flat.extend(v);
                            }
                            let r = self.call_helper(h, &flat, nret);
                            if nret > 0 {
                                let dp = self.place(dest);
                                self.write_place(&dp, Val::Regs(r));
                            }
                            finish(self);
                            return;
                        }
                    }
                    let resolved = self.cx.resolve_symbol(&name).or_else(|| {
                        // allocator shim: __rust_X falls back to the default __rdl_X
                        let item = tcx.item_name(d);
                        let base = item.as_str().strip_prefix("__rust_")?;
                        let alt = rustc_symbol_mangling::mangle_internal_symbol(tcx, &format!("__rdl_{base}"));
                        self.cx.resolve_symbol(&alt)
                    });
                    match resolved {
                        Some(def) => callee_inst = Some(def),
                        None => self.fatal(format!("unresolved external function `{name}`")),
                    }
                }
                _ => callee_inst = Some(inst),
            }
        }

        let fn_abi: &FnAbi<'tcx, Ty<'tcx>> = match callee_inst {
            Some(i) => self.lcx.fn_abi_of_instance(i, ty::List::empty()),
            None => self.lcx.fn_abi_of_fn_ptr(fn_sig, ty::List::empty()),
        };

        // flatten arguments (untupling rust-call)
        let mut vals: Vec<(Val, TyAndLayout<'tcx>, bool)> = vec![];
        let rust_call = fn_sig.abi() == rustc_abi::ExternAbi::RustCall;
        let n = args.len();
        for (i, a) in args.iter().enumerate() {
            let owned = matches!(a.node, MOp::Move(_));
            if rust_call && i + 1 == n {
                // spread the tuple
                let (v, tl) = self.operand(&a.node);
                match v {
                    Val::Regs(ops) => {
                        let leaves = self.natural_leaves(tl).unwrap();
                        for fi in 0..tl.fields.count() {
                            let fl = tl.field(&self.lcx, fi);
                            let foff = tl.fields.offset(fi).bytes();
                            let fls = self.natural_leaves(fl);
                            match fls {
                                Some(_) => {
                                    let mut part = vec![];
                                    let (offs, _) = abi::var_offsets(&leaves);
                                    for (li, lf) in leaves.iter().enumerate() {
                                        if lf.off >= foff && lf.off < foff + fl.size.bytes() {
                                            for k in 0..lf.kind.nvars() {
                                                part.push(ops[offs[li] + k]);
                                            }
                                        }
                                    }
                                    vals.push((Val::Regs(part), fl, false));
                                }
                                None => unreachable!(),
                            }
                        }
                    }
                    Val::Mem(addr) => {
                        for fi in 0..tl.fields.count() {
                            let fl = tl.field(&self.lcx, fi);
                            let foff = tl.fields.offset(fi).bytes() as i64;
                            let p = PlaceV::Mem { addr, off: foff, layout: fl, meta: None };
                            let v = self.read_place(&p);
                            vals.push((v, fl, owned));
                        }
                    }
                }
            } else {
                let (v, lay) = self.operand(&a.node);
                vals.push((v, lay, owned));
            }
        }

        let mut call_args: Vec<Operand> = vec![];
        let dest_pv = self.place(dest);
        let ret_leaves = abi::leaves(self.lcx, fn_abi.ret.layout);
        if ret_leaves.is_none() {
            let (a, _) = match &dest_pv {
                PlaceV::Mem { .. } => self.place_addr(&dest_pv),
                PlaceV::Reg { .. } => self.fatal("indirect return into register place"),
            };
            call_args.push(a);
        }

        let mut callee = None;
        let mut skip_first = false;
        if let Some(Instance { def: InstanceKind::Virtual(_, idx), .. }) = callee_inst {
            match find_dyn(tcx, vals[0].1.ty) {
                Some(p) => {
                    self.cx.used_slots.insert((p, idx));
                }
                None => {
                    self.cx.any_slot.insert(idx);
                }
            }
            let (v, _, _) = &vals[0];
            let Val::Regs(ops) = v else { self.fatal("virtual call on memory receiver") };
            let (data, vt) = (ops[0], ops[ops.len() - 1]);
            let f = self.num(Rv::Load { mem: Mem::U32, addr: vt, off: (idx * 4) as i32 });
            callee = Some(Callee::Indirect(f));
            call_args.push(data);
            skip_first = true;
        }

        for (i, (v, lay, owned)) in vals.into_iter().enumerate() {
            if skip_first && i == 0 {
                continue;
            }
            let arg_layout = fn_abi.args.get(i).map(|a| a.layout).unwrap_or(lay);
            match abi::leaves(self.lcx, arg_layout) {
                Some(_) => match v {
                    Val::Regs(ops) => call_args.extend(ops),
                    Val::Mem(a) => {
                        let leaves = self.natural_leaves(arg_layout).unwrap();
                        let ops = self.load_val(a, 0, &leaves);
                        call_args.extend(ops);
                    }
                },
                None => match v {
                    Val::Mem(a) if owned => call_args.push(a),
                    v => {
                        // copy into a fresh slot owned by the callee
                        let slot = self.temp_slot(arg_layout);
                        let p = PlaceV::Mem { addr: slot, off: 0, layout: arg_layout, meta: None };
                        self.write_place(&p, v);
                        call_args.push(slot);
                    }
                },
            }
        }

        if let Some(inst) = callee_inst {
            if inst.def.requires_caller_location(tcx) {
                let loc = self.caller_location(si);
                call_args.push(loc);
            }
        }

        let callee = match callee {
            Some(c) => c,
            None => match callee_inst {
                Some(inst) => Callee::Func(self.cx.fn_id(inst)),
                None => {
                    let (f, _) = self.operand_regs(func);
                    Callee::Indirect(f[0])
                }
            },
        };

        match ret_leaves {
            Some(rl) if target.is_some() => {
                let dests: Vec<VarId> = self.leaf_vars(&rl);
                self.push(Stmt::Call { dests: dests.clone(), callee, args: call_args });
                if !dests.is_empty() {
                    let ops = dests.into_iter().map(Operand::Var).collect();
                    self.write_place(&dest_pv, Val::Regs(ops));
                }
            }
            _ => self.push(Stmt::Call { dests: vec![], callee, args: call_args }),
        }
        finish(self);
    }

    // accessors used by intrinsics.rs
    pub(crate) fn i_operand(&mut self, op: &MOp<'tcx>) -> (Vec<Operand>, TyAndLayout<'tcx>) {
        self.operand_regs(op)
    }
    pub(crate) fn i_operand_val(&mut self, op: &MOp<'tcx>) -> (Val, TyAndLayout<'tcx>) {
        self.operand(op)
    }
    pub(crate) fn i_write(&mut self, dest: &PlaceV<'tcx>, ops: Vec<Operand>) {
        self.write_place(dest, Val::Regs(ops));
    }
    pub(crate) fn i_write_val(&mut self, dest: &PlaceV<'tcx>, v: Val) {
        self.write_place(dest, v);
    }
    pub(crate) fn i_place(&mut self, p: Place<'tcx>) -> PlaceV<'tcx> {
        self.place(p)
    }
    pub(crate) fn i_layout(&self, t: Ty<'tcx>) -> TyAndLayout<'tcx> {
        self.layout(t)
    }
    pub(crate) fn i_tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }
    pub(crate) fn i_bin(&mut self, op: Bin, a: Operand, b: Operand) -> Operand {
        self.bin(op, a, b)
    }
    pub(crate) fn i_un(&mut self, op: Un, a: Operand) -> Operand {
        self.un(op, a)
    }
    pub(crate) fn i_push(&mut self, s: Stmt) {
        self.push(s)
    }
    pub(crate) fn i_term_trap(&mut self, parts: Vec<TrapPart>) {
        self.term(Term::Trap(parts));
        // following code (if any) is unreachable; continue in a fresh block
        let b = self.f.new_block();
        self.cur = b;
    }
    pub(crate) fn i_assign(&mut self, ty: l::Ty, rv: Rv) -> Operand {
        self.assign(ty, rv)
    }
    pub(crate) fn i_helper(&mut self, name: &str, args: &[Operand], n: usize) -> Vec<Operand> {
        self.call_helper(name, args, n)
    }
    pub(crate) fn i_caller_location(&mut self, si: mir::SourceInfo) -> Operand {
        self.caller_location(si)
    }
    pub(crate) fn i_fatal(&self, msg: impl Into<String>) -> ! {
        self.fatal(msg)
    }
    pub(crate) fn i_monomorphize<T: ty::TypeFoldable<TyCtxt<'tcx>> + Copy>(&self, v: T) -> T {
        self.monomorphize(v)
    }
    pub(crate) fn i_set_unreachable(&mut self) {
        self.term(Term::Unreachable);
        let b = self.f.new_block();
        self.cur = b;
    }
    pub(crate) fn i_const_value(&mut self, v: ConstValue, lay: TyAndLayout<'tcx>) -> Val {
        self.const_value(v, lay)
    }
    pub(crate) fn i_spill(&mut self, v: Val, lay: TyAndLayout<'tcx>) -> Operand {
        self.spill(v, lay)
    }
    pub(crate) fn i_load(&mut self, addr: Operand, lay: TyAndLayout<'tcx>) -> Val {
        let p = PlaceV::Mem { addr, off: 0, layout: lay, meta: None };
        self.read_place(&p)
    }
    pub(crate) fn i_store(&mut self, addr: Operand, lay: TyAndLayout<'tcx>, v: Val) {
        let p = PlaceV::Mem { addr, off: 0, layout: lay, meta: None };
        self.write_place(&p, v)
    }
    pub(crate) fn i_fn_ptr(&mut self, inst: Instance<'tcx>) -> u32 {
        self.cx.fn_ptr(inst)
    }
    pub(crate) fn i_call_fn(&mut self, inst: Instance<'tcx>, args: Vec<Operand>, nret: Vec<l::Ty>) -> Vec<Operand> {
        let id = self.cx.fn_id(inst);
        let dests: Vec<VarId> = nret.into_iter().map(|t| self.var(t)).collect();
        self.push(Stmt::Call { dests: dests.clone(), callee: Callee::Func(id), args });
        dests.into_iter().map(Operand::Var).collect()
    }
    pub(crate) fn i_call_indirect(&mut self, f: Operand, args: Vec<Operand>, nret: Vec<l::Ty>) -> Vec<Operand> {
        let dests: Vec<VarId> = nret.into_iter().map(|t| self.var(t)).collect();
        self.push(Stmt::Call { dests: dests.clone(), callee: Callee::Indirect(f), args });
        dests.into_iter().map(Operand::Var).collect()
    }
    pub(crate) fn i_lcx(&self) -> LCx<'tcx> {
        self.lcx
    }
    pub(crate) fn i_write_variant(&mut self, dest: &PlaceV<'tcx>, v: VariantIdx, vals: Vec<Operand>) {
        let d = self.downcast(dest, v);
        if !vals.is_empty() {
            let fp = self.field(&d, FieldIdx::ZERO);
            self.write_place(&fp, Val::Regs(vals));
        }
        self.set_discriminant(dest, v);
    }
    pub(crate) fn i_param(&self, i: usize) -> Vec<Operand> {
        let (vars, _) = self.local_leaves(Local::from_usize(i + 1));
        vars.iter().map(|v| Operand::Var(*v)).collect()
    }
    pub(crate) fn i_temp_slot(&mut self, l: TyAndLayout<'tcx>) -> Operand {
        self.temp_slot(l)
    }
    pub(crate) fn i_leaf_tys(&self, l: TyAndLayout<'tcx>) -> Vec<l::Ty> {
        abi::ltys(&abi::leaves(self.lcx, l).unwrap_or_default())
    }
    pub(crate) fn i_rvalue(&mut self, dest: &PlaceV<'tcx>, rv: &Rvalue<'tcx>) {
        self.rvalue(dest, rv)
    }
    pub(crate) fn i_mem_place(&self, addr: Operand, layout: TyAndLayout<'tcx>) -> PlaceV<'tcx> {
        PlaceV::Mem { addr, off: 0, layout, meta: None }
    }
    pub(crate) fn i_discriminant(&mut self, pv: &PlaceV<'tcx>, dl: TyAndLayout<'tcx>) -> Vec<Operand> {
        self.get_discriminant(pv, dl)
    }
    pub(crate) fn i_binop_pub(
        &mut self,
        op: BinOp,
        a: &[Operand],
        al: TyAndLayout<'tcx>,
        b: &[Operand],
        bl: TyAndLayout<'tcx>,
        dl: TyAndLayout<'tcx>,
    ) -> Vec<Operand> {
        self.binop(op, a, al, b, bl, dl)
    }
}

pub(crate) use self::PlaceV as PlaceVal;
pub(crate) use self::Val as Value;

/// Leaf kind of a primitive-ish type (ints, floats, bool, char, thin pointers).
pub fn leaf_of(ty: Ty<'_>) -> Option<LeafKind> {
    if let Some(k) = abi::prim_kind(ty) {
        return Some(k);
    }
    match ty.kind() {
        ty::Pat(base, _) => leaf_of(*base),
        ty::RawPtr(..) | ty::Ref(..) | ty::FnPtr(..) => Some(LeafKind::Int(IntKind::U32)),
        _ => None,
    }
}

/// Constant from raw bits.
pub fn const_bits(bits: u128, kind: LeafKind) -> Vec<Operand> {
    match kind {
        LeafKind::Bool => vec![Operand::Bool(bits != 0)],
        LeafKind::Int(k) => {
            let mask = if k.bits == 128 { u128::MAX } else { (1u128 << k.bits) - 1 };
            let u = bits & mask;
            let v = if k.signed && u >= (1u128 << (k.bits - 1)) { u as f64 - k.modulus() } else { u as f64 };
            vec![Operand::Num(v)]
        }
        LeafKind::I64 { .. } => vec![Operand::Num((bits & 0xFFFF_FFFF) as f64), Operand::Num(((bits >> 32) & 0xFFFF_FFFF) as f64)],
        LeafKind::I128 { .. } => (0..4).map(|i| Operand::Num(((bits >> (32 * i)) & 0xFFFF_FFFF) as f64)).collect(),
        LeafKind::F32 => vec![Operand::Num(f32::from_bits(bits as u32) as f64)],
        LeafKind::F64 => vec![Operand::Num(f64::from_bits(bits as u64))],
    }
}

/// Fold an rvalue whose operands are constants.
fn fold(rv: &Rv) -> Option<Operand> {
    use Operand::Num as N;
    Some(match rv {
        Rv::Bin(op, N(a), N(b)) => {
            let (a, b) = (*a, *b);
            match op {
                Bin::Add => N(a + b),
                Bin::Sub => N(a - b),
                Bin::Mul if (a * b).abs() < 9007199254740992.0 => N(a * b),
                Bin::Eq => Operand::Bool(a == b),
                Bin::Ne => Operand::Bool(a != b),
                Bin::Lt => Operand::Bool(a < b),
                Bin::Le => Operand::Bool(a <= b),
                Bin::BAnd if a >= 0.0 && b >= 0.0 && a < 4294967296.0 && b < 4294967296.0 => N(((a as u32) & (b as u32)) as f64),
                Bin::BOr if a >= 0.0 && b >= 0.0 && a < 4294967296.0 && b < 4294967296.0 => N(((a as u32) | (b as u32)) as f64),
                _ => return None,
            }
        }
        Rv::Bin(Bin::Add, Operand::Var(_), N(z)) | Rv::Bin(Bin::Sub, Operand::Var(_), N(z)) if *z == 0.0 => {
            if let Rv::Bin(_, v, _) = rv {
                *v
            } else {
                unreachable!()
            }
        }
        Rv::Un(Un::Wrap(k), N(a)) => N(k.wrap_const(*a)),
        Rv::Un(Un::Neg, N(a)) => N(-a),
        Rv::Un(Un::Not, Operand::Bool(b)) => Operand::Bool(!b),
        Rv::Un(Un::BoolToNum, Operand::Bool(b)) => N(if *b { 1.0 } else { 0.0 }),
        Rv::Select(Operand::Bool(c), a, b) => {
            if *c {
                *a
            } else {
                *b
            }
        }
        _ => return None,
    })
}

trait TyOf {
    fn ty_of(&self, o: &Operand) -> l::Ty;
}
impl TyOf for l::Function {
    fn ty_of(&self, o: &Operand) -> l::Ty {
        match o {
            Operand::Var(v) => self.ty(*v),
            Operand::Num(_) => l::Ty::Num,
            Operand::Bool(_) => l::Ty::Bool,
        }
    }
}


/// The principal trait of the `dyn Trait` a receiver type points to (through references,
/// raw pointers and wrapper structs like `Box`/`Pin`/`Rc`).
fn find_dyn<'tcx>(tcx: TyCtxt<'tcx>, t: Ty<'tcx>) -> Option<Option<rustc_hir::def_id::DefId>> {
    let mut t = t;
    for _ in 0..8 {
        match t.kind() {
            ty::Dynamic(data, _) => return Some(data.principal_def_id()),
            ty::Ref(_, p, _) | ty::RawPtr(p, _) => t = *p,
            ty::Adt(def, args) if def.is_struct() => {
                // follow the first field that can hold the pointer
                let f = def.non_enum_variant().fields.iter().find_map(|f| {
                    let ft = tcx.normalize_erasing_regions(ty::TypingEnv::fully_monomorphized(), f.ty(tcx, args));
                    if ft.is_phantom_data() { None } else { Some(ft) }
                })?;
                t = f;
            }
            ty::Pat(b, _) => t = *b,
            _ => return None,
        }
    }
    None
}
