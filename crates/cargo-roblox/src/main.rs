//! `cargo roblox` — build Rust crates into Luau ModuleScripts for Roblox.
//!
//! ```text
//! cargo roblox setup                 # install the pinned nightly + build the backend
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

/// The source tree this binary was built from: the repository checkout, or the copy that
/// `cargo install --git` / `--path` built from. `setup` builds the backend from it.
fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Identifies the backend sources this binary expects (the checkout path is unique per
/// installed revision).
fn source_stamp() -> String {
    format!("{} {}", env!("CARGO_PKG_VERSION"), env!("CARGO_MANIFEST_DIR"))
}

/// `<cargo home>/lib`, next to `<cargo home>/bin` where `cargo install` puts this binary.
fn install_lib_dir() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    Some(exe.parent()?.join("../lib/rustc_codegen_luau"))
}

fn toolchain_components() -> Vec<String> {
    TOOLCHAIN_TOML
        .lines()
        .find_map(|l| l.trim().strip_prefix("components = "))
        .map(|v| v.trim_matches(|c| c == '[' || c == ']').split(',').map(|c| c.trim().trim_matches('"').to_string()).filter(|c| !c.is_empty()).collect())
        .unwrap_or_default()
}

/// Install the pinned toolchain and build the codegen backend into `<cargo home>/lib`.
fn setup() -> Result<PathBuf, String> {
    let tc = toolchain();
    let root = source_root();
    if !root.join("crates/rustc_codegen_luau/Cargo.toml").exists() {
        return Err(format!(
            "backend sources not found at {} (reinstall with `cargo install --git https://github.com/johnnydevelops/rustc-codegen-luau cargo-roblox`)",
            root.display()
        ));
    }
    eprintln!("    Installing toolchain {tc} (rustc-dev, rust-src, llvm-tools)");
    let status = Command::new("rustup")
        .args(["toolchain", "install", &tc, "--profile", "minimal", "--no-self-update"])
        .args(toolchain_components().iter().flat_map(|c| ["--component", c.as_str()]))
        .status()
        .map_err(|e| format!("failed to run rustup (is it installed? https://rustup.rs): {e}"))?;
    if !status.success() {
        return Err("rustup toolchain install failed".into());
    }
    let lib_dir = install_lib_dir().ok_or("cannot locate the install directory")?;
    std::fs::create_dir_all(&lib_dir).map_err(|e| format!("create {}: {e}", lib_dir.display()))?;
    let lib_dir = lib_dir.canonicalize().unwrap_or(lib_dir);
    let target_dir = lib_dir.join("target");
    eprintln!("    Building rustc_codegen_luau (takes a minute)");
    let status = Command::new("cargo")
        .arg(format!("+{tc}"))
        .args(["build", "--release", "-p", "rustc_codegen_luau", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err("building the backend failed".into());
    }
    let built = target_dir.join("release").join(BACKEND_FILE);
    let dest = lib_dir.join(BACKEND_FILE);
    std::fs::copy(&built, &dest).map_err(|e| format!("copy {} -> {}: {e}", built.display(), dest.display()))?;
    std::fs::write(lib_dir.join("stamp"), source_stamp()).map_err(|e| e.to_string())?;
    // the intermediate build is large; keep only the backend
    let _ = std::fs::remove_dir_all(&target_dir);
    eprintln!("    Installed {}", dest.display());
    Ok(dest)
}

fn toolchain() -> String {
    TOOLCHAIN_TOML
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = ").map(|v| v.trim_matches('"').to_string()))
        .expect("channel in rust-toolchain.toml")
}

fn usage() -> ExitCode {
    eprintln!(
        "cargo roblox — compile Rust to Luau for Roblox\n\n\
         USAGE:\n    cargo roblox build [OPTIONS] [-- <cargo args>]\n    cargo roblox setup    install the pinned nightly and build the backend\n\n\
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
    // next to the binary (a build inside this repository: target/{debug,release})
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(BACKEND_FILE));
        }
    }
    // installed by `cargo roblox setup`, for this exact source revision
    if let Some(lib) = install_lib_dir() {
        if std::fs::read_to_string(lib.join("stamp")).ok().as_deref() == Some(source_stamp().as_str()) {
            candidates.push(lib.join(BACKEND_FILE));
        }
    }
    // a checkout of this repository with the backend already built
    for profile in ["release", "debug"] {
        candidates.push(source_root().join("target").join(profile).join(BACKEND_FILE));
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

/// Path prefixes remapped in panic locations: the std sources, the cargo registry, and the
/// current project.
fn remaps(tc: &str) -> Vec<(PathBuf, &'static str)> {
    let mut v = vec![];
    if let Ok(o) = Command::new("rustc").arg(format!("+{tc}")).args(["--print", "sysroot"]).output() {
        let sysroot = String::from_utf8_lossy(&o.stdout).trim().to_string();
        v.push((Path::new(&sysroot).join("lib/rustlib/src/rust/library"), "/rust"));
    }
    let cargo_home = env::var("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env::var("HOME").unwrap_or_default()).join(".cargo")
    });
    if let Ok(rd) = std::fs::read_dir(cargo_home.join("registry/src")) {
        for e in rd.flatten() {
            v.push((e.path(), "/crates"));
        }
    }
    if let Ok(cwd) = env::current_dir() {
        v.push((cwd, "."));
    }
    v
}

fn build(args: &[String]) -> Result<(), String> {
    let mut release = false;
    let mut out: Option<PathBuf> = None;
    let mut package: Option<String> = None;
    let mut no_std = false;
    let mut verbose = false;
    let mut minify: Option<bool> = None;
    let mut tiny = false;
    let mut extra: Vec<String> = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" | "-r" => release = true,
            "-o" | "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a path")?)),
            "-p" | "--package" => package = Some(it.next().ok_or("--package needs a name")?.clone()),
            "--no-std" => no_std = true,
            "-v" | "--verbose" => verbose = true,
            "--minify" => minify = Some(true),
            "--no-minify" => minify = Some(false),
            "--tiny" => tiny = true,
            "--" => {
                extra.extend(it.by_ref().cloned());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let tc = toolchain();
    let backend = match find_backend() {
        Some(b) => b,
        None => {
            eprintln!("    No codegen backend found for this version of cargo-roblox; running setup");
            setup()?
        }
    };
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
    if tiny {
        rustflags.push_str(" -Zunstable-options -Cpanic=immediate-abort");
    }
    // short, machine-independent source paths in panic messages
    for (from, to) in remaps(&tc) {
        rustflags.push_str(&format!(" --remap-path-prefix={}={}", from.display(), to));
    }
    let mut cmd = Command::new("cargo");
    cmd.arg(format!("+{tc}"))
        .args(["rustc", "--lib", "--crate-type", "cdylib", "-Zjson-target-spec", "--target"])
        .arg(&spec)
        .arg(if no_std { "-Zbuild-std=core,alloc" } else { "-Zbuild-std=std,panic_abort" })
        .env("RUSTFLAGS", rustflags.trim());
    if release {
        cmd.arg("--release");
    }
    if tiny {
        cmd.arg("-Zbuild-std-features=optimize_for_size");
    }
    if let Some(p) = &package {
        cmd.args(["-p", p]);
    }
    cmd.args(&extra);
    // rebuild the final crate whenever the backend changes (all codegen happens there)
    let fp = std::fs::metadata(&backend)
        .map(|m| {
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            format!("{:x}{:x}", m.len(), t)
        })
        .unwrap_or_default();
    if !extra.iter().any(|a| a == "--") {
        cmd.arg("--");
    }
    cmd.arg(format!("-Cmetadata=luau{fp}"));
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
    let src = std::fs::read_to_string(&artifact).map_err(|e| e.to_string())?;
    let src = if minify.unwrap_or(release) {
        luau_lir::minify::minify_with(&src, luau_lir::minify::Options { strip_local_types: tiny, alias_in_loops: true })
    } else {
        src
    };
    std::fs::write(&out, src).map_err(|e| format!("write {}: {e}", out.display()))?;
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
        Some("setup") => match setup() {
            Ok(_) => ExitCode::SUCCESS,
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
