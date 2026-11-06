# Showcase

A small Roblox experience written entirely in Rust with the [`roblox`](../../crates/roblox)
crate. The place's only Luau is `require(ServerScriptService.Showcase).main()`.

- a rainbow tower of neon blocks;
- a platform spun every frame from `RunService.Heartbeat`;
- touch pads that count character touches and change color;
- a GUI for every player, with a live counter and a button that drops physics balls (cleaned
  up by `Debris`).

```sh
cargo roblox build --release           # -> out/showcase.luau
rojo build default.project.json -o showcase.rbxl
```

Open `showcase.rbxl` in Studio and press Play.
