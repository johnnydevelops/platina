//! The std platform layer for Roblox/Luau.
//!
//! The target looks like `wasm32-unknown-unknown` to `std`, whose platform layer is mostly
//! stubs. Calls to a few of those functions are replaced here with Luau runtime functions:
//! allocation, stdout/stderr, clocks and abort.

use rustc_abi::FieldIdx;
use rustc_middle::mir::Operand as MOp;
use rustc_middle::ty::layout::TyAndLayout;
use rustc_middle::ty::{self, Instance};
use rustc_span::{Spanned, sym};

use luau_lir::ir::{Bin, Operand};

use crate::abi::{self, LeafKind};
use crate::lower::{FnLower, PlaceVal};

type Args<'a, 'tcx> = &'a [Spanned<MOp<'tcx>>];

/// Returns true if the call was replaced.
pub fn lower_override<'tcx>(
    fx: &mut FnLower<'_, 'tcx>,
    inst: Instance<'tcx>,
    args: Args<'_, 'tcx>,
    dest: &PlaceVal<'tcx>,
) -> bool {
    let tcx = fx.i_tcx();
    let def_id = inst.def_id();
    if tcx.crate_name(def_id.krate) != sym::std {
        return false;
    }
    let Some(name) = tcx.opt_item_name(def_id) else { return false };
    let name = name.as_str();
    let path = rustc_middle::ty::print::with_no_trimmed_paths!(tcx.def_path_str(def_id));

    // System allocator (std::sys::alloc::wasm)
    if path.contains("sys::alloc::wasm") && path.contains("GlobalAlloc") {
        // args: &System, [ptr], Layout, [new_size]
        let ops: Vec<(Vec<Operand>, TyAndLayout<'tcx>)> = args.iter().map(|a| fx.i_operand(&a.node)).collect();
        let layout_arg = |i: usize, fx: &mut FnLower<'_, 'tcx>| layout_size_align(fx, &ops[i].0, ops[i].1);
        let r = match name {
            "alloc" | "alloc_zeroed" => {
                let (size, align) = layout_arg(1, fx);
                let h = if name == "alloc" { "rt_alloc" } else { "rt_alloc_zeroed" };
                fx.i_helper(h, &[size, align], 1)
            }
            "dealloc" => {
                let (size, align) = layout_arg(2, fx);
                fx.i_helper("rt_dealloc", &[ops[1].0[0], size, align], 0)
            }
            "realloc" => {
                let (size, align) = layout_arg(2, fx);
                fx.i_helper("rt_realloc", &[ops[1].0[0], size, align, ops[3].0[0]], 1)
            }
            _ => return false,
        };
        if !r.is_empty() {
            fx.i_write(dest, r);
        }
        return true;
    }

    // stdout / stderr: <StdoutRaw|StderrRaw as io::Write>::{write, write_all}
    let fd = if path.contains("io::stdio::StdoutRaw") {
        1.0
    } else if path.contains("io::stdio::StderrRaw") || path.contains("sys::stdio::unsupported") {
        // raw platform writes come from panic output (`sys::stdio::panic_output`)
        2.0
    } else {
        0.0
    };
    if fd != 0.0 && (name == "write" || name == "write_all") {
        // args: &mut self, &[u8]
        let (buf, _) = fx.i_operand(&args[1].node);
        fx.i_helper("rt_write", &[Operand::Num(fd), buf[0], buf[1]], 0);
        // io::Result<usize> = Ok(len) / io::Result<()> = Ok(())
        let ok = if name == "write" { vec![buf[1]] } else { vec![] };
        write_ok(fx, dest, ok);
        return true;
    }
    if fd != 0.0 && name == "flush" {
        write_ok(fx, dest, vec![]);
        return true;
    }

    // clocks: sys::time::unsupported::{Instant, SystemTime}::now
    if path.contains("sys::time") && name == "now" {
        let h = if path.contains("SystemTime") { "rt_systime" } else { "rt_instant" };
        let parts = fx.i_helper(h, &[], 3);
        write_duration_newtype(fx, dest, &parts);
        return true;
    }

    // panic hook: record "panicked at file:line:col:\nmsg" for the abort that follows
    if path == "std::panicking::default_hook" {
        let (info, info_ty) = fx.i_operand(&args[0].node);
        let pointee = info_ty.ty.builtin_deref(true).unwrap();
        let ty::Adt(adt, _) = pointee.kind() else { return false };
        let find = |n: &str| -> Option<Instance<'tcx>> {
            for imp in tcx.inherent_impls(adt.did()) {
                for item in tcx.associated_items(*imp).in_definition_order() {
                    if item.name().as_str() == n {
                        return Some(Instance::mono(tcx, item.def_id));
                    }
                }
            }
            None
        };
        let (Some(payload_as_str), Some(location)) = (find("payload_as_str"), find("location")) else { return false };
        let msg = fx.i_call_fn(payload_as_str, vec![info[0]], vec![luau_lir::ir::Ty::Num, luau_lir::ir::Ty::Num]);
        let loc = fx.i_call_fn(location, vec![info[0]], vec![luau_lir::ir::Ty::Num]);
        // Location { file: &str, line: u32, col: u32 } (field order chosen by rustc)
        let loc_ty = tcx.caller_location_ty().builtin_deref(true).unwrap();
        let ll = fx.i_layout(loc_ty);
        let ty::Adt(ldef, _) = loc_ty.kind() else { return false };
        let off = |n: &str| -> i32 {
            let fields = &ldef.non_enum_variant().fields;
            let i = fields.iter().position(|f| f.name.as_str() == n).unwrap();
            ll.fields.offset(i).bytes() as i32
        };
        let (fo, lo, co) = (off("filename"), off("line"), off("col"));
        fx.i_helper("rt_panic_msg", &[msg[0], msg[1], loc[0], Operand::Num(fo as f64), Operand::Num(lo as f64), Operand::Num(co as f64)], 0);
        return true;
    }

    // abort: sys::pal::*::abort_internal / process::abort
    if name == "abort_internal" || (path == "std::process::abort") {
        fx.i_helper("rt_abort", &[], 0);
        fx.i_set_unreachable();
        return true;
    }
    false
}

/// (size, align) of a `core::alloc::Layout` value given as leaves.
fn layout_size_align<'tcx>(fx: &mut FnLower<'_, 'tcx>, vals: &[Operand], lay: TyAndLayout<'tcx>) -> (Operand, Operand) {
    let tcx = fx.i_tcx();
    let ty::Adt(def, _) = lay.ty.kind() else { fx.i_fatal("Layout is not an ADT") };
    let fields = &def.non_enum_variant().fields;
    let off_of = |n: &str| -> u64 {
        let i = fields.iter().position(|f| f.name.as_str() == n).expect("Layout field");
        lay.fields.offset(FieldIdx::from_usize(i).as_usize()).bytes()
    };
    let leaves = abi::leaves(fx.i_lcx(), lay).expect("Layout leaves");
    let idx = |off: u64| leaves.iter().position(|l| l.off == off).expect("Layout leaf");
    let _ = tcx;
    (vals[idx(off_of("size"))], vals[idx(off_of("align"))])
}

/// Write `Ok(vals)` into an `io::Result<T>` destination.
fn write_ok<'tcx>(fx: &mut FnLower<'_, 'tcx>, dest: &PlaceVal<'tcx>, vals: Vec<Operand>) {
    let lay = dest.layout_pub();
    let ty::Adt(def, _) = lay.ty.kind() else { fx.i_fatal("io::Result is not an ADT") };
    let ok = def.variants().iter_enumerated().find(|(_, v)| v.name.as_str() == "Ok").map(|(i, _)| i).unwrap();
    fx.i_write_variant(dest, ok, vals);
}

/// Write an `Instant`/`SystemTime` (newtype of `Duration { secs: u64, nanos: u32 }`).
fn write_duration_newtype<'tcx>(fx: &mut FnLower<'_, 'tcx>, dest: &PlaceVal<'tcx>, parts: &[Operand]) {
    let lay = dest.layout_pub();
    let leaves = abi::leaves(fx.i_lcx(), lay).expect("time leaves");
    // leaves sorted by offset; Duration is { secs: u64, nanos: Nanoseconds(u32) } in some order
    let mut out = vec![];
    for l in &leaves {
        match l.kind {
            LeafKind::I64 { .. } => out.extend([parts[0], parts[1]]),
            LeafKind::Int(_) => out.push(parts[2]),
            _ => fx.i_fatal("unexpected time layout"),
        }
    }
    let _ = Bin::Add;
    fx.i_write(dest, out);
}

/// Functions whose bodies are replaced entirely.
pub fn is_body_override<'tcx>(tcx: ty::TyCtxt<'tcx>, inst: Instance<'tcx>) -> bool {
    let def_id = inst.def_id();
    matches!(inst.def, ty::InstanceKind::Item(_))
        && tcx.crate_name(def_id.krate) == sym::std
        && {
            let p = rustc_middle::ty::print::with_no_trimmed_paths!(tcx.def_path_str(def_id));
            matches!(p.as_str(), "std::panicking::begin_panic_handler" | "std::panicking::panic_handler")
        }
}

/// `std::panicking::begin_panic_handler(info: &PanicInfo) -> !`: format the message with
/// `ToString`, hand it (with the location) to the runtime and abort with it as the error.
pub fn override_body<'tcx>(fx: &mut FnLower<'_, 'tcx>) {
    let tcx = fx.i_tcx();
    let info = fx.i_param(0);
    // PanicInfo methods
    let info_def = tcx.lang_items().panic_info().expect("PanicInfo lang item");
    let method = |n: &str| -> Instance<'tcx> {
        for imp in tcx.inherent_impls(info_def) {
            for item in tcx.associated_items(*imp).in_definition_order() {
                if item.name().as_str() == n {
                    return Instance::mono(tcx, item.def_id);
                }
            }
        }
        panic!("PanicInfo::{n} not found")
    };
    let message = method("message");
    let out_ty = |def_id| {
        let sig = tcx.fn_sig(def_id).instantiate_identity().skip_normalization();
        tcx.instantiate_bound_regions_with_erased(sig).output()
    };
    let msg_ty = out_ty(message.def_id());
    let msg_layout = fx.i_layout(msg_ty);
    let msg_tys = fx.i_leaf_tys(msg_layout);
    let msg = fx.i_call_fn(message, vec![info[0]], msg_tys);
    // &PanicMessage for ToString::to_string
    let slot = fx.i_temp_slot(msg_layout);
    fx.i_store(slot, msg_layout, crate::lower::Value::Regs(msg));
    let to_string_def = tcx.get_diagnostic_item(rustc_span::Symbol::intern("to_string_method")).expect("ToString::to_string");
    let to_string = Instance::expect_resolve(
        tcx,
        ty::TypingEnv::fully_monomorphized(),
        to_string_def,
        tcx.mk_args(&[msg_ty.into()]),
        rustc_span::DUMMY_SP,
    );
    let string_ty = out_ty(to_string_def);
    let string_layout = fx.i_layout(string_ty);
    let s = fx.i_call_fn(to_string, vec![slot], fx.i_leaf_tys(string_layout));
    // String { vec: Vec<u8> { buf: RawVec { inner: RawVecInner { ptr, .. } }, len } }
    let leaves = abi::leaves(fx.i_lcx(), string_layout).expect("String leaves");
    let ptr_off = field_path_offset(fx, string_layout, &["vec", "buf", "inner", "ptr"]);
    let len_off = field_path_offset(fx, string_layout, &["vec", "len"]);
    let at = |off: u64| leaves.iter().position(|l| l.off == off).expect("String leaf");
    let (sp, sl) = (s[at(ptr_off)], s[at(len_off)]);
    // location
    let location = method("location");
    let loc = fx.i_call_fn(location, vec![info[0]], vec![luau_lir::ir::Ty::Num]);
    let loc_ty = tcx.caller_location_ty().builtin_deref(true).unwrap();
    let ll = fx.i_layout(loc_ty);
    let fo = field_path_offset(fx, ll, &["filename"]) as f64;
    let lo = field_path_offset(fx, ll, &["line"]) as f64;
    let co = field_path_offset(fx, ll, &["col"]) as f64;
    fx.i_helper("rt_panic_msg", &[sp, sl, loc[0], Operand::Num(fo), Operand::Num(lo), Operand::Num(co)], 0);
    fx.i_helper("rt_abort", &[], 0);
    fx.i_set_unreachable();
}

/// Byte offset of a nested field path (by field names) inside a struct layout.
fn field_path_offset<'tcx>(fx: &FnLower<'_, 'tcx>, mut lay: TyAndLayout<'tcx>, path: &[&str]) -> u64 {
    let mut off = 0;
    for name in path {
        let ty::Adt(def, _) = lay.ty.kind() else { fx.i_fatal(format!("field path {path:?}: not an ADT")) };
        let fields = &def.non_enum_variant().fields;
        let i = fields
            .iter()
            .position(|f| f.name.as_str() == *name)
            .unwrap_or_else(|| fx.i_fatal(format!("no field {name} in {}", lay.ty)));
        off += lay.fields.offset(i).bytes();
        lay = lay.field(&fx.i_lcx(), i);
    }
    off
}
