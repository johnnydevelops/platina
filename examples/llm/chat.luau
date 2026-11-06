-- usage (from the repo root): lune run examples/llm/chat.luau <model.q8> <tok> <temperature> <message>...
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
local t = os.clock()
local mp, ml = put(fs.readFile(process.args[1]))
local tp, tl = put(fs.readFile(process.args[2]))
L.chat_init(mp, ml, tp, tl, 1024, 42)
L.chat_set_sampling(tonumber(process.args[3]) or 0.7, 40)
print(string.format("loaded in %.1fs", os.clock() - t))
for i = 4, #process.args do
	print("> " .. process.args[i])
	L.chat_user(put(process.args[i]))
	local prompt = L.chat_pending()
	local t0, n = os.clock(), 0
	local tPrompt = nil
	while true do
		local len = L.chat_step()
		if len < 0 then break end
		if not tPrompt and L.chat_pending() == 0 then
			tPrompt = os.clock() - t0
		end
		if len > 0 then
			stdio.write(buffer.readstring(L.memory(), L.chat_text(), len))
			n += 1
		end
	end
	local dt = os.clock() - t0
	print(string.format("\n[prompt: %d tokens in %.1fs (%.1f tok/s); reply: %d tokens in %.1fs (%.2f tok/s)]",
		prompt, tPrompt, prompt / tPrompt, n, dt - tPrompt, n / (dt - tPrompt)))
end
