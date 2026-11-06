# N64 (RustyN64 → Luau)

[RustyN64](https://github.com/doublegate/RustyN64) (a pure-Rust, LLE N64 emulator, MIT/Apache)
compiled to Luau with `cargo roblox`. Runs Super Mario 64 pixel-identical to the native build.
`vendor/RustyN64` is a trimmed copy of the upstream core crates. No ROM is included.

```sh
# native reference (dumps PPMs every 30 frames)
cargo run --release --bin n64-native -- sm64.z64 300 /tmp/frames
# Luau build + headless run under Lune
cargo roblox build --release -o out/n64.luau
lune run examples/n64/run.luau sm64.z64 150 /tmp/frames 30   # from the repo root
# Roblox place (ROM split into base64 ModuleScripts under place/rom, gitignored)
python3 make_rom_modules.py sm64.z64 && rojo build default.project.json -o sm64.rbxl
```

Studio controls: WASD stick, Space A, LeftShift B, Q Z, Enter Start, E R, arrows C buttons.
