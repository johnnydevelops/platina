"""Convert a HF Llama-architecture chat model (e.g. SmolLM2-135M-Instruct) to the example's
Q8 format (int8 weights, one f32 scale per 64 values) plus a raw-byte BPE tokenizer file.

usage: python3 convert_smol.py <hf model dir> <out.q8> <out.tok>
"""
import json, struct, sys
import numpy as np

src, out_model, out_tok = sys.argv[1:4]
cfg = json.load(open(f"{src}/config.json"))
f = open(f"{src}/model.safetensors", "rb")
n = struct.unpack("<Q", f.read(8))[0]
hdr = json.loads(f.read(n))
data = f.read()

def tensor(name):
    t = hdr[name]
    a, b = t["data_offsets"]
    raw = np.frombuffer(data[a:b], dtype=np.uint16)
    assert t["dtype"] == "BF16"
    return (raw.astype(np.uint32) << 16).view(np.float32).reshape(t["shape"])

GS = 64
out = open(out_model, "wb")
def q8(w):
    w = w.astype(np.float32).reshape(-1, GS)
    scale = np.abs(w).max(axis=1) / 127.0
    scale[scale == 0] = 1.0
    q = np.clip(np.round(w / scale[:, None]), -127, 127).astype(np.int8)
    out.write(scale.astype("<f4").tobytes())
    out.write(q.tobytes())
def f32(w):
    out.write(w.astype("<f4").tobytes())

L = cfg["num_hidden_layers"]
out.write(b"SMQ8")
out.write(struct.pack("<7i", cfg["hidden_size"], cfg["intermediate_size"], L, cfg["num_attention_heads"],
                      cfg["num_key_value_heads"], cfg["vocab_size"], GS))
out.write(struct.pack("<2f", cfg["rope_theta"], cfg["rms_norm_eps"]))
assert cfg.get("tie_word_embeddings", False)
q8(tensor("model.embed_tokens.weight"))
for l in range(L):
    p = f"model.layers.{l}."
    f32(tensor(p + "input_layernorm.weight"))
    for m in ["q_proj", "k_proj", "v_proj", "o_proj"]:
        q8(tensor(p + f"self_attn.{m}.weight"))
    f32(tensor(p + "post_attention_layernorm.weight"))
    for m in ["gate_proj", "down_proj", "up_proj"]:
        q8(tensor(p + f"mlp.{m}.weight"))
f32(tensor("model.norm.weight"))
out.close()

# tokenizer: GPT-2 byte-level BPE -> raw bytes per token, merges as (left, right, result) ids
tok = json.load(open(f"{src}/tokenizer.json"))
bs = list(range(ord("!"), ord("~") + 1)) + list(range(ord("¡"), ord("¬") + 1)) + list(range(ord("®"), ord("ÿ") + 1))
cs = bs[:]
k = 0
for b in range(256):
    if b not in bs:
        bs.append(b); cs.append(256 + k); k += 1
u2b = {chr(c): b for b, c in zip(bs, cs)}
vocab = tok["model"]["vocab"]
V = cfg["vocab_size"]
pieces = [b""] * V
special = {a["id"]: a["content"].encode() for a in tok["added_tokens"]}
for s, i in vocab.items():
    pieces[i] = special[i] if i in special else bytes(u2b[ch] for ch in s)
merges = []
for m in tok["model"]["merges"]:
    a, b = m.split(" ") if isinstance(m, str) else m
    merges.append((vocab[a], vocab[b], vocab[a + b]))
byte_tok = [vocab.get(chr(cs[bs.index(b)]), -1) for b in range(256)]  # -1: byte not in vocab (dropped)
with open(out_tok, "wb") as t:
    t.write(struct.pack("<3i", V, len(merges), len(special)))
    t.write(struct.pack("<256i", *byte_tok))
    for p in pieces:
        t.write(struct.pack("<H", len(p)) + p)
    for a, b, r in merges:
        t.write(struct.pack("<3i", a, b, r))
import os
print(f"{out_model}: {os.path.getsize(out_model)/1e6:.1f} MB, {out_tok}: {os.path.getsize(out_tok)/1e6:.2f} MB")
