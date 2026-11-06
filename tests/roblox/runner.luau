-- Run a Platina module that uses the `roblox` crate under Lune, with Roblox globals from
-- @lune/roblox. usage (from the repo root): lune run tests/roblox/runner.luau <module.luau> <export>
local luau = require("@lune/luau")
local fs = require("@lune/fs")
local process = require("@lune/process")
local roblox = require("@lune/roblox")
local task = require("@lune/task")

local game = roblox.Instance.new("DataModel")

-- a minimal signal (Lune has no engine events)
local function newSignal()
	local s = { handlers = {} }
	function s:Connect(f)
		local c = { Connected = true }
		self.handlers[c] = f
		function c.Disconnect()
			c.Connected = false
			self.handlers[c] = nil
		end
		return c
	end
	function s:Fire(...)
		for _, f in self.handlers do
			local ok, err = pcall(f, ...)
			if not ok then
				print("handler error: " .. tostring(err))
			end
		end
	end
	return s
end
local env = {
	Instance = roblox.Instance,
	Vector3 = roblox.Vector3,
	Vector2 = roblox.Vector2,
	CFrame = roblox.CFrame,
	Color3 = roblox.Color3,
	UDim = roblox.UDim,
	UDim2 = roblox.UDim2,
	Rect = roblox.Rect,
	NumberRange = roblox.NumberRange,
	BrickColor = roblox.BrickColor,
	Enum = roblox.Enum,
	game = game,
	workspace = game:GetService("Workspace"),
	task = task,
}
-- user globals are reached through _G (Value::global never uses getfenv)
_G.TestSignal = newSignal()
local M = luau.load(fs.readFile(process.args[1]), { debugName = "module", environment = env, codegenEnabled = true })()
M[process.args[2] or "run"]()
if process.args[3] == "stress" then
	-- fire the test signal between the yields of running Rust tasks
	local part = roblox.Instance.new("Part")
	for i = 1, 200 do
		_G.TestSignal:Fire(i, part)
		task.wait(0.002)
	end
	task.wait(0.1)
	M[process.args[4]]()
end
