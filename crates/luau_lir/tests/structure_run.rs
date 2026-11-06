//! Builds small CFGs, emits Luau, runs them with the `luau` CLI and checks results.
use luau_lir::ir::*;
use std::process::Command;

fn run(p: &Program, calls: &str) -> String {
    let src = luau_lir::emit_program(p).unwrap();
    let dir = std::env::temp_dir().join(format!("lir_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("t{}.luau", rand_id()));
    std::fs::write(&path, format!("local M = (function()\n{src}\nend)()\n{calls}")).unwrap();
    let out = Command::new("luau").arg("--codegen").arg(&path).output().expect("luau CLI");
    let s = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "luau failed:\n{s}\n--- source ---\n{src}");
    s
}

fn rand_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn prog(f: Function) -> Program {
    Program { funcs: vec![f], exports: vec![("f".into(), 0)], initial_mem: 64, stack_top: 64, ..Default::default() }
}

/// sum of i*i for i in 0..n, with an early exit when the sum exceeds `cap`
#[test]
fn loop_with_early_exit() {
    let mut f = Function::new("f");
    let n = f.new_var(Ty::Num);
    let cap = f.new_var(Ty::Num);
    f.params = vec![n, cap];
    f.rets = vec![Ty::Num];
    let i = f.new_var(Ty::Num);
    let s = f.new_var(Ty::Num);
    let c = f.new_var(Ty::Bool);
    let c2 = f.new_var(Ty::Bool);
    let t = f.new_var(Ty::Num);
    let b0 = f.new_block();
    let head = f.new_block();
    let body = f.new_block();
    let cont = f.new_block();
    let exit = f.new_block();
    let early = f.new_block();
    f.blocks[b0 as usize].stmts = vec![Stmt::Assign(i, Rv::Use(Operand::Num(0.0))), Stmt::Assign(s, Rv::Use(Operand::Num(0.0)))];
    f.blocks[b0 as usize].term = Term::Goto(head);
    f.blocks[head as usize].stmts = vec![Stmt::Assign(c, Rv::Bin(Bin::Lt, i.into(), n.into()))];
    f.blocks[head as usize].term = Term::Br(c.into(), body, exit);
    f.blocks[body as usize].stmts = vec![
        Stmt::Assign(t, Rv::Bin(Bin::Mul, i.into(), i.into())),
        Stmt::Assign(s, Rv::Bin(Bin::Add, s.into(), t.into())),
        Stmt::Assign(c2, Rv::Bin(Bin::Lt, cap.into(), s.into())),
    ];
    f.blocks[body as usize].term = Term::Br(c2.into(), early, cont);
    f.blocks[cont as usize].stmts = vec![Stmt::Assign(i, Rv::Bin(Bin::Add, i.into(), Operand::Num(1.0)))];
    f.blocks[cont as usize].term = Term::Goto(head);
    f.blocks[exit as usize].term = Term::Return(vec![s.into()]);
    f.blocks[early as usize].term = Term::Return(vec![Operand::Num(-1.0)]);
    let out = run(&prog(f), "print(M.f(10, 1000), M.f(100, 1000))");
    assert_eq!(out.trim(), "285\t-1");
}

/// nested loops with a break out of both (multi-level exit), then a merge
#[test]
fn nested_multilevel_break() {
    // for a in 0..n { for b in 0..n { if a*b == target { found = a*100+b; goto done } } } found = -1; done: return found+0
    let mut f = Function::new("f");
    let n = f.new_var(Ty::Num);
    let target = f.new_var(Ty::Num);
    f.params = vec![n, target];
    let a = f.new_var(Ty::Num);
    let b = f.new_var(Ty::Num);
    let r = f.new_var(Ty::Num);
    let c = f.new_var(Ty::Bool);
    let bb: Vec<BlockId> = (0..9).map(|_| f.new_block()).collect();
    let [entry, ohead, oinit, ihead, ibody, inext, onext, notfound, done] = bb[..] else { unreachable!() };
    let set = |f: &mut Function, b: BlockId, stmts: Vec<Stmt>, t: Term| {
        f.blocks[b as usize].stmts = stmts;
        f.blocks[b as usize].term = t;
    };
    set(&mut f, entry, vec![Stmt::Assign(a, Rv::Use(Operand::Num(0.0)))], Term::Goto(ohead));
    set(&mut f, ohead, vec![Stmt::Assign(c, Rv::Bin(Bin::Lt, a.into(), n.into()))], Term::Br(c.into(), oinit, notfound));
    set(&mut f, oinit, vec![Stmt::Assign(b, Rv::Use(Operand::Num(0.0)))], Term::Goto(ihead));
    set(&mut f, ihead, vec![Stmt::Assign(c, Rv::Bin(Bin::Lt, b.into(), n.into()))], Term::Br(c.into(), ibody, onext));
    let p = f.new_var(Ty::Num);
    set(
        &mut f,
        ibody,
        vec![
            Stmt::Assign(p, Rv::Bin(Bin::Mul, a.into(), b.into())),
            Stmt::Assign(c, Rv::Bin(Bin::Eq, p.into(), target.into())),
            Stmt::Assign(r, Rv::Bin(Bin::Add, Operand::Var(a), Operand::Num(0.0))),
        ],
        Term::Br(c.into(), done, inext),
    );
    set(&mut f, inext, vec![Stmt::Assign(b, Rv::Bin(Bin::Add, b.into(), Operand::Num(1.0)))], Term::Goto(ihead));
    set(&mut f, onext, vec![Stmt::Assign(a, Rv::Bin(Bin::Add, a.into(), Operand::Num(1.0)))], Term::Goto(ohead));
    set(&mut f, notfound, vec![Stmt::Assign(r, Rv::Use(Operand::Num(-1.0)))], Term::Goto(done));
    let r2 = f.new_var(Ty::Num);
    set(&mut f, done, vec![Stmt::Assign(r2, Rv::Bin(Bin::Mul, r.into(), Operand::Num(100.0)))], Term::Return(vec![r2.into(), b.into()]));
    let out = run(&prog(f), "print(M.f(10, 42))\nprint(M.f(5, 99))");
    assert_eq!(out.trim(), "600\t7\n-100\t5");
}

#[test]
fn switch_dispatch() {
    let mut f = Function::new("f");
    let x = f.new_var(Ty::Num);
    f.params = vec![x];
    let entry = f.new_block();
    let a = f.new_block();
    let b = f.new_block();
    let d = f.new_block();
    let m = f.new_block();
    let r = f.new_var(Ty::Num);
    f.blocks[entry as usize].term = Term::Switch(x.into(), vec![(1.0, a), (2.0, b), (3.0, a)], d);
    f.blocks[a as usize].stmts = vec![Stmt::Assign(r, Rv::Use(Operand::Num(10.0)))];
    f.blocks[a as usize].term = Term::Goto(m);
    f.blocks[b as usize].stmts = vec![Stmt::Assign(r, Rv::Use(Operand::Num(20.0)))];
    f.blocks[b as usize].term = Term::Goto(m);
    f.blocks[d as usize].term = Term::Return(vec![Operand::Num(0.0)]);
    f.blocks[m as usize].term = Term::Return(vec![r.into()]);
    let out = run(&prog(f), "print(M.f(1), M.f(2), M.f(3), M.f(4))");
    assert_eq!(out.trim(), "10\t20\t10\t0");
}
