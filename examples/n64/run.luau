-- Runs SM64 on the RustyN64 core compiled to Luau, dumping frames as PPM images.
-- usage: lune run examples/n64/run.luau <rom.z64> <frames> <outdir> [every]
local luau = require("@lune/luau")
local fs = require("@lune/fs")
local process = require("@lune/process")

local romPath, frames, outDir = process.args[1], tonumber(process.args[2]) or 60, process.args[3] or "."
local every = tonumber(process.args[4]) or 20
local src = fs.readFile("examples/n64/out/n64.luau")
local t0 = os.clock()
local N = luau.load(src, { debugName = "n64", codegenEnabled = true })()
print(string.format("module loaded in %.2fs", os.clock() - t0))

local rom = fs.readFile(romPath)
local ptr = N.n64_alloc(#rom)
buffer.writestring(N.memory(), ptr, rom) -- fetch MEM after allocating (it may have grown)
assert(N.n64_load(ptr, #rom) == 0, "boot failed")
print("booted", #rom, "bytes")

local function dump(f)
	local w, h = N.n64_w(), N.n64_h()
	if w == 0 then return end
	local mem, fb = N.memory(), N.n64_fb()
	local parts = table.create(h + 1)
	parts[1] = string.format("P6\n%d %d\n255\n", w, h)
	local row = buffer.create(w * 3)
	for y = 0, h - 1 do
		local base = fb + y * w * 4
		for x = 0, w - 1 do
			buffer.writeu8(row, x * 3, buffer.readu8(mem, base + x * 4))
			buffer.writeu8(row, x * 3 + 1, buffer.readu8(mem, base + x * 4 + 1))
			buffer.writeu8(row, x * 3 + 2, buffer.readu8(mem, base + x * 4 + 2))
		end
		parts[y + 2] = buffer.tostring(row)
	end
	fs.writeFile(string.format("%s/frame_%04d.ppm", outDir, f), table.concat(parts))
end

local start = os.clock()
for f = 1, frames do
	local t = os.clock()
	N.n64_run_frame()
	local dt = os.clock() - t
	if f % every == 0 then
		print(string.format("frame %d: %dx%d  %.2fs this frame, %.1fs total, %.0fM instructions", f, N.n64_w(), N.n64_h(), dt, os.clock() - start, N.n64_retired() / 1e6))
		dump(f)
	end
end
