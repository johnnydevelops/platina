-- usage: lune run bench/runner.luau <module.luau> <n> [baseline.luau]
local luau = require("@lune/luau")
local fs = require("@lune/fs")
local process = require("@lune/process")

local function load(path)
	return luau.load(fs.readFile(path), { debugName = path, codegenEnabled = true })()
end

local function time(f, n)
	local best = math.huge
	local r
	for _ = 1, 5 do
		local t = os.clock()
		r = f(n)
		best = math.min(best, os.clock() - t)
	end
	return r, best
end

local n = tonumber(process.args[2])
local M = load(process.args[1])
local r, t = time(M.bench, n)
local line = string.format("result=%d rust=%.4fs", r, t)
if process.args[3] then
	local B = load(process.args[3])
	local br, bt = time(B.bench, n)
	line ..= string.format(" baseline=%.4fs (result %d) ratio=%.2fx", bt, br, t / bt)
end
print(line)
