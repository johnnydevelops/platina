# LLM inference in Luau

Llama-architecture inference written in plain Rust and compiled to Luau with `cargo roblox`.
Two models are supported:

- **[SmolLM2-135M-Instruct](https://huggingface.co/HuggingFaceTB/SmolLM2-135M-Instruct)** (Apache-2.0), a small chat model:
  - weights quantized to int8 with one scale per 64 values (143 MB);
  - GPT-2 byte-level BPE tokenizer and the ChatML template;
  - about 6–7 tokens/s for replies and 12–15 tokens/s for reading the prompt under Lune.
- **[TinyStories](https://huggingface.co/karpathy/tinyllamas)** (MIT), story models in
  karpathy's llama2.c format: about 22 tokens/s for the 15M model under Lune.

Output is identical to the native build of the same code.

## Get the models

```sh
# SmolLM2 (needs Python with numpy and zstandard)
mkdir smol && cd smol
for f in config.json tokenizer.json model.safetensors; do
  curl -LO https://huggingface.co/HuggingFaceTB/SmolLM2-135M-Instruct/resolve/main/$f
done
cd .. && python3 tools/convert_smol.py smol smol.q8 smol.tok

# TinyStories
curl -LO https://huggingface.co/karpathy/tinyllamas/resolve/main/stories15M.bin
curl -LO https://raw.githubusercontent.com/karpathy/llama2.c/master/tokenizer.bin
```

## Run

```sh
cargo roblox build --release                     # -> out/llm.luau
# from the repository root, under Lune:
lune run examples/llm/chat.luau smol.q8 smol.tok 0.7 "What is the capital of France?" "Tell me a joke."
lune run examples/llm/run.luau stories15M.bin tokenizer.bin "Once upon a time" 0.8
# native reference
cargo run --release --bin chat-native -- smol.q8 smol.tok 0 "What is the capital of France?"
```

## Roblox place

```sh
python3 make_place.py smol.q8 smol.tok           # zstd chunks -> place/model (~175 MB)
rojo build default.project.json -o chat.rbxl
```

The model runs in a server Script, and the weights stay in `ServerStorage`. On startup the
server decodes the chunks with `EncodingService` (`Base64Decode`, then zstd `DecompressBuffer`).
Replies stream to a chat UI on the client. Open `chat.rbxl` in Studio, press Play, and type a
message.

## Performance notes

The hot loops are written so that the compiler emits straight-line Luau:
- Dot products are unrolled over fixed-size arrays (`as_chunks::<16>()`), so there are no
  bounds checks and little loop overhead.
- Generation computes four output rows per pass, so each input value is read once.
- The prompt is processed four tokens per pass, so each weight is read once for all four.
- Sums are accumulated in `f64`, so no per-operation `f32` rounding is needed.
