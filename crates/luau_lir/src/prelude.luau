local SCR: buffer = buffer.create(8)
local function f32(x: number): number
	buffer.writef32(SCR, 0, x)
	return buffer.readf32(SCR, 0)
end
local function imul32(a: number, b: number): number
	a = a % 0x100000000
	b = b % 0x100000000
	local al = a % 65536
	local bl = b % 65536
	return (al * bl + ((((a - al) / 65536) * bl + al * ((b - bl) / 65536)) % 65536) * 65536) % 0x100000000
end
local function trunc(x: number): number
	if x >= 0 then return math.floor(x) else return math.ceil(x) end
end
local function idivt(a: number, b: number): number
	local q = a / b
	if q >= 0 then return math.floor(q) else return math.ceil(q) end
end
local function ftoi(x: number, lo: number, hi: number): number
	if x ~= x then return 0 end
	if x >= 0 then x = math.floor(x) else x = math.ceil(x) end
	if x < lo then return lo elseif x > hi then return hi else return x end
end
local function popcnt(x: number): number
	x = x - bit32.band(bit32.rshift(x, 1), 0x55555555)
	x = bit32.band(x, 0x33333333) + bit32.band(bit32.rshift(x, 2), 0x33333333)
	x = bit32.band(x + bit32.rshift(x, 4), 0x0F0F0F0F)
	return bit32.rshift(imul32(x, 0x01010101), 24)
end
local function bitrev32(x: number): number
	local r = 0
	for _ = 1, 32 do
		r = r * 2 + bit32.band(x, 1)
		x = bit32.rshift(x, 1)
	end
	return r
end
local function roundaway(x: number): number
	-- round half away from zero, exact for all doubles
	local t = trunc(x)
	if math.abs(x - t) >= 0.5 then
		if x > 0 then return t + 1 else return t - 1 end
	end
	return t
end
local function fmin(a: number, b: number): number
	if a ~= a then return b elseif b ~= b then return a elseif a < b then return a else return b end
end
local function fmax(a: number, b: number): number
	if a ~= a then return b elseif b ~= b then return a elseif a > b then return a else return b end
end
local function copysign(a: number, b: number): number
	local neg = b < 0 or (b == 0 and 1 / b < 0)
	a = math.abs(a)
	if neg then return -a else return a end
end
local function memcmp(a: number, b: number, n: number): number
	for i = 0, n - 1 do
		local x = buffer.readu8(MEM, a + i)
		local y = buffer.readu8(MEM, b + i)
		if x ~= y then
			if x < y then return -1 else return 1 end
		end
	end
	return 0
end

-- 64-bit integers as (lo, hi) pairs of u32
local function add64(al: number, ah: number, bl: number, bh: number): (number, number)
	local lo = al + bl
	local c = 0
	if lo >= 0x100000000 then lo -= 0x100000000; c = 1 end
	return lo, (ah + bh + c) % 0x100000000
end
local function sub64(al: number, ah: number, bl: number, bh: number): (number, number)
	local lo = al - bl
	local b = 0
	if lo < 0 then lo += 0x100000000; b = 1 end
	return lo, (ah - bh - b) % 0x100000000
end
local function neg64(lo: number, hi: number): (number, number)
	return sub64(0, 0, lo, hi)
end
-- full 64-bit product of two u32 values
local function mulu32(a: number, b: number): (number, number)
	local a0 = a % 65536
	local a1 = (a - a0) / 65536
	local b0 = b % 65536
	local b1 = (b - b0) / 65536
	local mid = a0 * b1 + a1 * b0
	local midlo = mid % 65536
	local lofull = a0 * b0 + midlo * 65536
	local lo = lofull % 0x100000000
	local carry = (lofull - lo) / 0x100000000
	local hi = a1 * b1 + (mid - midlo) / 65536 + carry
	return lo, hi
end
local function mul64(al: number, ah: number, bl: number, bh: number): (number, number)
	local lo, hi = mulu32(al, bl)
	hi = (hi + imul32(al, bh) + imul32(ah, bl)) % 0x100000000
	return lo, hi
end
local function shl64(lo: number, hi: number, n: number): (number, number)
	if n == 0 then return lo, hi end
	if n >= 32 then return 0, bit32.lshift(lo, n - 32) end
	return bit32.lshift(lo, n), bit32.bor(bit32.lshift(hi, n), bit32.rshift(lo, 32 - n))
end
local function shr64(lo: number, hi: number, n: number): (number, number)
	if n == 0 then return lo, hi end
	if n >= 32 then return bit32.rshift(hi, n - 32), 0 end
	return bit32.bor(bit32.rshift(lo, n), bit32.lshift(hi, 32 - n)), bit32.rshift(hi, n)
end
local function sar64(lo: number, hi: number, n: number): (number, number)
	if n == 0 then return lo, hi end
	if n >= 32 then return bit32.arshift(hi, n - 32), bit32.arshift(hi, 31) end
	return bit32.bor(bit32.rshift(lo, n), bit32.lshift(hi, 32 - n)), bit32.arshift(hi, n)
end
local function rotl64(lo: number, hi: number, n: number): (number, number)
	n = n % 64
	local al, ah = shl64(lo, hi, n)
	local bl, bh = shr64(lo, hi, (64 - n) % 64)
	if n == 0 then return lo, hi end
	return bit32.bor(al, bl), bit32.bor(ah, bh)
end
local function rotr64(lo: number, hi: number, n: number): (number, number)
	return rotl64(lo, hi, (64 - n % 64) % 64)
end
local function lt64u(al: number, ah: number, bl: number, bh: number): boolean
	return ah < bh or (ah == bh and al < bl)
end
local function udivmod64(al: number, ah: number, bl: number, bh: number): (number, number, number, number)
	if ah == 0 and bh == 0 then
		local q = al // bl
		return q, 0, al - q * bl, 0
	end
	if ah < 0x100000 and bh < 0x100000 then
		-- both fit in 52 bits: exact double arithmetic
		local a = ah * 0x100000000 + al
		local b = bh * 0x100000000 + bl
		local q = a // b
		local r = a - q * b
		if r < 0 then q -= 1; r += b elseif r >= b then q += 1; r -= b end
		local ql = q % 0x100000000
		local rl = r % 0x100000000
		return ql, (q - ql) / 0x100000000, rl, (r - rl) / 0x100000000
	end
	-- binary long division
	local ql, qh, rl, rh = 0, 0, 0, 0
	for i = 63, 0, -1 do
		rl, rh = shl64(rl, rh, 1)
		local bit = if i >= 32 then bit32.extract(ah, i - 32) else bit32.extract(al, i)
		rl = bit32.bor(rl, bit)
		if not lt64u(rl, rh, bl, bh) then
			rl, rh = sub64(rl, rh, bl, bh)
			if i >= 32 then qh = bit32.replace(qh, 1, i - 32) else ql = bit32.replace(ql, 1, i) end
		end
	end
	return ql, qh, rl, rh
end
local function udiv64(al: number, ah: number, bl: number, bh: number): (number, number)
	local ql, qh = udivmod64(al, ah, bl, bh)
	return ql, qh
end
local function urem64(al: number, ah: number, bl: number, bh: number): (number, number)
	local _, _, rl, rh = udivmod64(al, ah, bl, bh)
	return rl, rh
end
local function idiv64(al: number, ah: number, bl: number, bh: number): (number, number)
	local na = ah >= 0x80000000
	local nb = bh >= 0x80000000
	if na then al, ah = neg64(al, ah) end
	if nb then bl, bh = neg64(bl, bh) end
	local ql, qh = udivmod64(al, ah, bl, bh)
	if na ~= nb then return neg64(ql, qh) end
	return ql, qh
end
local function irem64(al: number, ah: number, bl: number, bh: number): (number, number)
	local na = ah >= 0x80000000
	if na then al, ah = neg64(al, ah) end
	if bh >= 0x80000000 then bl, bh = neg64(bl, bh) end
	local _, _, rl, rh = udivmod64(al, ah, bl, bh)
	if na then return neg64(rl, rh) end
	return rl, rh
end
local function addo64(al: number, ah: number, bl: number, bh: number, signed: boolean): (number, number, number)
	local lo, hi = add64(al, ah, bl, bh)
	local o
	if signed then
		local sa, sb, sr = ah >= 0x80000000, bh >= 0x80000000, hi >= 0x80000000
		o = sa == sb and sr ~= sa
	else
		o = lt64u(lo, hi, al, ah)
	end
	return lo, hi, if o then 1 else 0
end
local function subo64(al: number, ah: number, bl: number, bh: number, signed: boolean): (number, number, number)
	local lo, hi = sub64(al, ah, bl, bh)
	local o
	if signed then
		local sa, sb, sr = ah >= 0x80000000, bh >= 0x80000000, hi >= 0x80000000
		o = sa ~= sb and sr ~= sa
	else
		o = lt64u(al, ah, bl, bh)
	end
	return lo, hi, if o then 1 else 0
end
local function umulhi_ov(al: number, ah: number, bl: number, bh: number): boolean
	if ah ~= 0 and bh ~= 0 then return true end
	local _, h = mulu32(al, bl)
	local t = h + al * bh + ah * bl
	return t >= 0x100000000
end
local function mulo64(al: number, ah: number, bl: number, bh: number, signed: boolean): (number, number, number)
	local lo, hi = mul64(al, ah, bl, bh)
	local o
	if signed then
		local na, nb = ah >= 0x80000000, bh >= 0x80000000
		local xl, xh, yl, yh = al, ah, bl, bh
		if na then xl, xh = neg64(xl, xh) end
		if nb then yl, yh = neg64(yl, yh) end
		o = umulhi_ov(xl, xh, yl, yh)
		if not o then
			local _, mh = mul64(xl, xh, yl, yh)
			if na ~= nb then
				local ml, _ = mul64(xl, xh, yl, yh)
				o = mh > 0x80000000 or (mh == 0x80000000 and ml ~= 0)
			else
				o = mh >= 0x80000000
			end
		end
	else
		o = umulhi_ov(al, ah, bl, bh)
	end
	return lo, hi, if o then 1 else 0
end
local function u64tof(lo: number, hi: number): number
	return hi * 0x100000000 + lo
end
local function i64tof(lo: number, hi: number): number
	if hi >= 0x80000000 then hi -= 0x100000000 end
	return hi * 0x100000000 + lo
end
local function ftoi64(x: number): (number, number)
	if x ~= x then return 0, 0 end
	x = trunc(x)
	if x >= 9223372036854775807 then return 0xFFFFFFFF, 0x7FFFFFFF end
	if x <= -9223372036854775808 then return 0, 0x80000000 end
	local hi = math.floor(x / 0x100000000)
	local lo = x - hi * 0x100000000
	return lo, hi % 0x100000000
end
local function ftou64(x: number): (number, number)
	if x ~= x or x <= 0 then return 0, 0 end
	x = trunc(x)
	if x >= 18446744073709551615 then return 0xFFFFFFFF, 0xFFFFFFFF end
	local hi = math.floor(x / 0x100000000)
	return x - hi * 0x100000000, hi
end
local function popcnt64(lo: number, hi: number): number
	return popcnt(lo) + popcnt(hi)
end
local function clz64(lo: number, hi: number): number
	if hi ~= 0 then return bit32.countlz(hi) end
	return 32 + bit32.countlz(lo)
end
local function ctz64(lo: number, hi: number): number
	if lo ~= 0 then return bit32.countrz(lo) end
	return 32 + bit32.countrz(hi)
end

-- runtime imports (extern "C" fn __luau_NAME)
local function rt_print(p: number, n: number)
	print(buffer.readstring(MEM, p, n))
end
local function rt_trap(p: number, n: number)
	error(buffer.readstring(MEM, p, n), 0)
end
local function rt_mem_size(): number
	return buffer.len(MEM)
end
local function rt_mem_grow(n: number): number
	local old = buffer.len(MEM)
	local nb = buffer.create(old + n)
	buffer.copy(nb, 0, MEM, 0, old)
	MEM = nb
	return old
end
local function rt_clock(): number
	return os.clock()
end
local function rt_heap_base(): number
	return HEAP_BASE
end
