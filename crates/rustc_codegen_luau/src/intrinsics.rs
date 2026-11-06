//! Compiler intrinsics and runtime imports (`extern "C" { fn __luau_*(..); }`).

use rustc_middle::mir::{self, Operand as MOp, Place};
use rustc_middle::ty::{self, Instance};
use rustc_span::Spanned;
use rustc_span::sym;

use luau_lir::ir::{self as l, Bin, IntKind, Mem, Operand, Rv, Stmt, Un};

use crate::abi::{self, LeafKind};
use crate::lower::{FnLower, PlaceVal, Value, leaf_of};

type Args<'a, 'tcx> = &'a [Spanned<MOp<'tcx>>];

/// Returns true if the intrinsic was lowered; false to fall back to its MIR body.
pub fn lower_intrinsic<'tcx>(
    fx: &mut FnLower<'_, 'tcx>,
    inst: Instance<'tcx>,
    args: Args<'_, 'tcx>,
    dest: &PlaceVal<'tcx>,
    si: mir::SourceInfo,
) -> bool {
    let tcx = fx.i_tcx();
    let name = tcx.item_name(inst.def_id());
    let name_s = name.as_str().to_string();
    let n = name_s.as_str();
    let tyarg = |i: usize| inst.args.type_at(i);

    macro_rules! arg {
        ($i:expr) => {
            fx.i_operand(&args[$i].node).0
        };
    }

    match n {
        "abort" => {
            fx.i_helper("rt_abort", &[], 0);
            fx.i_set_unreachable();
        }
        "unreachable" => fx.i_set_unreachable(),
        "cold_path" | "breakpoint" | "assert_inhabited" | "assert_zero_valid" | "assert_mem_uninitialized_valid" => {}
        "likely" | "unlikely" | "black_box" => {
            let (v, _) = fx.i_operand_val(&args[0].node);
            fx.i_write_val(dest, v);
        }
        "ub_checks" | "contract_checks" | "is_val_statically_known" => fx.i_write(dest, vec![Operand::Bool(false)]),
        "caller_location" => {
            let l = fx.i_caller_location(si);
            fx.i_write(dest, vec![l]);
        }
        "size_of_val" | "align_of_val" => {
            let (p, _) = fx.i_operand(&args[0].node);
            let t = tyarg(0);
            let lay = fx.i_layout(t);
            let r = if lay.is_sized() {
                Operand::Num(if n == "size_of_val" { lay.size.bytes() } else { lay.align.bytes() } as f64)
            } else {
                match t.kind() {
                    ty::Slice(e) | ty::Array(e, _) => {
                        let el = fx.i_layout(*e);
                        if n == "size_of_val" {
                            fx.i_bin(Bin::Mul, p[1], Operand::Num(el.size.bytes() as f64))
                        } else {
                            Operand::Num(el.align.bytes() as f64)
                        }
                    }
                    ty::Str => {
                        if n == "size_of_val" {
                            p[1]
                        } else {
                            Operand::Num(1.0)
                        }
                    }
                    ty::Dynamic(..) => {
                        let off = if n == "size_of_val" { 4 } else { 8 };
                        fx.i_assign(l::Ty::Num, Rv::Load { mem: Mem::U32, addr: p[1], off })
                    }
                    _ => {
                        let (size, align) = dst_size_align(fx, t, p[1]);
                        if n == "size_of_val" { size } else { align }
                    }
                }
            };
            fx.i_write(dest, vec![r]);
        }
        "vtable_size" | "vtable_align" => {
            let p = arg!(0);
            let off = if n == "vtable_size" { 4 } else { 8 };
            let r = fx.i_assign(l::Ty::Num, Rv::Load { mem: Mem::U32, addr: p[0], off });
            fx.i_write(dest, vec![r]);
        }
        "copy_nonoverlapping" | "copy" | "volatile_copy_memory" | "volatile_copy_nonoverlapping_memory" => {
            let (s, d) = if n == "copy" || n == "copy_nonoverlapping" { (0, 1) } else { (1, 0) };
            let src = arg!(s);
            let dst = arg!(d);
            let count = arg!(2);
            let size = fx.i_layout(tyarg(0)).size.bytes();
            let len = if size == 1 { count[0] } else { fx.i_bin(Bin::Mul, count[0], Operand::Num(size as f64)) };
            fx.i_push(Stmt::Copy { dst: dst[0], src: src[0], len });
        }
        "write_bytes" | "volatile_set_memory" => {
            let dst = arg!(0);
            let val = arg!(1);
            let count = arg!(2);
            let size = fx.i_layout(tyarg(0)).size.bytes();
            let len = if size == 1 { count[0] } else { fx.i_bin(Bin::Mul, count[0], Operand::Num(size as f64)) };
            fx.i_push(Stmt::Fill { dst: dst[0], val: val[0], len });
        }
        "volatile_load" | "unaligned_volatile_load" | "read_via_copy" => {
            let p = arg!(0);
            let lay = fx.i_layout(tyarg(0));
            let v = fx.i_load(p[0], lay);
            fx.i_write_val(dest, v);
        }
        "volatile_store" | "unaligned_volatile_store" | "write_via_move" => {
            let p = arg!(0);
            let (v, _) = fx.i_operand_val(&args[1].node);
            let lay = fx.i_layout(tyarg(0));
            fx.i_store(p[0], lay, v);
        }
        "transmute" | "transmute_unchecked" => {
            let (v, fl) = fx.i_operand_val(&args[0].node);
            let tl = dest.layout_pub();
            let addr = fx.i_spill(v, fl);
            let r = fx.i_load(addr, tl);
            fx.i_write_val(dest, r);
        }
        "forget" => {}
        "needs_drop" | "type_id" | "type_name" | "variant_count" | "size_of" | "align_of" => {
            let c = tcx
                .const_eval_instance(ty::TypingEnv::fully_monomorphized(), inst, si.span)
                .unwrap_or_else(|_| fx.i_fatal("const eval of intrinsic failed"));
            let v = fx.i_const_value(c, dest.layout_pub());
            fx.i_write_val(dest, v);
        }
        "bswap" if matches!(leaf_of(tyarg(0)), Some(LeafKind::I64 { .. } | LeafKind::I128 { .. })) => {
            // reverse the limbs and byte-swap each one
            let v = arg!(0);
            let r: Vec<Operand> = v.iter().rev().map(|x| fx.i_un(Un::BSwap, *x)).collect();
            fx.i_write(dest, r);
        }
        "ctpop" | "ctlz" | "ctlz_nonzero" | "cttz" | "cttz_nonzero" | "bswap" | "bitreverse" => {
            let v = arg!(0);
            let k = leaf_of(tyarg(0)).unwrap();
            let r = bit_count(fx, n, k, &v);
            // result type is u32
            fx.i_write(dest, vec![r]);
            let _ = r;
        }
        "rotate_left" | "rotate_right" => {
            let v = arg!(0);
            let s = arg!(1);
            let k = leaf_of(tyarg(0)).unwrap();
            let left = n == "rotate_left";
            let r = match k {
                LeafKind::Int(ik) if ik.bits == 32 => {
                    let r = fx.i_bin(if left { Bin::Rol } else { Bin::Ror }, v[0], s[0]);
                    vec![if ik.signed { fx.i_un(Un::Wrap(ik), r) } else { r }]
                }
                LeafKind::Int(ik) => {
                    // rotate within `bits`
                    let bits = ik.bits as f64;
                    let u = fx.i_un(Un::Wrap(IntKind { bits: ik.bits, signed: false }), v[0]);
                    let amt = fx.i_bin(Bin::Mod, s[0], Operand::Num(bits));
                    let amt = if left { amt } else {
                        let t = fx.i_bin(Bin::Sub, Operand::Num(bits), amt);
                        fx.i_bin(Bin::Mod, t, Operand::Num(bits))
                    };
                    let a = fx.i_bin(Bin::Shl, u, amt);
                    let inv = fx.i_bin(Bin::Sub, Operand::Num(bits), amt);
                    let b = fx.i_bin(Bin::Shr, u, inv);
                    let o = fx.i_bin(Bin::BOr, a, b);
                    vec![fx.i_un(Un::Wrap(ik), o)]
                }
                LeafKind::I64 { .. } => {
                    // rotl(x, n) = shl(x, n) | shr(x, 64 - n), with n taken mod 64
                    let n = match s[0] {
                        Operand::Num(c) => Operand::Num(((c as i64).rem_euclid(64)) as f64),
                        o => fx.i_bin(Bin::BAnd, o, Operand::Num(63.0)),
                    };
                    let n = if left { n } else {
                        match n {
                            Operand::Num(c) => Operand::Num(((64 - c as i64) % 64) as f64),
                            o => {
                                let t = fx.i_bin(Bin::Sub, Operand::Num(64.0), o);
                                fx.i_bin(Bin::BAnd, t, Operand::Num(63.0))
                            }
                        }
                    };
                    let inv = match n {
                        Operand::Num(c) => Operand::Num(((64 - c as i64) % 64) as f64),
                        o => {
                            let t = fx.i_bin(Bin::Sub, Operand::Num(64.0), o);
                            fx.i_bin(Bin::BAnd, t, Operand::Num(63.0))
                        }
                    };
                    let a = fx.i_shift64_var(&v, n, 0);
                    let b = fx.i_shift64_var(&v, inv, 1);
                    // n == 0: inv == 0 too, so a == b == x and the OR is still x
                    vec![fx.i_bin(Bin::BOr, a[0], b[0]), fx.i_bin(Bin::BOr, a[1], b[1])]
                }
                _ => fx.i_fatal("bad rotate"),
            };
            fx.i_write(dest, r);
        }
        "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "unchecked_add" | "unchecked_sub" | "unchecked_mul"
        | "unchecked_div" | "unchecked_rem" | "unchecked_shl" | "unchecked_shr" | "exact_div" | "add_with_overflow"
        | "sub_with_overflow" | "mul_with_overflow" | "three_way_compare" | "disjoint_bitor" => {
            use rustc_middle::mir::BinOp as B;
            let op = match n {
                "wrapping_add" => B::Add,
                "wrapping_sub" => B::Sub,
                "wrapping_mul" => B::Mul,
                "unchecked_add" => B::AddUnchecked,
                "unchecked_sub" => B::SubUnchecked,
                "unchecked_mul" => B::MulUnchecked,
                "unchecked_div" | "exact_div" => B::Div,
                "unchecked_rem" => B::Rem,
                "unchecked_shl" => B::ShlUnchecked,
                "unchecked_shr" => B::ShrUnchecked,
                "add_with_overflow" => B::AddWithOverflow,
                "sub_with_overflow" => B::SubWithOverflow,
                "mul_with_overflow" => B::MulWithOverflow,
                "three_way_compare" => B::Cmp,
                _ => B::BitOr,
            };
            let rv = mir::Rvalue::BinaryOp(op, Box::new((args[0].node.clone(), args[1].node.clone())));
            fx.i_rvalue(dest, &rv);
        }
        "saturating_add" | "saturating_sub" if matches!(leaf_of(tyarg(0)), Some(LeafKind::I64 { .. } | LeafKind::I128 { .. })) => {
            let a = arg!(0);
            let b = arg!(1);
            let (signed, wide) = match leaf_of(tyarg(0)).unwrap() {
                LeafKind::I64 { signed } => (signed, false),
                LeafKind::I128 { signed } => (signed, true),
                _ => unreachable!(),
            };
            let add = n == "saturating_add";
            let helper = match (add, wide) {
                (true, false) => "addo64",
                (false, false) => "subo64",
                (true, true) => "addo128",
                (false, true) => "subo128",
            };
            let nl = a.len();
            let mut args = a.clone();
            args.extend(b);
            args.push(Operand::Bool(signed));
            let r = fx.i_helper(helper, &args, nl + 1);
            let ov = fx.i_un(Un::NumToBool, r[nl]);
            // clamp: unsigned add -> MAX, unsigned sub -> 0, signed -> MIN/MAX by a's sign
            let top = |i: usize| i + 1 == nl;
            let neg = if signed {
                Some(fx.i_bin(Bin::Le, Operand::Num(2147483648.0), a[nl - 1]))
            } else {
                None
            };
            let mut out = vec![];
            for i in 0..nl {
                let sat = match (signed, add) {
                    (false, true) => Operand::Num(4294967295.0),
                    (false, false) => Operand::Num(0.0),
                    (true, _) => {
                        // a < 0 -> MIN (only the sign bit), else MAX
                        let min = if top(i) { 2147483648.0 } else { 0.0 };
                        let max = if top(i) { 2147483647.0 } else { 4294967295.0 };
                        fx.i_assign(l::Ty::Num, Rv::Select(neg.unwrap(), Operand::Num(min), Operand::Num(max)))
                    }
                };
                out.push(fx.i_assign(l::Ty::Num, Rv::Select(ov, sat, r[i])));
            }
            fx.i_write(dest, out);
        }
        "saturating_add" | "saturating_sub" => {
            let a = arg!(0);
            let b = arg!(1);
            let k = leaf_of(tyarg(0)).unwrap();
            let LeafKind::Int(ik) = k else { fx.i_fatal(format!("{n} on {} is unsupported", tyarg(0))) };
            let r = if n == "saturating_add" { fx.i_bin(Bin::Add, a[0], b[0]) } else { fx.i_bin(Bin::Sub, a[0], b[0]) };
            let r = fx.i_bin(Bin::Max, r, Operand::Num(ik.min()));
            let r = fx.i_bin(Bin::Min, r, Operand::Num(ik.max()));
            fx.i_write(dest, vec![r]);
        }
        "ptr_offset_from" | "ptr_offset_from_unsigned" => {
            let a = arg!(0);
            let b = arg!(1);
            let size = fx.i_layout(tyarg(0)).size.bytes();
            let d = fx.i_bin(Bin::Sub, a[0], b[0]);
            let r = if size == 1 { d } else { fx.i_bin(Bin::Div, d, Operand::Num(size as f64)) };
            fx.i_write(dest, vec![r]);
        }
        "arith_offset" => {
            let a = arg!(0);
            let b = arg!(1);
            let size = fx.i_layout(tyarg(0)).size.bytes();
            let s = if size == 1 { b[0] } else { fx.i_bin(Bin::Mul, b[0], Operand::Num(size as f64)) };
            let r = fx.i_bin(Bin::Add, a[0], s);
            let r = fx.i_un(Un::Wrap(IntKind::U32), r);
            fx.i_write(dest, vec![r]);
        }
        "ptr_mask" => {
            let a = arg!(0);
            let m = arg!(1);
            let r = fx.i_bin(Bin::BAnd, a[0], m[0]);
            fx.i_write(dest, vec![r]);
        }
        "ptr_guaranteed_cmp" => {
            let a = arg!(0);
            let b = arg!(1);
            let e = fx.i_bin(Bin::Eq, a[0], b[0]);
            let r = fx.i_un(Un::BoolToNum, e);
            fx.i_write(dest, vec![r]);
        }
        "select_unpredictable" => {
            let c = arg!(0);
            let (a, al) = fx.i_operand_val(&args[1].node);
            let (b, _) = fx.i_operand_val(&args[2].node);
            match (a, b) {
                (Value::Regs(a), Value::Regs(b)) => {
                    let mut r: Vec<Operand> = vec![];
                    for (x, y) in a.into_iter().zip(b) {
                        let t = if matches!(x, Operand::Bool(_)) { l::Ty::Bool } else { l::Ty::Num };
                        r.push(fx.i_assign(t, Rv::Select(c[0], x, y)));
                    }
                    fx.i_write(dest, r);
                }
                (a, b) => {
                    let pa = fx.i_spill(a, al);
                    let pb = fx.i_spill(b, al);
                    let p = fx.i_assign(l::Ty::Num, Rv::Select(c[0], pa, pb));
                    let v = fx.i_load(p, al);
                    fx.i_write_val(dest, v);
                }
            }
        }
        "discriminant_value" => {
            let p = arg!(0);
            let t = tyarg(0);
            let place = fx.i_mem_place(p[0], fx.i_layout(t));
            let d = fx.i_discriminant(&place, dest.layout_pub());
            fx.i_write(dest, d);
        }
        "raw_eq" => {
            let a = arg!(0);
            let b = arg!(1);
            let size = fx.i_layout(tyarg(0)).size.bytes();
            let r = fx.i_helper("memcmp", &[a[0], b[0], Operand::Num(size as f64)], 1);
            let e = fx.i_bin(Bin::Eq, r[0], Operand::Num(0.0));
            fx.i_write(dest, vec![e]);
        }
        "compare_bytes" => {
            let a = arg!(0);
            let b = arg!(1);
            let len = arg!(2);
            let r = fx.i_helper("memcmp", &[a[0], b[0], len[0]], 1);
            fx.i_write(dest, r);
        }
        "catch_unwind" => {
            // no unwinding: just call the function
            let f = arg!(0);
            let data = arg!(1);
            fx.i_call_indirect(f[0], vec![data[0]], vec![]);
            fx.i_write(dest, vec![Operand::Num(0.0)]);
        }
        "atomic_fence" | "atomic_singlethreadfence" => {}
        _ if n.starts_with("atomic_") => return atomic(fx, n, inst, args, dest),
        _ => {
            // generic float intrinsics (`fabs::<f64>`): append the type suffix
            let generic = inst.args.types().next().and_then(|t| match t.kind() {
                ty::Float(ty::FloatTy::F32) => Some(format!("{n}f32")),
                ty::Float(ty::FloatTy::F64) => Some(format!("{n}f64")),
                _ => None,
            });
            if let Some(g) = generic.as_deref() {
                if !n.ends_with("f32") && !n.ends_with("f64") {
                    if let Some(r) = float_intrinsic(fx, g, args, dest) {
                        return r;
                    }
                }
            }
            if let Some(r) = float_intrinsic(fx, n, args, dest) {
                return r;
            }
            if tcx.intrinsic(inst.def_id()).map(|i| i.must_be_overridden).unwrap_or(true) {
                fx.i_fatal(format!("unsupported intrinsic `{n}`"));
            }
            return false;
        }
    }
    let _ = sym::abort;
    true
}

fn bit_count<'tcx>(fx: &mut FnLower<'_, 'tcx>, n: &str, k: LeafKind, v: &[Operand]) -> Operand {
    match k {
        LeafKind::Int(ik) => {
            let bits = ik.bits as f64;
            let u = if ik.signed { fx.i_un(Un::Wrap(IntKind { bits: ik.bits, signed: false }), v[0]) } else { v[0] };
            match n {
                "ctpop" => fx.popcnt_inline(u),
                "ctlz" | "ctlz_nonzero" => {
                    let c = fx.i_un(Un::Clz, u);
                    if ik.bits == 32 { c } else { fx.i_bin(Bin::Sub, c, Operand::Num(32.0 - bits)) }
                }
                "cttz" | "cttz_nonzero" => {
                    let c = fx.i_un(Un::Ctz, u);
                    if ik.bits == 32 { c } else { fx.i_bin(Bin::Min, c, Operand::Num(bits)) }
                }
                "bswap" => {
                    let r = match ik.bits {
                        8 => u,
                        16 => {
                            let s = fx.i_un(Un::BSwap, u);
                            fx.i_bin(Bin::Shr, s, Operand::Num(16.0))
                        }
                        _ => fx.i_un(Un::BSwap, u),
                    };
                    if ik.signed { fx.i_un(Un::Wrap(ik), r) } else { r }
                }
                _ => {
                    let r = fx.i_helper("bitrev32", &[u], 1)[0];
                    let r = if ik.bits < 32 { fx.i_bin(Bin::Shr, r, Operand::Num(32.0 - bits)) } else { r };
                    if ik.signed { fx.i_un(Un::Wrap(ik), r) } else { r }
                }
            }
        }
        LeafKind::I64 { .. } => match n {
            "ctpop" => {
                let a = fx.popcnt_inline(v[0]);
                let b = fx.popcnt_inline(v[1]);
                fx.i_bin(Bin::Add, a, b)
            }
            "ctlz" | "ctlz_nonzero" => {
                let hz = fx.i_bin(Bin::Eq, v[1], Operand::Num(0.0));
                let ch = fx.i_un(Un::Clz, v[1]);
                let cl = fx.i_un(Un::Clz, v[0]);
                let cl = fx.i_bin(Bin::Add, cl, Operand::Num(32.0));
                fx.i_assign(l::Ty::Num, Rv::Select(hz, cl, ch))
            }
            "cttz" | "cttz_nonzero" => {
                let lz = fx.i_bin(Bin::Eq, v[0], Operand::Num(0.0));
                let cl = fx.i_un(Un::Ctz, v[0]);
                let ch = fx.i_un(Un::Ctz, v[1]);
                let ch = fx.i_bin(Bin::Add, ch, Operand::Num(32.0));
                fx.i_assign(l::Ty::Num, Rv::Select(lz, ch, cl))
            }
            "bswap" => fx.i_fatal("bswap64 must be handled as pair"),
            _ => fx.i_fatal("bitreverse64 unsupported"),
        },
        LeafKind::I128 { .. } => match n {
            "ctpop" => {
                let mut acc = Operand::Num(0.0);
                for limb in v.iter() {
                    let c = fx.popcnt_inline(*limb);
                    acc = fx.i_bin(Bin::Add, acc, c);
                }
                acc
            }
            "ctlz" | "ctlz_nonzero" | "cttz" | "cttz_nonzero" => {
                // scan limbs from the significant end: count = 32*k + clz(limb_k) for the first
                // non-zero limb
                let lead = n.starts_with("ctlz");
                let order: Vec<usize> = if lead { vec![3, 2, 1, 0] } else { vec![0, 1, 2, 3] };
                let mut acc = Operand::Num(128.0);
                for (k, &i) in order.iter().enumerate().rev() {
                    let nz = fx.i_bin(Bin::Ne, v[i], Operand::Num(0.0));
                    let c = fx.i_un(if lead { Un::Clz } else { Un::Ctz }, v[i]);
                    let c = fx.i_bin(Bin::Add, c, Operand::Num(32.0 * k as f64));
                    acc = fx.i_assign(l::Ty::Num, Rv::Select(nz, c, acc));
                }
                acc
            }
            _ => fx.i_fatal(format!("{n} on 128-bit integers is unsupported")),
        },
        _ => fx.i_fatal("bad bit intrinsic"),
    }
}

fn float_intrinsic<'tcx>(fx: &mut FnLower<'_, 'tcx>, n: &str, args: Args<'_, 'tcx>, dest: &PlaceVal<'tcx>) -> Option<bool> {
    let (base, f32) = if let Some(b) = n.strip_suffix("f32") {
        (b, true)
    } else if let Some(b) = n.strip_suffix("f64") {
        (b, false)
    } else {
        return None;
    };
    let a: Vec<Operand> = args.iter().map(|a| fx.i_operand(&a.node).0[0]).collect();
    let r = match base {
        "sqrt" => fx.i_un(Un::Sqrt, a[0]),
        "floor" => fx.i_un(Un::Floor, a[0]),
        "ceil" => fx.i_un(Un::Ceil, a[0]),
        "trunc" => fx.i_un(Un::Trunc, a[0]),
        "fabs" => fx.i_un(Un::Abs, a[0]),
        "round" => fx.i_helper("roundaway", &a, 1)[0],
        "round_ties_even" | "rint" | "nearbyint" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.round", vec![a[0]])),
        "minnum" | "minimum" => fx.i_helper("fmin", &a, 1)[0],
        "maxnum" | "maximum" => fx.i_helper("fmax", &a, 1)[0],
        "copysign" => fx.i_helper("copysign", &a, 1)[0],
        "pow" => fx.i_bin(Bin::Pow, a[0], a[1]),
        "powi" => fx.i_bin(Bin::Pow, a[0], a[1]),
        "exp" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.exp", vec![a[0]])),
        "exp2" => fx.i_bin(Bin::Pow, Operand::Num(2.0), a[0]),
        "log" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.log", vec![a[0]])),
        "log2" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.log", vec![a[0], Operand::Num(2.0)])),
        "log10" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.log10", vec![a[0]])),
        "sin" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.sin", vec![a[0]])),
        "cos" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.cos", vec![a[0]])),
        "tan" => fx.i_assign(l::Ty::Num, Rv::Builtin("math.tan", vec![a[0]])),
        "fma" | "fmuladd" => {
            let m = fx.i_bin(Bin::Mul, a[0], a[1]);
            fx.i_bin(Bin::Add, m, a[2])
        }
        "fadd_fast" | "fadd_algebraic" => fx.i_bin(Bin::Add, a[0], a[1]),
        "fsub_fast" | "fsub_algebraic" => fx.i_bin(Bin::Sub, a[0], a[1]),
        "fmul_fast" | "fmul_algebraic" => fx.i_bin(Bin::Mul, a[0], a[1]),
        "fdiv_fast" | "fdiv_algebraic" => fx.i_bin(Bin::Div, a[0], a[1]),
        _ => return None,
    };
    let r = if f32 && !matches!(base, "fabs" | "copysign" | "minnum" | "maxnum" | "minimum" | "maximum" | "floor" | "ceil" | "trunc") {
        fx.i_un(Un::F32Round, r)
    } else {
        r
    };
    fx.i_write(dest, vec![r]);
    Some(true)
}

fn atomic<'tcx>(fx: &mut FnLower<'_, 'tcx>, n: &str, inst: Instance<'tcx>, args: Args<'_, 'tcx>, dest: &PlaceVal<'tcx>) -> bool {
    let t = inst.args.type_at(0);
    let lay = fx.i_layout(t);
    let p = fx.i_operand(&args[0].node).0[0];
    match n {
        "atomic_load" => {
            let v = fx.i_load(p, lay);
            fx.i_write_val(dest, v);
        }
        "atomic_store" => {
            let (v, _) = fx.i_operand_val(&args[1].node);
            fx.i_store(p, lay, v);
        }
        "atomic_xchg" => {
            let old = fx.i_load(p, lay);
            let (v, _) = fx.i_operand_val(&args[1].node);
            fx.i_store(p, lay, v);
            fx.i_write_val(dest, old);
        }
        "atomic_cxchg" | "atomic_cxchgweak" => {
            let Value::Regs(old) = fx.i_load(p, lay) else { unreachable!() };
            let (exp, _) = fx.i_operand(&args[1].node);
            let (new, _) = fx.i_operand(&args[2].node);
            let mut eq = Operand::Bool(true);
            for (o, e) in old.iter().zip(exp.iter()) {
                let c = fx.i_bin(Bin::Eq, *o, *e);
                eq = fx.i_bin(Bin::And, eq, c);
            }
            let sel: Vec<Operand> = old.iter().zip(new.iter()).map(|(o, nw)| fx.i_assign(l::Ty::Num, Rv::Select(eq, *nw, *o))).collect();
            fx.i_store(p, lay, Value::Regs(sel));
            let mut r = old.clone();
            r.push(eq);
            fx.i_write(dest, r);
        }
        "atomic_xadd" | "atomic_xsub" | "atomic_and" | "atomic_or" | "atomic_xor" | "atomic_nand" | "atomic_max"
        | "atomic_min" | "atomic_umax" | "atomic_umin" => {
            use rustc_middle::mir::BinOp as B;
            let old = fx.i_load(p, lay);
            let Value::Regs(oldv) = old.clone() else { unreachable!() };
            let (v, vl) = fx.i_operand(&args[1].node);
            let res = match n {
                "atomic_max" | "atomic_umax" | "atomic_min" | "atomic_umin" => {
                    let gt = fx.i_binop_pub(B::Gt, &oldv, lay, &v, vl, lay)[0];
                    let take_old = if n.ends_with("max") { gt } else { fx.i_un(Un::Not, gt) };
                    oldv.iter().zip(v.iter()).map(|(o, x)| fx.i_assign(l::Ty::Num, Rv::Select(take_old, *o, *x))).collect()
                }
                "atomic_nand" => {
                    let a = fx.i_binop_pub(B::BitAnd, &oldv, lay, &v, vl, lay);
                    a.iter()
                        .map(|x| {
                            let nb = fx.i_un(Un::BNot, *x);
                            match leaf_of(t) {
                                Some(LeafKind::Int(k)) => fx.i_un(Un::Wrap(k), nb),
                                _ => nb,
                            }
                        })
                        .collect()
                }
                _ => {
                    let op = match n {
                        "atomic_xadd" => B::Add,
                        "atomic_xsub" => B::Sub,
                        "atomic_and" => B::BitAnd,
                        "atomic_or" => B::BitOr,
                        _ => B::BitXor,
                    };
                    fx.i_binop_pub(op, &oldv, lay, &v, vl, lay)
                }
            };
            fx.i_store(p, lay, Value::Regs(res));
            fx.i_write_val(dest, old);
        }
        _ => fx.i_fatal(format!("unsupported atomic intrinsic `{n}`")),
    }
    true
}

/// `extern "C" { fn __luau_NAME(...) }` imports map to prelude `rt_NAME` functions.
pub fn lower_import<'tcx>(fx: &mut FnLower<'_, 'tcx>, name: &str, args: Args<'_, 'tcx>, dest: Place<'tcx>) -> bool {
    let Some(rt) = name.strip_prefix("__luau_") else { return false };
    let mut flat = vec![];
    for a in args {
        let (v, _) = fx.i_operand(&a.node);
        flat.extend(v);
    }
    let dpv = fx.i_place(dest);
    let dl = dpv.layout_pub();
    let rets = abi::leaves(fx.i_lcx(), dl).map(|l| abi::ltys(&l)).unwrap_or_default();
    let r = fx.i_helper(&format!("rt_{rt}"), &flat, rets.len());
    if !r.is_empty() {
        fx.i_write(&dpv, r);
    }
    true
}

#[allow(dead_code)]
fn unused(_: Bin, _: IntKind) {}

/// (size, align) of a dynamically sized value of type `t` with pointer metadata `meta`.
fn dst_size_align<'tcx>(fx: &mut FnLower<'_, 'tcx>, t: ty::Ty<'tcx>, meta: Operand) -> (Operand, Operand) {
    let lay = fx.i_layout(t);
    match t.kind() {
        ty::Slice(e) | ty::Array(e, _) => {
            let el = fx.i_layout(*e);
            (fx.i_bin(Bin::Mul, meta, Operand::Num(el.size.bytes() as f64)), Operand::Num(el.align.bytes() as f64))
        }
        ty::Str => (meta, Operand::Num(1.0)),
        ty::Dynamic(..) => (
            fx.i_assign(l::Ty::Num, Rv::Load { mem: Mem::U32, addr: meta, off: 4 }),
            fx.i_assign(l::Ty::Num, Rv::Load { mem: Mem::U32, addr: meta, off: 8 }),
        ),
        ty::Adt(..) | ty::Tuple(..) => {
            // struct with an unsized last field
            let last = lay.fields.count() - 1;
            let tail_off = lay.fields.offset(last).bytes() as f64;
            let tail_ty = lay.field(&fx.i_lcx(), last).ty;
            let (tsize, talign) = dst_size_align(fx, tail_ty, meta);
            let salign = Operand::Num(lay.align.bytes() as f64);
            let align = fx.i_bin(Bin::Max, salign, talign);
            // offset of the tail is aligned to the tail's alignment (which may be dynamic)
            let a1 = fx.i_bin(Bin::Sub, talign, Operand::Num(1.0));
            let o = fx.i_bin(Bin::Add, Operand::Num(tail_off), a1);
            let o = fx.i_bin(Bin::FloorDiv, o, talign);
            let o = fx.i_bin(Bin::Mul, o, talign);
            let unpadded = fx.i_bin(Bin::Add, o, tsize);
            let a2 = fx.i_bin(Bin::Sub, align, Operand::Num(1.0));
            let s = fx.i_bin(Bin::Add, unpadded, a2);
            let s = fx.i_bin(Bin::FloorDiv, s, align);
            (fx.i_bin(Bin::Mul, s, align), align)
        }
        _ => fx.i_fatal(format!("size_of_val of {t} not supported")),
    }
}
