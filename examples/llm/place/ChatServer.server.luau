--!optimize 2
-- SmolLM2-135M-Instruct (Q8) running in Luau: llama inference written in Rust, compiled by
-- rustc_codegen_luau. The model runs here on the server; replies stream to clients.
local EncodingService = game:GetService("EncodingService")
local ReplicatedStorage = game:GetService("ReplicatedStorage")
local ServerStorage = game:GetService("ServerStorage")

local remotes = Instance.new("Folder")
remotes.Name = "ChatRemotes"
local ask = Instance.new("RemoteEvent")
ask.Name = "Ask"
ask.Parent = remotes
local token = Instance.new("RemoteEvent") -- (kind, text): "text" | "status" | "done" | "user"
token.Name = "Token"
token.Parent = remotes
remotes.Parent = ReplicatedStorage

local function status(s: string)
	print("[chat] " .. s)
	token:FireAllClients("status", s)
end

task.wait(10) -- let the first-load hitch settle

local L = require(ServerStorage:WaitForChild("LLM"))
local folder = ServerStorage:WaitForChild("Model")

-- decode `parts` chunk modules named prefix001.. into a fresh 4-aligned allocation
local function load(prefix: string, size: number, parts: number, what: string): number
	local ptr = L.llm_alloc(size)
	local at = ptr
	local t0 = os.clock()
	for i = 1, parts do
		local mod = folder:WaitForChild(string.format("%s%03d", prefix, i))
		local copy = mod:Clone() -- required once, then dropped: no cached 4 MB string
		local b64: string = require(copy)
		copy:Destroy()
		local raw = EncodingService:DecompressBuffer(
			EncodingService:Base64Decode(buffer.fromstring(b64)),
			Enum.CompressionAlgorithm.Zstd
		)
		b64 = nil :: any
		local n = buffer.len(raw)
		buffer.copy(L.memory(), at, raw, 0, n) -- fetch memory() after allocating (it may grow)
		at += n
		status(string.format("loading %s %d/%d (%.0fs)", what, i, parts, os.clock() - t0))
		task.wait()
	end
	assert(at - ptr == size, "size mismatch while loading " .. what)
	return ptr
end

local msize, tsize = folder:GetAttribute("ModelSize"), folder:GetAttribute("TokSize")
local mp = load("M", msize, folder:GetAttribute("ModelParts"), "model")
local tp = load("T", tsize, folder:GetAttribute("TokParts"), "tokenizer")
L.chat_init(mp, msize, tp, tsize, 1024, os.time())
L.chat_set_sampling(0.7, 40)
status("ready: ask me something")

local queue: { { player: Player, text: string } } = {}
ask.OnServerEvent:Connect(function(player, text)
	if typeof(text) == "string" and #text > 0 and #text <= 500 then
		table.insert(queue, { player = player, text = text })
	end
end)

while true do
	local item = table.remove(queue, 1)
	if not item then
		task.wait(0.1)
		continue
	end
	token:FireAllClients("user", item.player.DisplayName .. ": " .. item.text)
	local p = L.llm_alloc(#item.text)
	buffer.writestring(L.memory(), p, item.text)
	L.chat_user(p, #item.text)
	local total = L.chat_pending()
	local t0, n = os.clock(), 0
	while true do
		local len = L.chat_step()
		if len == -2 then
			L.chat_reset()
			status("context full: conversation reset")
			break
		elseif len < 0 then
			break
		end
		local pending = L.chat_pending()
		if pending > 0 then
			status(string.format("reading prompt %d/%d", total - pending, total))
		elseif len > 0 then
			n += 1
			token:FireAllClients("text", buffer.readstring(L.memory(), L.chat_text(), len))
		end
		task.wait()
	end
	token:FireAllClients("done", "")
	status(string.format("%d tokens in %.0fs (%.2f tok/s), context %d/1024", n, os.clock() - t0, n / (os.clock() - t0), L.chat_pos()))
end
