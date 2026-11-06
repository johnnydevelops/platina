-- Loads a compiled module with native codegen and calls its `main` export.
local luau = require("@lune/luau")
local fs = require("@lune/fs")
local process = require("@lune/process")

local path = process.args[1]
local fnName = process.args[2] or "main"
local chunk = luau.load(fs.readFile(path), { debugName = path, codegenEnabled = true })
local M = chunk()
local ok, err = pcall(M[fnName])
if not ok then
	print("PANIC: " .. tostring(err))
	process.exit(101)
end
