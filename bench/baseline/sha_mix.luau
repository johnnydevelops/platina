--!native
--!optimize 2
local rrotate, band, bxor, bnot = bit32.rrotate, bit32.band, bit32.bxor, bit32.bnot
local function bench(rounds: number): number
	local a, b, c, d, e, f, g, h = 0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19
	for r = 0, rounds - 1 do
		local s1 = bxor(rrotate(e, 6), rrotate(e, 11), rrotate(e, 25))
		local ch = bxor(band(e, f), band(bnot(e), g))
		local t1 = (h + s1 + ch + r + 0x428a2f98) % 0x100000000
		local s0 = bxor(rrotate(a, 2), rrotate(a, 13), rrotate(a, 22))
		local maj = bxor(band(a, b), band(a, c), band(b, c))
		local t2 = (s0 + maj) % 0x100000000
		a, b, c, d, e, f, g, h = (t1 + t2) % 0x100000000, a, b, c, (d + t1) % 0x100000000, e, f, g
	end
	return bxor(a, b, c, d, e, f, g, h)
end
return { bench = bench }
