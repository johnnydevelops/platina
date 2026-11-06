# rustc-codegen-luau

A rustc codegen backend that compiles Rust to **native Luau** for Roblox: real Luau locals,
structured `while`/`if`/`break`/`continue` control flow, multiple returns, and a `buffer`
only for memory that genuinely needs an address. No WebAssembly in between.

Status: early but usable for compute code. `core`, `alloc` and `std` work, and real crates
(serde/serde_json, regex, itertools, rand, sha2, glam) pass differential tests.

## Usage

```sh
# once: build the backend and the cargo subcommand
cargo build --release -p rustc_codegen_luau
cargo install --path crates/cargo-roblox

# in any library crate (no Cargo.toml changes needed)
cargo roblox build --release            # -> out/<crate>.luau
cargo roblox build --release -o src/server/Rust.luau
```

### Output size

Release builds are minified: declared names are shortened (frequency-ranked), builtins like
`buffer.readu32` are aliased to short locals (still fast-called by Luau's native codegen),
whitespace/comments are stripped, repeated strings are stored once, static data is
base64-packed and source paths in panic messages are shortened (`/rust/...`, `/crates/...`).
Use `--no-minify` for readable output, or `--tiny` to also drop panic-message formatting.

| crate | readable | minified (default) | `--tiny` |
| --- | --- | --- | --- |
| hello (HashMap + println) | 264 KB | 135 KB | 89 KB |
| serde_json | | 475 KB | |
| regex | | 2.3 MB | 2.0 MB |

Beyond formatting, the backend avoids emitting code it doesn't need: the runtime prelude is
tree-shaken, branches into `unreachable` are pruned (UB in Rust), identical match arms and
identical functions are merged.

Every `#[unsafe(no_mangle)] pub extern "C" fn` becomes a field of the returned module table:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn fib(n: u32) -> f64 { /* ... */ }
```

```lua
local Rust = require(script.Rust)
print(Rust.fib(80))
```

### std on Roblox

The target looks like `wasm32-unknown-unknown` to `std`; the backend supplies the platform layer:

| std | Luau |
| --- | --- |
| global allocator | size-class heap in the runtime `buffer` (grows on demand) |
| `println!` / `eprintln!` | `print` / `warn` (line buffered) |
| `Instant::now` / `SystemTime::now` | `os.clock` / `DateTime.now()` |
| `panic!` | `error("panicked at file:line:col:\nmessage")` — callers can `pcall` |
| `HashMap` seeds, `Mutex`, `RwLock`, `thread_local!`, `OnceLock` | single-threaded implementations |
| files, network, threads, processes | unsupported (return errors) |

Use `cfg(target_vendor = "roblox")` to detect the target.

## How it works

1. `rustc` runs with `-Zcodegen-backend=librustc_codegen_luau.so` on the
   `luau32-unknown-roblox` target (`targets/luau32-unknown-roblox.json`) with
   `-Zbuild-std=core,alloc` and `-Zalways-encode-mir`.
2. Library crates only carry metadata + MIR. When the final `cdylib` is compiled, the backend
   walks MIR from every `#[no_mangle]` export and monomorphizes the whole program itself.
3. MIR is lowered to **LIR** (`crates/luau_lir`): values are split into scalar "leaves" that
   become Luau locals; only address-taken or non-splittable values live in linear memory
   (`MEM: buffer`, with a shadow stack).
4. The CFG is structured with Ramsey's dominator-tree algorithm and emitted as Luau with
   `--!native` and type annotations.

## Layout

| Path | What |
| --- | --- |
| `crates/rustc_codegen_luau` | the rustc backend (MIR → LIR) |
| `crates/luau_lir` | IR, structurer, Luau emitter, prelude |
| `crates/luau_rt` | runtime crate for programs (`println!`, panic handler) |
| `crates/cargo-roblox` | the `cargo roblox` subcommand |
| `crates/luau-tests` | differential test runner (native rustc vs Luau under Lune) and benchmarks |
| `tests/run/*.rs` | `no_std` + `alloc` differential tests |
| `tests/std/*.rs` | `std` differential tests |
| `tests/crates/*/` | crates.io compatibility tests (built with `cargo roblox`) |
| `bench/` | benchmarks with handwritten Luau baselines |

## Testing

```sh
cargo run -p luau-tests            # all tests (no_std, std, crates)
cargo run -p luau-tests -- --bench # benchmarks vs handwritten Luau
cargo run -p luau-tests -- arith   # filter
cargo test -p luau_lir             # IR / structurer tests (needs the `luau` CLI)
```

Requires the pinned nightly (`rust-toolchain.toml`, with `rustc-dev`), and
[Lune](https://github.com/lune-org/lune) + Rojo via `aftman install`.
