--!native
--!optimize 2
-- u64 as (hi, lo) u32 pairs, written by hand
local bxor, lshift, rshift, bor = bit32.bxor, bit32.lshift, bit32.rshift, bit32.bor
local function bench(n: number): number
	local hi, lo = 0x2545F491, 0x4F6CDD1D
	local ahi, alo = 0, 0
	for _ = 1, n do
		-- x ^= x << 13
		local shi = bor(lshift(hi, 13), rshift(lo, 19))
		local slo = lshift(lo, 13)
		hi, lo = bxor(hi, shi), bxor(lo, slo)
		-- x ^= x >> 7
		slo = bor(rshift(lo, 7), lshift(hi, 25))
		shi = rshift(hi, 7)
		hi, lo = bxor(hi, shi), bxor(lo, slo)
		-- x ^= x << 17
		shi = bor(lshift(hi, 17), rshift(lo, 15))
		slo = lshift(lo, 17)
		hi, lo = bxor(hi, shi), bxor(lo, slo)
		-- acc += x
		alo += lo
		local c = 0
		if alo > 0xFFFFFFFF then alo -= 0x100000000; c = 1 end
		ahi = (ahi + hi + c) % 0x100000000
	end
	return bxor(alo, ahi)
end
return { bench = bench }
