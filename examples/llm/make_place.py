"""Pack a converted chat model into the Rojo place (place/model, gitignored), then build:
    python3 make_place.py <model.q8> <model.tok> && rojo build default.project.json -o chat.rbxl

Each chunk is an independent zstd frame, base64-encoded into a ModuleScript; the server
decodes them with EncodingService (Base64Decode + DecompressBuffer)."""
import base64, json, os, sys
import zstandard

CHUNK = 4 * 1024 * 1024
here = os.path.dirname(os.path.abspath(__file__))
out = os.path.join(here, "place", "model")
os.makedirs(out, exist_ok=True)
for f in os.listdir(out):
    os.remove(os.path.join(out, f))
cctx = zstandard.ZstdCompressor(level=19)

def pack(path, prefix):
    raw = open(path, "rb").read()
    n = 0
    for i in range(0, len(raw), CHUNK):
        z = cctx.compress(raw[i:i + CHUNK])
        name = f"{prefix}{i // CHUNK + 1:03d}"
        open(os.path.join(out, name + ".luau"), "w").write(f'return "{base64.b64encode(z).decode()}"\n')
        n += 1
    return len(raw), n

msize, mparts = pack(sys.argv[1], "M")
tsize, tparts = pack(sys.argv[2], "T")
json.dump({"className": "Folder", "attributes": {"ModelSize": msize, "ModelParts": mparts, "TokSize": tsize,
           "TokParts": tparts, "ChunkSize": CHUNK}}, open(os.path.join(out, "init.meta.json"), "w"))
total = sum(os.path.getsize(os.path.join(out, f)) for f in os.listdir(out))
print(f"model {msize/1e6:.1f} MB in {mparts} parts, tokenizer {tsize/1e6:.2f} MB in {tparts}; {total/1e6:.1f} MB of modules")
