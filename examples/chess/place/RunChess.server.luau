-- Pleco (Rust chess engine) compiled to Luau with rustc_codegen_luau.
local ReplicatedStorage = game:GetService("ReplicatedStorage")

local t0 = os.clock()
local Chess = require(ReplicatedStorage.Chess)
print(string.format("[chess] module loaded in %.3fs", os.clock() - t0))

for depth = 1, 5 do
	local t = os.clock()
	local n = Chess.perft(depth)
	print(string.format("[chess] perft(%d) = %d  (%.3fs)", depth, n, os.clock() - t))
end

local t = os.clock()
Chess.selfplay(10, 3) -- prints the moves and the final position
print(string.format("[chess] 10 moves of depth-3 self-play in %.3fs", os.clock() - t))
