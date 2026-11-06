# rustc-codegen-luau

A rustc codegen backend that compiles Rust to **native Luau** for Roblox: real Luau locals,
structured `while`/`if`/`break`/`continue` control flow, multiple returns, and a `buffer`
only for memory that genuinely needs an address. No WebAssembly in between.

Status: early. `core` compiles through the backend and `core::fmt` works; `alloc` is next.

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
| `crates/luau-tests` | differential test runner (native rustc vs Luau under Lune) |
| `tests/run/*.rs` | differential tests |

## Testing

```sh
cargo run -p luau-tests            # all tests
cargo run -p luau-tests -- arith   # filter
cargo test -p luau_lir             # IR / structurer tests (needs the `luau` CLI)
```

Requires the pinned nightly (`rust-toolchain.toml`, with `rustc-dev`), and
[Lune](https://github.com/lune-org/lune) + Rojo via `aftman install`.
