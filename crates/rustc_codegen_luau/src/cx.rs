//! Whole-program codegen context: function ids, the static data image, the function table
//! and cross-crate symbol resolution.

use std::collections::{HashMap, HashSet, VecDeque};

use rustc_hir::def_id::{CrateNum, DefId, LOCAL_CRATE};
use rustc_middle::middle::exported_symbols::ExportedSymbol;
use rustc_middle::mir::interpret::{AllocId, GlobalAlloc};
use rustc_middle::ty::{self, Instance, TyCtxt};

use luau_lir::ir::{Function, Program};

pub const NULL_GUARD: u32 = 16;

pub struct Cx<'tcx> {
    pub tcx: TyCtxt<'tcx>,
    pub fn_ids: HashMap<Instance<'tcx>, u32>,
    pub queue: VecDeque<Instance<'tcx>>,
    pub funcs: Vec<Option<Function>>,
    pub fn_names: Vec<String>,
    /// fn table: slot -> function id. Slot 0 is the null function pointer.
    pub table: Vec<u32>,
    pub table_slot: HashMap<u32, u32>,
    /// static data image, relative to `data_base`
    pub data: Vec<u8>,
    pub data_base: u32,
    pub alloc_addr: HashMap<AllocId, u32>,
    pending_allocs: Vec<(AllocId, u32)>,
    symbols: Option<HashMap<String, Instance<'tcx>>>,
    /// vtable memory allocations -> principal trait (None = no principal)
    pub vtables: HashMap<AllocId, Option<DefId>>,
    /// (principal trait, slot) pairs invoked by virtual calls; `None` principal = unknown
    pub used_slots: HashSet<(Option<DefId>, usize)>,
    pub any_slot: HashSet<usize>,
    /// vtable method entries left as 0 until some virtual call uses their slot:
    /// (address in data, target alloc, addend, principal, slot)
    deferred: Vec<(u32, AllocId, u32, Option<DefId>, usize)>,
}

impl<'tcx> Cx<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>, data_base: u32) -> Self {
        Cx {
            tcx,
            fn_ids: HashMap::new(),
            queue: VecDeque::new(),
            funcs: vec![],
            fn_names: vec![],
            table: vec![0],
            table_slot: HashMap::new(),
            data: vec![],
            data_base,
            alloc_addr: HashMap::new(),
            pending_allocs: vec![],
            symbols: None,
            vtables: HashMap::new(),
            used_slots: HashSet::new(),
            any_slot: HashSet::new(),
            deferred: vec![],
        }
    }

    /// Function id for an instance, queueing it for codegen if new.
    pub fn fn_id(&mut self, inst: Instance<'tcx>) -> u32 {
        if let Some(id) = self.fn_ids.get(&inst) {
            return *id;
        }
        let id = self.funcs.len() as u32;
        self.funcs.push(None);
        let name = rustc_middle::ty::print::with_no_trimmed_paths!(format!("{inst}"));
        self.fn_names.push(name);
        self.fn_ids.insert(inst, id);
        self.queue.push_back(inst);
        id
    }

    /// Function-pointer value (table slot) for an instance.
    pub fn fn_ptr(&mut self, inst: Instance<'tcx>) -> u32 {
        let id = self.fn_id(inst);
        if let Some(s) = self.table_slot.get(&id) {
            return *s;
        }
        let slot = self.table.len() as u32;
        self.table.push(id);
        self.table_slot.insert(id, slot);
        slot
    }

    /// Absolute address (or fn-table slot for functions) of a global allocation.
    pub fn alloc_value(&mut self, id: AllocId) -> u32 {
        if let Some(a) = self.alloc_addr.get(&id) {
            return *a;
        }
        let tcx = self.tcx;
        let ga = tcx.global_alloc(id);
        match ga {
            GlobalAlloc::Function { instance } => {
                let s = self.fn_ptr(instance);
                self.alloc_addr.insert(id, s);
                s
            }
            GlobalAlloc::VTable(ty, dyn_ty) => {
                let vt = tcx.vtable_allocation((
                    ty,
                    dyn_ty.principal().map(|p| tcx.instantiate_bound_regions_with_erased(p)),
                ));
                self.vtables.insert(vt, dyn_ty.principal_def_id());
                let a = self.alloc_value(vt);
                self.alloc_addr.insert(id, a);
                a
            }
            GlobalAlloc::Memory(alloc) => {
                let a = alloc.inner();
                let addr = self.reserve(a.size().bytes(), a.align.bytes());
                self.alloc_addr.insert(id, addr);
                self.pending_allocs.push((id, addr));
                addr
            }
            GlobalAlloc::Static(def_id) => {
                if tcx.is_foreign_item(def_id) {
                    tcx.dcx().fatal(format!("extern statics are not supported: {}", tcx.def_path_str(def_id)));
                }
                let alloc = tcx.eval_static_initializer(def_id).unwrap_or_else(|_| tcx.dcx().fatal("static eval failed"));
                let a = alloc.inner();
                let addr = self.reserve(a.size().bytes(), a.align.bytes());
                self.alloc_addr.insert(id, addr);
                // Fill from the static's own allocation.
                let inner_id = tcx.reserve_and_set_memory_alloc(alloc);
                self.pending_allocs.push((inner_id, addr));
                addr
            }
            GlobalAlloc::TypeId { .. } => {
                let addr = self.reserve(1, 1);
                self.alloc_addr.insert(id, addr);
                addr
            }
        }
    }

    fn reserve(&mut self, size: u64, align: u64) -> u32 {
        let align = align.max(1) as usize;
        let mut off = self.data.len();
        off = (off + align - 1) / align * align;
        self.data.resize(off + size.max(1) as usize, 0);
        self.data_base + off as u32
    }

    /// Copy bytes of all reserved allocations into the image, resolving relocations.
    pub fn flush_allocs(&mut self) {
        while let Some((id, addr)) = self.pending_allocs.pop() {
            let tcx = self.tcx;
            let alloc = match tcx.global_alloc(id) {
                GlobalAlloc::Memory(a) => a,
                _ => unreachable!(),
            };
            let a = alloc.inner();
            let len = a.size().bytes() as usize;
            let bytes = a.inspect_with_uninit_and_ptr_outside_interpreter(0..len).to_vec();
            let base = (addr - self.data_base) as usize;
            self.data[base..base + len].copy_from_slice(&bytes);
            let vtable_of = self.vtables.get(&id).copied();
            for (off, prov) in a.provenance().ptrs().iter() {
                let off = off.bytes() as usize;
                let addend = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
                // vtable method slots nobody calls stay null (slot 0 = drop, 1-2 = size/align)
                if let Some(principal) = vtable_of {
                    let slot = off / 4;
                    if slot >= 3
                        && matches!(tcx.global_alloc(prov.alloc_id()), GlobalAlloc::Function { .. })
                        && !self.slot_used(principal, slot)
                    {
                        self.deferred.push((addr + off as u32, prov.alloc_id(), addend, principal, slot));
                        self.data[base + off..base + off + 4].copy_from_slice(&0u32.to_le_bytes());
                        continue;
                    }
                }
                let target = self.alloc_value(prov.alloc_id());
                let v = target.wrapping_add(addend);
                self.data[base + off..base + off + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
    }

    fn slot_used(&self, principal: Option<DefId>, slot: usize) -> bool {
        self.any_slot.contains(&slot) || self.used_slots.contains(&(principal, slot))
    }

    /// Fill deferred vtable slots that became used. Returns true if anything changed.
    pub fn retry_deferred(&mut self) -> bool {
        let mut changed = false;
        let pending = std::mem::take(&mut self.deferred);
        for d in pending {
            let (addr, target, addend, principal, slot) = d;
            if self.slot_used(principal, slot) {
                let v = self.alloc_value(target).wrapping_add(addend);
                let at = (addr - self.data_base) as usize;
                self.data[at..at + 4].copy_from_slice(&v.to_le_bytes());
                changed = true;
            } else {
                self.deferred.push(d);
            }
        }
        changed
    }

    /// Resolve a symbol name to a defining instance anywhere in the crate graph.
    pub fn resolve_symbol(&mut self, name: &str) -> Option<Instance<'tcx>> {
        if self.symbols.is_none() {
            let tcx = self.tcx;
            let mut map = HashMap::new();
            let mut crates: Vec<CrateNum> = tcx.crates(()).to_vec();
            crates.push(LOCAL_CRATE);
            for cnum in crates {
                for (sym, _) in tcx.exported_non_generic_symbols(cnum).iter() {
                    if let ExportedSymbol::NonGeneric(def_id) = sym {
                        if tcx.is_foreign_item(*def_id) {
                            continue;
                        }
                        if !matches!(tcx.def_kind(*def_id), rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn) {
                            continue;
                        }
                        let inst = Instance::mono(tcx, *def_id);
                        map.insert(tcx.symbol_name(inst).name.to_string(), inst);
                    }
                }
            }
            self.symbols = Some(map);
        }
        self.symbols.as_ref().unwrap().get(name).copied()
    }

    pub fn into_program(self, exports: Vec<(String, u32)>, stack_top: u32) -> Program {
        let data_end = self.data_base + self.data.len() as u32;
        let heap_base = (data_end + 15) & !15;
        Program {
            funcs: self.funcs.into_iter().map(|f| f.expect("function not lowered")).collect(),
            table: self.table,
            data: self.data,
            data_base: self.data_base,
            stack_top,
            heap_base,
            initial_mem: heap_base + 64 * 1024,
            exports,
        }
    }
}

pub fn typing_env<'tcx>() -> ty::TypingEnv<'tcx> {
    ty::TypingEnv::fully_monomorphized()
}
