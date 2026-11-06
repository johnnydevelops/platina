//! LIR: a small typed CFG IR whose values map 1:1 onto Luau values.
//!
//! Every variable is either a Luau `number` or a Luau `boolean`. Integer semantics (width,
//! signedness, wrapping) are carried by operations, not by variable types: an integer of
//! `bits <= 32` is always kept in its *canonical* form (its mathematical value inside the
//! type's range), and arithmetic that may leave that range is followed by an explicit
//! [`Un::Wrap`], which optimization passes are free to delay or delete.

use std::fmt;

pub type VarId = u32;
pub type BlockId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Num,
    Bool,
}

/// Integer kind (width <= 32 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IntKind {
    pub bits: u8,
    pub signed: bool,
}

impl IntKind {
    pub const U8: IntKind = IntKind { bits: 8, signed: false };
    pub const I8: IntKind = IntKind { bits: 8, signed: true };
    pub const U16: IntKind = IntKind { bits: 16, signed: false };
    pub const I16: IntKind = IntKind { bits: 16, signed: true };
    pub const U32: IntKind = IntKind { bits: 32, signed: false };
    pub const I32: IntKind = IntKind { bits: 32, signed: true };

    pub fn min(self) -> f64 {
        if self.signed { -(2f64.powi(self.bits as i32 - 1)) } else { 0.0 }
    }
    pub fn max(self) -> f64 {
        if self.signed { 2f64.powi(self.bits as i32 - 1) - 1.0 } else { 2f64.powi(self.bits as i32) - 1.0 }
    }
    pub fn modulus(self) -> f64 {
        2f64.powi(self.bits as i32)
    }
    /// Wrap a compile-time value into canonical range.
    pub fn wrap_const(self, v: f64) -> f64 {
        let m = self.modulus();
        let mut r = v.rem_euclid(m);
        if self.signed && r > self.max() {
            r -= m;
        }
        r
    }
}

/// Memory access kinds for `buffer.read*` / `buffer.write*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mem {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    F64,
    /// u8 in memory, Luau boolean in registers.
    Bool,
}

impl Mem {
    pub fn size(self) -> u32 {
        match self {
            Mem::U8 | Mem::I8 | Mem::Bool => 1,
            Mem::U16 | Mem::I16 => 2,
            Mem::U32 | Mem::I32 | Mem::F32 => 4,
            Mem::F64 => 8,
        }
    }
    pub fn for_int(k: IntKind) -> Mem {
        match (k.bits, k.signed) {
            (8, false) => Mem::U8,
            (8, true) => Mem::I8,
            (16, false) => Mem::U16,
            (16, true) => Mem::I16,
            (32, false) => Mem::U32,
            (32, true) => Mem::I32,
            _ => panic!("no memory kind for {k:?}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operand {
    Var(VarId),
    Num(f64),
    Bool(bool),
}

impl Operand {
    pub fn as_var(&self) -> Option<VarId> {
        match self {
            Operand::Var(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Operand::Num(n) => Some(*n),
            _ => None,
        }
    }
}

impl From<VarId> for Operand {
    fn from(v: VarId) -> Self {
        Operand::Var(v)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bin {
    // exact float arithmetic on numbers
    Add,
    Sub,
    Mul,
    Div,
    /// Luau `//` (floor division)
    FloorDiv,
    /// Luau `%` (floored modulo)
    Mod,
    /// `math.fmod` (truncated remainder)
    FMod,
    Pow,
    // comparisons -> boolean
    Eq,
    Ne,
    Lt,
    Le,
    // boolean logic (operands booleans)
    And,
    Or,
    // bit32 (results are u32 canonical)
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    Sar,
    Rol,
    Ror,
    /// low 32 bits of a 32x32 product, u32 result
    IMul32,
    /// truncating signed division (operands canonical signed ints)
    IDivTrunc,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Un {
    Neg,
    Not,
    /// bit32.bnot (u32 result)
    BNot,
    /// Reduce to canonical range of the int kind.
    Wrap(IntKind),
    /// Like `Wrap`, but the operand is known to be within one modulus of the range, so a
    /// conditional correction suffices. `above`/`below`: which side can be exceeded.
    WrapNear(IntKind, bool, bool),
    /// Round to nearest f32.
    F32Round,
    Floor,
    Ceil,
    Trunc,
    Sqrt,
    Abs,
    /// count leading zeros (32-bit)
    Clz,
    Ctz,
    Popcnt,
    BSwap,
    /// bool -> 0/1
    BoolToNum,
    /// number ~= 0
    NumToBool,
    /// Float to int with Rust `as` semantics (saturating, NaN -> 0).
    FToI(IntKind),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rv {
    Use(Operand),
    Bin(Bin, Operand, Operand),
    Un(Un, Operand),
    Load { mem: Mem, addr: Operand, off: i32 },
    /// Arbitrary pure call to a global builtin, e.g. `math.sin`.
    Builtin(&'static str, Vec<Operand>),
    Select(Operand, Operand, Operand),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Callee {
    Func(u32),
    /// Indirect call through function table by index value.
    Indirect(Operand),
    /// Call to a runtime/prelude helper or global (impure).
    Extern(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Assign(VarId, Rv),
    Store { mem: Mem, addr: Operand, off: i32, val: Operand },
    Call { dests: Vec<VarId>, callee: Callee, args: Vec<Operand> },
    /// memmove(dst, src, len)
    Copy { dst: Operand, src: Operand, len: Operand },
    Fill { dst: Operand, val: Operand, len: Operand },
    /// Set the shadow stack pointer.
    SetSp(Operand),
    /// Read the shadow stack pointer.
    GetSp(VarId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Goto(BlockId),
    Br(Operand, BlockId, BlockId),
    /// Switch on a number; cases are (value, target).
    Switch(Operand, Vec<(f64, BlockId)>, BlockId),
    Return(Vec<Operand>),
    /// `error(msg)`; message parts are concatenated (`Operand`s are converted with tostring).
    Trap(Vec<TrapPart>),
    Unreachable,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrapPart {
    Str(String),
    Val(Operand),
    /// string read from memory (ptr, len)
    MemStr(Operand, Operand),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub term: Term,
}

#[derive(Clone, Debug)]
pub struct VarInfo {
    pub ty: Ty,
    pub name: Option<String>,
    /// Declared value range: the variable always holds an integer in `[lo, hi]`
    /// (canonical form of a Rust integer type).
    pub range: Option<(f64, f64)>,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub params: Vec<VarId>,
    pub rets: Vec<Ty>,
    pub vars: Vec<VarInfo>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
    /// Shadow-stack objects (one per stack-allocated Rust local/temporary):
    /// `(sp snapshot var, start, end)` as byte offsets relative to that snapshot. Pointers
    /// derived from one object can only access that object (Rust provenance).
    pub frame_objects: Vec<(VarId, i64, i64)>,
}

impl Function {
    pub fn new(name: impl Into<String>) -> Self {
        Function { name: name.into(), params: vec![], rets: vec![], vars: vec![], blocks: vec![], entry: 0, frame_objects: vec![] }
    }
    pub fn new_var(&mut self, ty: Ty) -> VarId {
        self.vars.push(VarInfo { ty, name: None, range: None });
        (self.vars.len() - 1) as VarId
    }
    pub fn new_block(&mut self) -> BlockId {
        self.blocks.push(Block { stmts: vec![], term: Term::Unreachable });
        (self.blocks.len() - 1) as BlockId
    }
    pub fn ty(&self, v: VarId) -> Ty {
        self.vars[v as usize].ty
    }
}

impl Term {
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Term::Goto(b) => vec![*b],
            Term::Br(_, a, b) => vec![*a, *b],
            Term::Switch(_, cases, d) => {
                let mut v: Vec<BlockId> = cases.iter().map(|c| c.1).collect();
                v.push(*d);
                v
            }
            Term::Return(_) | Term::Trap(_) | Term::Unreachable => vec![],
        }
    }
    pub fn successors_mut(&mut self) -> Vec<&mut BlockId> {
        match self {
            Term::Goto(b) => vec![b],
            Term::Br(_, a, b) => vec![a, b],
            Term::Switch(_, cases, d) => {
                let mut v: Vec<&mut BlockId> = cases.iter_mut().map(|c| &mut c.1).collect();
                v.push(d);
                v
            }
            Term::Return(_) | Term::Trap(_) | Term::Unreachable => vec![],
        }
    }
    pub fn operands_mut(&mut self) -> Vec<&mut Operand> {
        match self {
            Term::Br(c, _, _) | Term::Switch(c, _, _) => vec![c],
            Term::Return(v) => v.iter_mut().collect(),
            Term::Trap(parts) => parts
                .iter_mut()
                .flat_map(|p| match p {
                    TrapPart::Str(_) => vec![],
                    TrapPart::Val(o) => vec![o],
                    TrapPart::MemStr(a, b) => vec![a, b],
                })
                .collect(),
            _ => vec![],
        }
    }
}

impl Rv {
    pub fn operands(&self) -> Vec<&Operand> {
        match self {
            Rv::Use(a) | Rv::Un(_, a) => vec![a],
            Rv::Bin(_, a, b) => vec![a, b],
            Rv::Load { addr, .. } => vec![addr],
            Rv::Builtin(_, args) => args.iter().collect(),
            Rv::Select(a, b, c) => vec![a, b, c],
        }
    }
    pub fn operands_mut(&mut self) -> Vec<&mut Operand> {
        match self {
            Rv::Use(a) | Rv::Un(_, a) => vec![a],
            Rv::Bin(_, a, b) => vec![a, b],
            Rv::Load { addr, .. } => vec![addr],
            Rv::Builtin(_, args) => args.iter_mut().collect(),
            Rv::Select(a, b, c) => vec![a, b, c],
        }
    }
    pub fn reads_memory(&self) -> bool {
        matches!(self, Rv::Load { .. })
    }
}

impl Stmt {
    pub fn def(&self) -> Vec<VarId> {
        match self {
            Stmt::Assign(v, _) | Stmt::GetSp(v) => vec![*v],
            Stmt::Call { dests, .. } => dests.clone(),
            _ => vec![],
        }
    }
    pub fn operands(&self) -> Vec<&Operand> {
        match self {
            Stmt::Assign(_, rv) => rv.operands(),
            Stmt::Store { addr, val, .. } => vec![addr, val],
            Stmt::Call { callee, args, .. } => {
                let mut v: Vec<&Operand> = args.iter().collect();
                if let Callee::Indirect(o) = callee {
                    v.push(o);
                }
                v
            }
            Stmt::Copy { dst, src, len } => vec![dst, src, len],
            Stmt::Fill { dst, val, len } => vec![dst, val, len],
            Stmt::SetSp(o) => vec![o],
            Stmt::GetSp(_) => vec![],
        }
    }
    pub fn operands_mut(&mut self) -> Vec<&mut Operand> {
        match self {
            Stmt::Assign(_, rv) => rv.operands_mut(),
            Stmt::Store { addr, val, .. } => vec![addr, val],
            Stmt::Call { callee, args, .. } => {
                let mut v: Vec<&mut Operand> = args.iter_mut().collect();
                if let Callee::Indirect(o) = callee {
                    v.push(o);
                }
                v
            }
            Stmt::Copy { dst, src, len } => vec![dst, src, len],
            Stmt::Fill { dst, val, len } => vec![dst, val, len],
            Stmt::SetSp(o) => vec![o],
            Stmt::GetSp(_) => vec![],
        }
    }
    /// Has an effect other than defining its destination vars.
    pub fn has_side_effects(&self) -> bool {
        !matches!(self, Stmt::Assign(..) | Stmt::GetSp(_))
    }
}

impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Operand::Var(v) => write!(f, "v{v}"),
            Operand::Num(n) => write!(f, "{n}"),
            Operand::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// A whole program: functions, the initial memory image, and exports.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub funcs: Vec<Function>,
    /// Indices of functions that may be called indirectly; slot `i` holds function `table[i]`.
    pub table: Vec<u32>,
    /// Initial memory contents, placed at `data_base`.
    pub data: Vec<u8>,
    pub data_base: u32,
    /// Stack top (initial SP); stack grows down towards `stack_limit`.
    pub stack_top: u32,
    /// First free byte after static data (heap start).
    pub heap_base: u32,
    pub initial_mem: u32,
    pub exports: Vec<(String, u32)>,
}
