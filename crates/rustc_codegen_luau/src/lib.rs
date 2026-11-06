#![feature(rustc_private)]
#![allow(clippy::too_many_arguments)]

extern crate rustc_abi;
extern crate rustc_codegen_ssa;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_metadata;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_symbol_mangling;
extern crate rustc_target;

mod abi;
mod cx;
mod intrinsics;
mod lower;

use std::any::Any;

use rustc_codegen_ssa::back::archive::ArArchiveBuilderBuilder;
use rustc_codegen_ssa::back::link::link_binary;
use rustc_codegen_ssa::traits::CodegenBackend;
use rustc_codegen_ssa::{CompiledModules, CrateInfo, TargetConfig};
use rustc_hir::def::DefKind;
use rustc_metadata::EncodedMetadata;
use rustc_middle::dep_graph::WorkProductMap;
use rustc_middle::ty::{Instance, TyCtxt};
use rustc_session::Session;
use rustc_session::config::{CrateType, OutputFilenames};

struct LuauBackend;

/// Memory layout: [0, NULL_GUARD) unused, then the shadow stack (grows down from
/// `stack_top`), then static data, then the heap.
const STACK_SIZE: u32 = 1 << 20;

impl CodegenBackend for LuauBackend {
    fn name(&self) -> &'static str {
        "luau"
    }

    fn target_cpu(&self, _sess: &Session) -> String {
        "generic".into()
    }

    fn target_config(&self, _sess: &Session) -> TargetConfig {
        TargetConfig {
            target_features: vec![],
            unstable_target_features: vec![],
            has_reliable_f16: false,
            has_reliable_f16_math: false,
            has_reliable_f128: false,
            has_reliable_f128_math: false,
        }
    }

    fn codegen_crate<'tcx>(&self, tcx: TyCtxt<'tcx>) -> Box<dyn Any> {
        let types = tcx.crate_types();
        if types.iter().any(|t| matches!(t, CrateType::Cdylib | CrateType::Executable)) {
            codegen_program(tcx);
        }
        Box::new(())
    }

    fn join_codegen(
        &self,
        _ongoing: Box<dyn Any>,
        _sess: &Session,
        _outputs: &OutputFilenames,
        _crate_info: &CrateInfo,
    ) -> (CompiledModules, WorkProductMap) {
        (CompiledModules { modules: vec![], allocator_module: None }, Default::default())
    }

    fn link(
        &self,
        sess: &Session,
        compiled_modules: CompiledModules,
        crate_info: CrateInfo,
        metadata: EncodedMetadata,
        outputs: &OutputFilenames,
    ) {
        if crate_info.crate_types.iter().any(|t| matches!(t, CrateType::Rlib)) {
            link_binary(sess, &ArArchiveBuilderBuilder, compiled_modules, crate_info, metadata, outputs, self.name());
        }
    }
}

fn codegen_program<'tcx>(tcx: TyCtxt<'tcx>) {
    let stack_top = cx::NULL_GUARD + STACK_SIZE;
    let mut cx = cx::Cx::new(tcx, stack_top);

    // roots: every #[no_mangle] / #[export_name] function of the local crate
    let mut exports = vec![];
    for def_id in tcx.hir_crate_items(()).definitions() {
        if !matches!(tcx.def_kind(def_id), DefKind::Fn | DefKind::AssocFn) {
            continue;
        }
        let attrs = tcx.codegen_fn_attrs(def_id);
        if !attrs.contains_extern_indicator()
            || attrs.flags.contains(rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags::RUSTC_STD_INTERNAL_SYMBOL)
        {
            continue;
        }
        if tcx.generics_of(def_id).requires_monomorphization(tcx) {
            continue;
        }
        let inst = Instance::mono(tcx, def_id.to_def_id());
        let name = tcx.symbol_name(inst).name.to_string();
        let id = cx.fn_id(inst);
        if is_lua_ident(&name) {
            exports.push((name, id));
        }
    }

    loop {
        while let Some(inst) = cx.queue.pop_front() {
            let id = cx.fn_ids[&inst];
            let name = cx.fn_names[id as usize].clone();
            let f = lower::FnLower::lower(&mut cx, inst, name);
            cx.funcs[id as usize] = Some(f);
        }
        cx.flush_allocs();
        if cx.queue.is_empty() {
            break;
        }
    }

    let mut program = cx.into_program(exports, stack_top);
    luau_lir::passes::optimize(&mut program);
    let src = match luau_lir::emit_program(&program) {
        Ok(s) => s,
        Err(e) => tcx.dcx().fatal(e),
    };
    let outputs = tcx.output_filenames(());
    for ct in tcx.crate_types() {
        if matches!(ct, CrateType::Cdylib | CrateType::Executable) {
            let path = rustc_session::output::out_filename(tcx.sess, *ct, outputs, tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE));
            let path = path.as_path();
            if let Err(e) = std::fs::write(path, &src) {
                tcx.dcx().fatal(format!("failed to write {}: {e}", path.display()));
            }
        }
    }
}

fn is_lua_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.chars().next().unwrap().is_ascii_digit()
}

#[unsafe(no_mangle)]
pub fn __rustc_codegen_backend() -> Box<dyn CodegenBackend> {
    Box::new(LuauBackend)
}
