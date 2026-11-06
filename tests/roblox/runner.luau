-- Run a Platina module that uses the `roblox` crate under Lune, with Roblox globals from
-- @lune/roblox. usage (from the repo root): lune run tests/roblox/runner.luau <module.luau> <export>
local luau = require("@lune/luau")
local fs = require("@lune/fs")
local process = require("@lune/process")
local roblox = require("@lune/roblox")
local task = require("@lune/task")

local game = roblox.Instance.new("DataModel")
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
local M = luau.load(fs.readFile(process.args[1]), { debugName = "module", environment = env, codegenEnabled = true })()
M[process.args[2] or "run"]()
