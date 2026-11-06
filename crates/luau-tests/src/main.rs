//! Differential tests: each `tests/run/*.rs` defines `pub fn run(w: &mut dyn core::fmt::Write)`.
//! It is compiled natively (expected output) and to Luau (run under Lune); outputs must match.
//!
//! Usage: cargo run -p luau-tests [-- <filter>...]

use std::path::{Path, PathBuf};
use std::process::Command;

const TOOLCHAIN: &str = "nightly-2026-06-26";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn sh(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("spawn {cmd:?}: {e}"))?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(s)
    } else {
        Err(format!("{s}{}", String::from_utf8_lossy(&out.stderr)))
    }
}

fn native(root: &Path, test: &Path, name: &str) -> Result<String, String> {
    let dir = root.join("target/luau-tests/native");
    std::fs::create_dir_all(&dir).unwrap();
    let main = dir.join(format!("{name}.rs"));
    std::fs::write(
        &main,
        format!(
            "#![allow(dead_code, unused)]\nextern crate alloc;\nmod t {{ include!({:?}); }}\nfn main() {{ let mut s = String::new(); t::run(&mut s); print!(\"{{s}}\"); }}\n",
            test
        ),
    )
    .unwrap();
    let bin = dir.join(name);
    sh(Command::new("rustc").args(["--edition", "2024", "-O", "-o"]).arg(&bin).arg(&main))?;
    sh(&mut Command::new(&bin))
}

fn luau(root: &Path, test: &Path, name: &str) -> Result<String, String> {
    let krate = root.join("target/luau-tests/crates").join(name);
    std::fs::create_dir_all(krate.join("src")).unwrap();
    std::fs::write(
        krate.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nluau_rt = {{ path = {:?} }}\n\n[profile.release]\npanic = \"abort\"\n\n[workspace]\n",
            root.join("crates/luau_rt")
        ),
    )
    .unwrap();
    std::fs::write(
        krate.join("src/lib.rs"),
        format!(
            "#![no_std]\n#![allow(dead_code, unused)]\nextern crate alloc;\nextern crate luau_rt;\nmod t {{ include!({:?}); }}\n#[unsafe(no_mangle)]\npub extern \"C\" fn main() {{ let mut o = luau_rt::Out::new(); t::run(&mut o); o.flush(); }}\n",
            test
        ),
    )
    .unwrap();
    let backend = root.join("target/debug/librustc_codegen_luau.so");
    let target_dir = root.join("target/luau-tests/target");
    sh(Command::new("cargo")
        .arg(format!("+{TOOLCHAIN}"))
        .args(["build", "--release", "-q", "-Zbuild-std=core,alloc", "-Zjson-target-spec", "--target"])
        .arg(root.join("targets/luau32-unknown-roblox.json"))
        .current_dir(&krate)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env(
            "RUSTFLAGS",
            format!("-Zcodegen-backend={} -Zalways-encode-mir -Zmir-opt-level=3", backend.display()),
        ))?;
    let module = target_dir.join(format!("luau32-unknown-roblox/release/{name}.luau"));
    let out = Command::new("lune")
        .arg("run")
        .arg(root.join("tests/runner.luau"))
        .arg(&module)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    let s = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() { Ok(s) } else { Err(s) }
}

fn main() {
    let root = root();
    let filters: Vec<String> = std::env::args().skip(1).collect();
    sh(Command::new("cargo").args(["build", "-q", "-p", "rustc_codegen_luau"]).current_dir(&root)).expect("build backend");
    let mut tests: Vec<PathBuf> = std::fs::read_dir(root.join("tests/run"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .collect();
    tests.sort();
    let (mut pass, mut fail) = (0, 0);
    for t in &tests {
        let name = t.file_stem().unwrap().to_str().unwrap().to_string();
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        let expected = match native(&root, t, &name) {
            Ok(s) => s,
            Err(e) => {
                println!("FAIL {name} (native build/run)\n{e}");
                fail += 1;
                continue;
            }
        };
        match luau(&root, t, &name) {
            Ok(got) if got == expected => {
                println!("PASS {name}");
                pass += 1;
            }
            Ok(got) => {
                println!("FAIL {name}: output mismatch");
                for (i, (e, g)) in expected.lines().zip(got.lines()).enumerate() {
                    if e != g {
                        println!("  line {}: expected {e:?}\n          got      {g:?}", i + 1);
                        break;
                    }
                }
                if expected.lines().count() != got.lines().count() {
                    println!("  expected {} lines, got {}", expected.lines().count(), got.lines().count());
                }
                fail += 1;
            }
            Err(e) => {
                let e: String = e.lines().take(25).collect::<Vec<_>>().join("\n");
                println!("FAIL {name}\n{e}");
                fail += 1;
            }
        }
    }
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
