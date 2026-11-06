# Split a local ROM into base64 ModuleScripts for the Rojo place (never commit the output).
import base64, json, os, sys
rom = open(sys.argv[1], 'rb').read()
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'place', 'rom')
os.makedirs(out, exist_ok=True)
for f in os.listdir(out):
    os.remove(os.path.join(out, f))
CHUNK = 768 * 1024  # multiple of 3 so chunks decode independently
for i in range(0, len(rom), CHUNK):
    b = base64.b64encode(rom[i:i + CHUNK]).decode()
    open(os.path.join(out, f'Part{i // CHUNK + 1:02d}.luau'), 'w').write(f'return "{b}"\n')
json.dump({"className": "Folder", "attributes": {"Size": len(rom)}}, open(os.path.join(out, 'init.meta.json'), 'w'))
print(f"{len(rom)} bytes -> {(len(rom) + CHUNK - 1) // CHUNK} modules in {out}")
