//! How Rust values map onto Luau values ("leaves").
//!
//! A type whose layout we can split into a handful of scalars is kept in Luau locals: one
//! local per leaf (two for 64-bit integers). Everything else lives in linear memory and is
//! handled by address.

use rustc_abi::{BackendRepr, FieldIdx, Primitive, Scalar, Size, Variants};
use rustc_middle::ty::layout::{
    FnAbiError, FnAbiOfHelpers, FnAbiRequest, HasTyCtxt, HasTypingEnv, LayoutError, LayoutOfHelpers, TyAndLayout,
};
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::Span;
use rustc_target::spec::{HasTargetSpec, Target};

use luau_lir::ir::{IntKind, Mem, Ty as LTy};

pub const MAX_LEAVES: usize = 12;

#[derive(Clone, Copy)]
pub struct LCx<'tcx>(pub TyCtxt<'tcx>);

impl<'tcx> LayoutOfHelpers<'tcx> for LCx<'tcx> {
    fn handle_layout_err(&self, err: LayoutError<'tcx>, span: Span, ty: Ty<'tcx>) -> ! {
        self.0.sess.dcx().span_fatal(span, format!("failed to get layout for `{ty}`: {err}"))
    }
}
impl<'tcx> FnAbiOfHelpers<'tcx> for LCx<'tcx> {
    fn handle_fn_abi_err(&self, err: FnAbiError<'tcx>, span: Span, req: FnAbiRequest<'tcx>) -> ! {
        self.0.sess.dcx().span_fatal(span, format!("fn abi error {err:?} for {req:?}"))
    }
}
impl<'tcx> HasTyCtxt<'tcx> for LCx<'tcx> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.0
    }
}
impl<'tcx> rustc_abi::HasDataLayout for LCx<'tcx> {
    fn data_layout(&self) -> &rustc_abi::TargetDataLayout {
        &self.0.data_layout
    }
}
impl<'tcx> HasTypingEnv<'tcx> for LCx<'tcx> {
    fn typing_env(&self) -> ty::TypingEnv<'tcx> {
        ty::TypingEnv::fully_monomorphized()
    }
}
impl<'tcx> HasTargetSpec for LCx<'tcx> {
    fn target_spec(&self) -> &Target {
        &self.0.sess.target
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeafKind {
    Bool,
    Int(IntKind),
    /// 64-bit integer as (lo, hi) u32 pair.
    I64 { signed: bool },
    /// 128-bit integer as four u32 limbs, least significant first.
    I128 { signed: bool },
    F32,
    F64,
}

impl LeafKind {
    pub fn nvars(self) -> usize {
        match self {
            LeafKind::I64 { .. } => 2,
            LeafKind::I128 { .. } => 4,
            _ => 1,
        }
    }
    pub fn lty(self) -> LTy {
        match self {
            LeafKind::Bool => LTy::Bool,
            _ => LTy::Num,
        }
    }
    pub fn size(self) -> u64 {
        match self {
            LeafKind::Bool => 1,
            LeafKind::Int(k) => k.bits as u64 / 8,
            LeafKind::I64 { .. } => 8,
            LeafKind::I128 { .. } => 16,
            LeafKind::F32 => 4,
            LeafKind::F64 => 8,
        }
    }
    /// Memory kinds of each var part, with byte offset relative to the leaf.
    pub fn mem_parts(self) -> Vec<(Mem, i32)> {
        match self {
            LeafKind::Bool => vec![(Mem::Bool, 0)],
            LeafKind::Int(k) => vec![(Mem::for_int(k), 0)],
            LeafKind::I64 { .. } => vec![(Mem::U32, 0), (Mem::U32, 4)],
            LeafKind::I128 { .. } => vec![(Mem::U32, 0), (Mem::U32, 4), (Mem::U32, 8), (Mem::U32, 12)],
            LeafKind::F32 => vec![(Mem::F32, 0)],
            LeafKind::F64 => vec![(Mem::F64, 0)],
        }
    }
    pub fn int(self) -> Option<IntKind> {
        match self {
            LeafKind::Int(k) => Some(k),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub off: u64,
    pub kind: LeafKind,
}

pub fn int_kind(bits: u64, signed: bool) -> Option<LeafKind> {
    Some(match bits {
        8 | 16 | 32 => LeafKind::Int(IntKind { bits: bits as u8, signed }),
        64 => LeafKind::I64 { signed },
        128 => LeafKind::I128 { signed },
        _ => return None,
    })
}

pub fn scalar_kind(cx: LCx<'_>, s: Scalar) -> Option<LeafKind> {
    match s.primitive() {
        Primitive::Int(i, signed) => int_kind(i.size().bits(), signed),
        Primitive::Float(f) => match f.size().bits() {
            32 => Some(LeafKind::F32),
            64 => Some(LeafKind::F64),
            _ => None,
        },
        Primitive::Pointer(_) => {
            let _ = cx;
            Some(LeafKind::Int(IntKind::U32))
        }
    }
}

/// Natural leaf kind of a scalar Rust type.
pub fn prim_kind<'tcx>(ty: Ty<'tcx>) -> Option<LeafKind> {
    use rustc_middle::ty::{FloatTy, IntTy, UintTy};
    Some(match ty.kind() {
        ty::Bool => LeafKind::Bool,
        ty::Char => LeafKind::Int(IntKind::U32),
        ty::Int(i) => match i {
            IntTy::I8 => LeafKind::Int(IntKind::I8),
            IntTy::I16 => LeafKind::Int(IntKind::I16),
            IntTy::I32 | IntTy::Isize => LeafKind::Int(IntKind::I32),
            IntTy::I64 => LeafKind::I64 { signed: true },
            IntTy::I128 => LeafKind::I128 { signed: true },
        },
        ty::Uint(u) => match u {
            UintTy::U8 => LeafKind::Int(IntKind::U8),
            UintTy::U16 => LeafKind::Int(IntKind::U16),
            UintTy::U32 | UintTy::Usize => LeafKind::Int(IntKind::U32),
            UintTy::U64 => LeafKind::I64 { signed: false },
            UintTy::U128 => LeafKind::I128 { signed: false },
        },
        ty::Pat(base, _) => return prim_kind(*base),
        ty::Float(f) => match f {
            FloatTy::F32 => LeafKind::F32,
            FloatTy::F64 => LeafKind::F64,
            _ => return None,
        },
        _ => return None,
    })
}

/// Split a layout into register leaves, or `None` if it must live in memory.
pub fn leaves<'tcx>(cx: LCx<'tcx>, layout: TyAndLayout<'tcx>) -> Option<Vec<Leaf>> {
    let mut out = Vec::new();
    if collect(cx, layout, 0, &mut out) && out.len() <= MAX_LEAVES {
        out.sort_by_key(|l| l.off);
        Some(out)
    } else {
        None
    }
}

fn collect<'tcx>(cx: LCx<'tcx>, layout: TyAndLayout<'tcx>, base: u64, out: &mut Vec<Leaf>) -> bool {
    if layout.is_uninhabited() && layout.size.bytes() == 0 {
        return true;
    }
    if layout.size.bytes() == 0 {
        return true;
    }
    if let Some(k) = prim_kind(layout.ty) {
        out.push(Leaf { off: base, kind: k });
        return true;
    }
    match layout.ty.kind() {
        ty::Adt(def, _) if def.is_union() => false,
        ty::Adt(..) | ty::Tuple(_) | ty::Closure(..) => {
            if !matches!(layout.variants, Variants::Single { .. }) {
                return scalar_layout(cx, layout, base, out);
            }
            for i in 0..layout.fields.count() {
                let f = layout.field(&cx, i);
                let off = layout.fields.offset(i).bytes();
                if !collect(cx, f, base + off, out) {
                    return false;
                }
            }
            true
        }
        ty::Array(..) => false,
        _ => scalar_layout(cx, layout, base, out),
    }
}

fn scalar_layout<'tcx>(cx: LCx<'tcx>, layout: TyAndLayout<'tcx>, base: u64, out: &mut Vec<Leaf>) -> bool {
    match layout.backend_repr {
        BackendRepr::Scalar(s) => match scalar_kind(cx, s) {
            Some(k) => {
                out.push(Leaf { off: base, kind: k });
                true
            }
            None => false,
        },
        BackendRepr::ScalarPair(a, b) => {
            let b_off = a.size(&cx).align_to(b.align(&cx).abi).bytes();
            match (scalar_kind(cx, a), scalar_kind(cx, b)) {
                (Some(ka), Some(kb)) => {
                    out.push(Leaf { off: base, kind: ka });
                    out.push(Leaf { off: base + b_off, kind: kb });
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Leaf range (indices into a leaves vector) of field `i` of a register-represented value.
/// Works for structs (flattened in field order) and for variant fields of scalar enums
/// (matched by byte offset).
pub fn field_leaf_range<'tcx>(
    cx: LCx<'tcx>,
    parent: TyAndLayout<'tcx>,
    parent_leaves: &[Leaf],
    variant: Option<rustc_abi::VariantIdx>,
    field: FieldIdx,
) -> (std::ops::Range<usize>, TyAndLayout<'tcx>) {
    let vlayout = match variant {
        Some(v) => parent.for_variant(&cx, v),
        None => parent,
    };
    let f = vlayout.field(&cx, field.as_usize());
    let off = vlayout.fields.offset(field.as_usize());
    range_at(parent_leaves, off, f.size, f)
}

fn range_at<'tcx>(
    leaves: &[Leaf],
    off: Size,
    size: Size,
    f: TyAndLayout<'tcx>,
) -> (std::ops::Range<usize>, TyAndLayout<'tcx>) {
    let (lo, hi) = (off.bytes(), off.bytes() + size.bytes());
    let start = leaves.iter().position(|l| l.off >= lo && l.off < hi);
    match start {
        None => (0..0, f),
        Some(s) => {
            let mut e = s;
            while e < leaves.len() && leaves[e].off < hi {
                e += 1;
            }
            (s..e, f)
        }
    }
}

/// Starting var index of each leaf, plus total var count.
pub fn var_offsets(leaves: &[Leaf]) -> (Vec<usize>, usize) {
    let mut v = Vec::with_capacity(leaves.len());
    let mut n = 0;
    for l in leaves {
        v.push(n);
        n += l.kind.nvars();
    }
    (v, n)
}

/// Declared integer range of each var of the leaves (None for floats/bools).
pub fn ranges(leaves: &[Leaf]) -> Vec<Option<(f64, f64)>> {
    let mut v = Vec::new();
    for l in leaves {
        match l.kind {
            LeafKind::Int(k) => v.push(Some((k.min(), k.max()))),
            LeafKind::I64 { .. } => v.extend([Some((0.0, 4294967295.0)); 2]),
            LeafKind::I128 { .. } => v.extend([Some((0.0, 4294967295.0)); 4]),
            _ => v.push(None),
        }
    }
    v
}

pub fn ltys(leaves: &[Leaf]) -> Vec<LTy> {
    let mut v = Vec::new();
    for l in leaves {
        for _ in 0..l.kind.nvars() {
            v.push(l.kind.lty());
        }
    }
    v
}
