-- usage (from the repo root): lune run examples/llm/run.luau <model.bin> <tokenizer.bin> [prompt] [temperature] [seed]
local luau = require("@lune/luau")
local fs = require("@lune/fs")
local process = require("@lune/process")
local stdio = require("@lune/stdio")

local L = luau.load(fs.readFile("examples/llm/out/llm.luau"), { debugName = "llm", codegenEnabled = true })()
local function put(s: string): (number, number)
	local p = L.llm_alloc(#s)
	buffer.writestring(L.memory(), p, s)
	return p, #s
end
local mp, ml = put(fs.readFile(process.args[1]))
local tp, tl = put(fs.readFile(process.args[2]))
L.llm_init(mp, ml, tp, tl, tonumber(process.args[5]) or 42)
L.llm_set_temperature(tonumber(process.args[4]) or 0)
L.llm_start(put(process.args[3] or ""))
local t0, n = os.clock(), 0
while true do
	local len = L.llm_step()
	if len < 0 then break end
	stdio.write(buffer.readstring(L.memory(), L.llm_text(), len))
	n += 1
end
local dt = os.clock() - t0
print(string.format("\n[%d tokens, %.1f tok/s]", n, n / dt))
