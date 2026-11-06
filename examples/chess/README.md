# Chess (Pleco)

[Pleco](https://github.com/pleco-rs/Pleco), a Rust chess engine (a Stockfish port), compiled
unmodified to Luau.

```sh
cargo roblox build --release      # -> out/chess.luau
rojo build default.project.json -o chess.rbxl
```

The place runs `perft(5)` (4,865,609 nodes, about 1.1 s in Studio) and a short self-play game,
and prints the moves and the final position.
