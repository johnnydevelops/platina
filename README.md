# Platina

Platina compiles Rust to **native Luau** for Roblox. It consists of a rustc codegen backend
(`rustc_codegen_luau`) and a cargo subcommand (`cargo roblox`).

It is not a WebAssembly translator. The backend lowers rustc's MIR straight to Luau:
- values live in real Luau locals;
- control flow comes out as structured `while`/`for`/`if`/`break`/`continue`;
- functions return multiple values;
- a single `buffer` holds only the memory that genuinely needs an address.

The output is written for Luau's native code generator (`--!native`).

`core`, `alloc` and `std` all work, and so do unmodified crates.io crates. Some programs
that run through it:

| example | what it is |
| --- | --- |
| [`examples/showcase`](examples/showcase) | a small Roblox experience written entirely in Rust: parts, physics, events, GUI |
| [`examples/llm`](examples/llm) | **SmolLM2-135M-Instruct** chatting in Roblox: a Llama implementation in Rust with int8 weights and a BPE tokenizer, about 6–7 tokens/s under Lune; the chat UI, RemoteEvents and model loading are Rust too |
| [`examples/n64`](examples/n64) | **[RustyN64](https://github.com/doublegate/RustyN64)**, a Nintendo 64 emulator, booting Super Mario 64 pixel-for-pixel identical to the native build; the in-game frontend (UI, input, `EditableImage` rendering) is Rust |
| `tests/crates` | serde_json, regex, itertools, rand + rand_chacha, sha2, glam |

## Quick start

You need [rustup](https://rustup.rs). Install the `cargo roblox` subcommand straight from GitHub:

```sh
cargo install --git https://github.com/johnnydevelops/platina cargo-roblox
```

The first `cargo roblox build` installs the pinned nightly toolchain (with `rustc-dev`) through
rustup and builds the backend into `~/.cargo/lib`. You can also do that ahead of time with
`cargo roblox setup`.

Next, export functions from a library crate. Every `#[unsafe(no_mangle)] pub extern "C" fn`
becomes a field of the returned ModuleScript table:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn fib(n: u32) -> f64 {
    let (mut a, mut b) = (0u64, 1u64);
    for _ in 0..n {
        (a, b) = (b, a.wrapping_add(b));
    }
    a as f64
}
```

```sh
cargo roblox build --release                     # -> out/<crate>.luau
cargo roblox build --release -o src/shared/Rust.luau
```

```lua
local Rust = require(game.ReplicatedStorage.Rust)
print(Rust.fib(70)) --> 190392490709135
```

No `Cargo.toml` changes are needed. Any crate with a library target works, and the subcommand
builds it as a `cdylib` for the `luau32-unknown-roblox` target.

| option | |
| --- | --- |
| `--release` | optimize (recommended; release builds are also minified) |
| `-o, --out <file>` | output path (default `out/<crate>.luau`) |
| `-p, --package <name>` | package to build in a workspace |
| `--no-std` | build only `core` + `alloc`, for `#![no_std]` crates |
| `--no-minify` | readable output |
| `--tiny` | also strip panic messages (`panic = immediate-abort`) |

### The Roblox API

The [`roblox`](crates/roblox) crate binds the whole scriptable Roblox API. It's generated from
Roblox's API dump: every class, property, method, event and enum, with Rust datatypes and
Rust closures as event handlers.

```rust
use roblox::prelude::*;

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let part = Part::new();
    part.set_color(Color3::from_rgb(255, 80, 0));
    part.set_parent(Some(&Workspace::get()));
    part.touched().connect(|other: Option<BasePart>| println!("touched: {:?}", other.map(|p| p.name())));
}
```

The examples are written this way. The only Luau in each place is a one-line script such as
`require(game.ServerScriptService.Showcase).main()`.

### Passing data

Exported functions take and return numbers (all integer types up to 32 bits, `f32`, `f64` and
`bool`). Larger data goes through the module's linear memory, which `memory()` returns:

```lua
local ptr = Rust.alloc(#s)                     -- an export of yours, e.g. returning Vec::leak
buffer.writestring(Rust.memory(), ptr, s)      -- call memory() again after any allocation:
Rust.process(ptr, #s)                          -- memory grows by replacing the buffer
```

Any crate can also call Luau directly. Declare an `extern "C"` function whose link name is the
Luau code, and Platina emits that code wherever the function is called:

```rust
unsafe extern "C" {
    #[link_name = "luau::function(x) return math.noise(x) end"]
    fn noise(x: f64) -> f64;
}
```

## Performance

Benchmarks against idiomatic handwritten Luau, both measured under Lune with native codegen
(lower is better):

| benchmark | Rust → Luau vs handwritten Luau |
| --- | --- |
| nbody (f64) | **0.67×** |
| xorshift64 (u64) | **0.85×** |
| sha_mix (u32 bit mixing) | 1.02× |
| sort | 1.9× (the baseline uses `table.sort`, which is written in C) |
| sieve | 2.0× (Rust's bounds and overflow checks) |

`cargo run -p luau-tests -- --bench` reproduces these numbers. Selected optimizations:
- whole-program inlining;
- scalar replacement of structs and enums, and promotion of stack memory to locals;
- range analysis, which removes overflow wrapping, bounds checks and `Option` null tests;
- store-to-load forwarding;
- numeric `for` loops for counted ranges;
- 64-bit integers as pairs of 32-bit halves, with inline arithmetic;
- function-pointer tables, and vtables pruned to the methods that are actually called.

### Output size

Release builds are minified:
- names are shortened, most frequent first;
- builtins such as `buffer.readu32` are aliased to locals, which native codegen still fast-calls;
- repeated strings are stored once;
- static data is packed as base64;
- the runtime prelude is tree-shaken.

Only code reachable from your exports is emitted. On top of that:
- `println!` formats straight to the runtime;
- vtable methods that are never called are dropped;
- identical functions and match arms are merged.

| crate | readable | minified (default) | `--tiny` |
| --- | --- | --- | --- |
| hello (`HashMap` + `println!`) | 212 KB | 108 KB | 71 KB |
| serde_json | | 454 KB | |
| regex | | 2.2 MB | |

## std on Roblox

To `std`, the target looks like `wasm32-unknown-unknown`. The backend supplies the platform layer:

| std | Luau |
| --- | --- |
| global allocator | size-class heap in the module's `buffer` (grows on demand) |
| `println!` / `eprintln!` | `print` / `warn` (line buffered) |
| `Instant::now` / `SystemTime::now` | `os.clock` / `DateTime.now()` |
| `panic!` | `error("panicked at file:line:col:\nmessage")`, catchable with `pcall` |
| `HashMap` seeds, `Mutex`, `RwLock`, `thread_local!`, `OnceLock` | single-threaded implementations |
| files, network, threads, processes | unsupported (they return errors) |

Use `cfg(target_vendor = "roblox")` to detect the target. The optional
[`luau_rt`](crates/luau_rt) crate adds a `getrandom` backend for `rand` and helpers for
`#![no_std]` programs.

## Limitations

- **Not every API type maps to a Rust type.** Members whose types have no Rust mapping take
  and return a dynamic `Value`.
- **Single-threaded.** Rust threads assume shared memory, and Roblox Actors don't share it.
- **Pinned nightly toolchain.** Codegen backends rely on unstable compiler internals.
- **32-bit pointers** (`usize` is 32 bits).
- **64- and 128-bit integers are emulated** with 32-bit halves. They're correct, but slower
  than 32-bit integers.
- **Panics abort the call.** Unwinding isn't supported; a panic surfaces as a Luau error.

## How it works

1. `cargo roblox` runs rustc with `-Zcodegen-backend=librustc_codegen_luau` on the
   `luau32-unknown-roblox` target ([`targets/`](targets)), using `-Zbuild-std`.
2. Library crates carry only metadata and MIR. When the final `cdylib` is compiled, the backend
   walks MIR from every export and monomorphizes the whole program itself.
3. MIR is lowered to **LIR** ([`crates/luau_lir`](crates/luau_lir)). Values are split into
   scalar "leaves" that become Luau locals. Only values whose address is taken, or that can't be
   split, live in linear memory (`MEM: buffer`, with a shadow stack).
4. LIR passes optimize the whole program, in particular inlining, the memory passes and range
   analysis.
5. The control-flow graph is structured into Luau loops and conditionals (Ramsey's "Beyond
   Relooper"), then emitted with registers allocated within Luau's 200-local limit.

| path | |
| --- | --- |
| `crates/rustc_codegen_luau` | the rustc backend (MIR → LIR) and the std platform layer |
| `crates/luau_lir` | IR, optimization passes, structurer, emitter, minifier, runtime prelude |
| `crates/cargo-roblox` | the `cargo roblox` subcommand |
| `crates/roblox` | the Roblox API (generated bindings, datatypes, signals, tasks) |
| `crates/luau_rt` | optional runtime helpers for programs |
| `crates/luau-tests` | differential test runner (native rustc vs Luau) and benchmarks |
| `tests/run`, `tests/std`, `tests/crates` | `no_std`, `std` and crates.io differential tests |
| `tests/roblox` | Roblox bindings test, run under Lune's `@lune/roblox` |
| `bench/` | benchmarks with handwritten Luau baselines |

## Development

```sh
git clone https://github.com/johnnydevelops/platina && cd platina
aftman install                         # Lune + Rojo (or install them yourself)
cargo build -p rustc_codegen_luau -p cargo-roblox
cargo run -p luau-tests                # every test, run natively and in Luau, outputs compared
cargo run -p luau-tests -- --bench     # benchmarks
cargo run -p luau-tests -- regex       # filter by name
```

Inside the repository, `target/debug/cargo-roblox` uses the freshly built backend next to it.
Debugging aids:
- `LUAU_DUMP_LIR=<fn name>` prints a function's final LIR;
- `LUAU_DUMP_MIR=<fn name>` prints its MIR;
- `LUAU_NO_INLINE=1` turns off inlining;
- `LUAU_DISABLE=range,mem,...` turns off individual passes.

## License

Platina is licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.

**The Luau that Platina generates is yours.** Under the [output exception](LICENSE-EXCEPTION),
compiled modules, including the runtime Platina embeds in them, can be used in any game,
commercial or not. You don't need attribution, and you don't need to ship license text. The
exception doesn't change the licenses of third-party code you compile in (for example crates
you depend on), which keep their own terms.

The examples' own code is covered by the same licenses. They also use third-party projects
under their own licenses:
- [RustyN64](examples/n64/vendor/RustyN64): MIT OR Apache-2.0, vendored.
- SmolLM2 (Apache-2.0) and TinyStories (MIT) model weights: the examples download them; they're
  not included here.
