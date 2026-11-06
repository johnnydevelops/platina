//! `cargo roblox` — build Rust crates into Luau ModuleScripts for Roblox.
//!
//! ```text
//! cargo roblox build [--release] [-o <file.luau>] [-p <package>] [--no-std] [-- <extra cargo args>]
//! ```
//!
//! The crate is built as a `cdylib` for the `luau32-unknown-roblox` target with the
//! `rustc_codegen_luau` backend; `#[no_mangle] pub extern "C" fn`s become the module's exports.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const TARGET_JSON: &str = include_str!("../../../targets/luau32-unknown-roblox.json");
const TOOLCHAIN_TOML: &str = include_str!("../../../rust-toolchain.toml");
const BACKEND_FILE: &str = if cfg!(target_os = "macos") {
    "librustc_codegen_luau.dylib"
} else if cfg!(windows) {
    "rustc_codegen_luau.dll"
} else {
    "librustc_codegen_luau.so"
};

fn toolchain() -> String {
    TOOLCHAIN_TOML
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = ").map(|v| v.trim_matches('"').to_string()))
        .expect("channel in rust-toolchain.toml")
}

fn usage() -> ExitCode {
    eprintln!(
        "cargo roblox — compile Rust to Luau for Roblox\n\n\
         USAGE:\n    cargo roblox build [OPTIONS] [-- <cargo args>]\n\n\
         OPTIONS:\n\
         \x20   --release          build with optimizations (recommended)\n\
         \x20   -o, --out <FILE>   where to write the .luau module (default: out/<crate>.luau)\n\
         \x20   -p, --package <P>  package to build\n\
         \x20   --no-std           only build core + alloc (for #![no_std] crates)\n\
         \x20   -v, --verbose      print the cargo command\n\n\
         ENVIRONMENT:\n\
         \x20   LUAU_CODEGEN_BACKEND   path to {BACKEND_FILE}"
    );
    ExitCode::from(2)
}

/// Locate the codegen backend dylib.
fn find_backend() -> Option<PathBuf> {
    if let Ok(p) = env::var("LUAU_CODEGEN_BACKEND") {
        return Some(PathBuf::from(p));
    }
    let mut candidates = vec![];
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(BACKEND_FILE));
            candidates.push(dir.join("../lib").join(BACKEND_FILE));
        }
    }
    // when run from a checkout of this repository
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for profile in ["release", "debug"] {
        candidates.push(repo.join("target").join(profile).join(BACKEND_FILE));
    }
    candidates.into_iter().find(|p| p.exists()).map(|p| p.canonicalize().unwrap_or(p))
}

/// Target directory and package name from `cargo metadata`.
fn cargo_metadata(tc: &str, package: Option<&str>) -> Result<(PathBuf, String), String> {
    let out = Command::new("cargo")
        .arg(format!("+{tc}"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let target_dir = json["target_directory"].as_str().ok_or("no target_directory in cargo metadata")?;
    let cwd_manifest = env::current_dir().unwrap().join("Cargo.toml");
    let packages = json["packages"].as_array().ok_or("no packages in cargo metadata")?;
    let pick = packages
        .iter()
        .find(|p| match package {
            Some(name) => p["name"] == name,
            None => p["manifest_path"].as_str().map(Path::new) == Some(cwd_manifest.as_path()),
        })
        .or(if package.is_none() && packages.len() == 1 { packages.first() } else { None })
        .ok_or("package not found (use -p <package>)")?;
    let name = pick["name"].as_str().unwrap_or_default().to_string();
    Ok((PathBuf::from(target_dir), name))
}

fn build(args: &[String]) -> Result<(), String> {
    let mut release = false;
    let mut out: Option<PathBuf> = None;
    let mut package: Option<String> = None;
    let mut no_std = false;
    let mut verbose = false;
    let mut extra: Vec<String> = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" | "-r" => release = true,
            "-o" | "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a path")?)),
            "-p" | "--package" => package = Some(it.next().ok_or("--package needs a name")?.clone()),
            "--no-std" => no_std = true,
            "-v" | "--verbose" => verbose = true,
            "--" => {
                extra.extend(it.by_ref().cloned());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let tc = toolchain();
    let backend = find_backend().ok_or_else(|| {
        format!("could not find {BACKEND_FILE}; build it with `cargo build --release -p rustc_codegen_luau` or set LUAU_CODEGEN_BACKEND")
    })?;
    let (target_dir, name) = cargo_metadata(&tc, package.as_deref())?;
    // target spec next to the build output (rustc names the target after the file stem)
    let spec_dir = target_dir.join("luau");
    std::fs::create_dir_all(&spec_dir).map_err(|e| e.to_string())?;
    let spec = spec_dir.join("luau32-unknown-roblox.json");
    if std::fs::read_to_string(&spec).ok().as_deref() != Some(TARGET_JSON) {
        std::fs::write(&spec, TARGET_JSON).map_err(|e| e.to_string())?;
    }
    let mut rustflags = env::var("RUSTFLAGS").unwrap_or_default();
    rustflags.push_str(&format!(" -Zcodegen-backend={} -Zalways-encode-mir -Zmir-opt-level=3", backend.display()));
    let mut cmd = Command::new("cargo");
    cmd.arg(format!("+{tc}"))
        .args(["rustc", "--lib", "--crate-type", "cdylib", "-Zjson-target-spec", "--target"])
        .arg(&spec)
        .arg(if no_std { "-Zbuild-std=core,alloc" } else { "-Zbuild-std=std,panic_abort" })
        .env("RUSTFLAGS", rustflags.trim());
    if release {
        cmd.arg("--release");
    }
    if let Some(p) = &package {
        cmd.args(["-p", p]);
    }
    cmd.args(&extra);
    if verbose {
        eprintln!("running {cmd:?}");
    }
    let status = cmd.status().map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err("build failed".into());
    }
    let profile = if release { "release" } else { "debug" };
    let artifact = target_dir
        .join("luau32-unknown-roblox")
        .join(profile)
        .join(format!("{}.luau", name.replace('-', "_")));
    if !artifact.exists() {
        return Err(format!("expected output {} was not produced", artifact.display()));
    }
    let out = out.unwrap_or_else(|| PathBuf::from("out").join(format!("{name}.luau")));
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    std::fs::copy(&artifact, &out).map_err(|e| format!("copy to {}: {e}", out.display()))?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    eprintln!("    Finished {} ({} KiB)", out.display(), size / 1024);
    Ok(())
}

fn main() -> ExitCode {
    // invoked as `cargo roblox ...` (argv[1] == "roblox") or directly as `cargo-roblox ...`
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("roblox") {
        args.remove(0);
    }
    match args.first().map(|s| s.as_str()) {
        Some("build") => match build(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Some("--version" | "-V") => {
            println!("cargo-roblox {} (toolchain {})", env!("CARGO_PKG_VERSION"), toolchain());
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
