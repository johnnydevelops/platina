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
local function oob(len: number, idx: number, file: string, line: number, col: number)
	error(string.format("index out of bounds: the len is %d but the index is %d at %s:%d:%d", len, idx, file, line, col), 0)
end
local function trap(msg: string, file: string, line: number, col: number)
	error(string.format("%s at %s:%d:%d", msg, file, line, col), 0)
end
local function unreachable()
	error("entered unreachable code", 0)
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

-- runtime imports (extern "C" fn __luau_NAME) and the std platform layer

-- heap: power-of-two size classes with free lists, bump allocation, growable buffer
local HEAP_TOP: number = HEAP_BASE
local FREE: { number } = table.create(20, 0)
local function heap_grow(need: number)
	local len = buffer.len(MEM)
	local newlen = math.max(need, len + len // 2)
	newlen = (newlen + 65535) // 65536 * 65536
	if newlen > 0x40000000 then
		newlen = math.max(need, 0x40000000)
		if newlen > 0x40000000 then error("out of memory (buffer limit)", 0) end
	end
	local nb = buffer.create(newlen)
	buffer.copy(nb, 0, MEM, 0, len)
	MEM = nb
end
local function heap_bump(size: number, align: number): number
	local start = (HEAP_TOP + align - 1) // align * align
	local new = start + size
	if new > buffer.len(MEM) then heap_grow(new) end
	HEAP_TOP = new
	return start
end
local function rt_alloc(size: number, align: number): number
	local n = math.max(size, align, 8)
	local c = 29 - bit32.countlz(n - 1)
	if c < 20 then
		local head = FREE[c + 1]
		if head ~= 0 then
			FREE[c + 1] = buffer.readu32(MEM, head)
			return head
		end
		local bs = bit32.lshift(8, c)
		return heap_bump(bs, math.min(bs, 4096))
	end
	return heap_bump(size, math.max(align, 8))
end
local function rt_dealloc(p: number, size: number, align: number)
	local n = math.max(size, align, 8)
	local c = 29 - bit32.countlz(n - 1)
	if c < 20 then
		buffer.writeu32(MEM, p, FREE[c + 1])
		FREE[c + 1] = p
	end
end
local function rt_alloc_zeroed(size: number, align: number): number
	local p = rt_alloc(size, align)
	buffer.fill(MEM, p, 0, size)
	return p
end
local function rt_realloc(p: number, size: number, align: number, newsize: number): number
	local n0 = math.max(size, align, 8)
	local n1 = math.max(newsize, align, 8)
	local c0 = 29 - bit32.countlz(n0 - 1)
	if c0 < 20 and c0 == 29 - bit32.countlz(n1 - 1) then return p end
	local q = rt_alloc(newsize, align)
	buffer.copy(MEM, q, MEM, p, math.min(size, newsize))
	rt_dealloc(p, size, align)
	return q
end

-- stdio: line buffered; stdout -> print, stderr -> warn (recent stderr kept for abort)
local LINEBUF: { string } = { "", "" }
local RECENT_ERR: { string } = {}
local warn_ = (warn :: any) or function(...) print(...) end
local function rt_write(fd: number, p: number, n: number)
	local s = LINEBUF[fd] .. buffer.readstring(MEM, p, n)
	while true do
		local i = string.find(s, "\n", 1, true)
		if not i then break end
		local line = string.sub(s, 1, i - 1)
		s = string.sub(s, i + 1)
		if fd == 1 then
			print(line)
		else
			warn_(line)
			table.insert(RECENT_ERR, line)
			if #RECENT_ERR > 16 then table.remove(RECENT_ERR, 1) end
		end
	end
	LINEBUF[fd] = s
end
local function rt_abort()
	if LINEBUF[2] ~= "" then
		table.insert(RECENT_ERR, LINEBUF[2])
		LINEBUF[2] = ""
	end
	local msg = table.concat(RECENT_ERR, "\n")
	table.clear(RECENT_ERR)
	error(if msg == "" then "aborted" else msg, 0)
end

-- std panic hook: message (ptr,len; ptr 0 = non-string payload) and &Location field offsets
local function rt_panic_msg(mp: number, ml: number, loc: number, fo: number, lo: number, co: number)
	local msg = if mp ~= 0 then buffer.readstring(MEM, mp, ml) else "Box<dyn Any>"
	local where_ = ""
	if loc ~= 0 then
		local fp = buffer.readu32(MEM, loc + fo)
		local fl = buffer.readu32(MEM, loc + fo + 4)
		where_ = string.format(" at %s:%d:%d", buffer.readstring(MEM, fp, fl), buffer.readu32(MEM, loc + lo), buffer.readu32(MEM, loc + co))
	end
	table.insert(RECENT_ERR, "panicked" .. where_ .. ":\n" .. msg)
end
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
	heap_grow(old + n)
	return old
end
local function rt_clock(): number
	return os.clock()
end
local function rt_heap_base(): number
	return HEAP_BASE
end
-- time as (secs lo, secs hi, nanos)
local function rt_instant(): (number, number, number)
	local t = os.clock()
	local secs = math.floor(t)
	return secs, 0, math.floor((t - secs) * 1e9)
end
local function rt_systime(): (number, number, number)
	local dt = (DateTime :: any)
	local ms = if dt then dt.now().UnixTimestampMillis else os.time() * 1000
	local secs = ms // 1000
	return secs % 0x100000000, secs // 0x100000000, (ms % 1000) * 1000000
end

-- 128-bit integers as four u32 limbs (least significant first)
local function add128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number)
	local r0 = a0 + b0
	local c = if r0 >= 0x100000000 then 1 else 0
	r0 -= c * 0x100000000
	local r1 = a1 + b1 + c
	c = if r1 >= 0x100000000 then 1 else 0
	r1 -= c * 0x100000000
	local r2 = a2 + b2 + c
	c = if r2 >= 0x100000000 then 1 else 0
	r2 -= c * 0x100000000
	return r0, r1, r2, (a3 + b3 + c) % 0x100000000
end
local function sub128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number)
	local r0 = a0 - b0
	local c = if r0 < 0 then 1 else 0
	r0 += c * 0x100000000
	local r1 = a1 - b1 - c
	c = if r1 < 0 then 1 else 0
	r1 += c * 0x100000000
	local r2 = a2 - b2 - c
	c = if r2 < 0 then 1 else 0
	r2 += c * 0x100000000
	return r0, r1, r2, (a3 - b3 - c) % 0x100000000
end
local function neg128(a0: number, a1: number, a2: number, a3: number): (number, number, number, number)
	return sub128(0, 0, 0, 0, a0, a1, a2, a3)
end
-- full product limbs (n result limbs)
local function mulwide(a: { number }, b: { number }, n: number): { number }
	local r = table.create(n, 0)
	for i = 1, 4 do
		if a[i] ~= 0 then
			for j = 1, 4 do
				local k = i + j - 1
				if k > n then break end
				local lo, hi = mulu32(a[i], b[j])
				local add = lo
				local kk = k
				while add ~= 0 and kk <= n do
					local t = r[kk] + add
					r[kk] = t % 0x100000000
					add = (t - r[kk]) / 0x100000000
					kk += 1
				end
				add = hi
				kk = k + 1
				while add ~= 0 and kk <= n do
					local t = r[kk] + add
					r[kk] = t % 0x100000000
					add = (t - r[kk]) / 0x100000000
					kk += 1
				end
			end
		end
	end
	return r
end
local function mul128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number)
	if a1 == 0 and a2 == 0 and a3 == 0 and b1 == 0 and b2 == 0 and b3 == 0 then
		local lo, hi = mulu32(a0, b0)
		return lo, hi, 0, 0
	end
	local r = mulwide({ a0, a1, a2, a3 }, { b0, b1, b2, b3 }, 4)
	return r[1], r[2], r[3], r[4]
end
local function cmpu128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): number
	if a3 ~= b3 then return if a3 < b3 then -1 else 1 end
	if a2 ~= b2 then return if a2 < b2 then -1 else 1 end
	if a1 ~= b1 then return if a1 < b1 then -1 else 1 end
	if a0 ~= b0 then return if a0 < b0 then -1 else 1 end
	return 0
end
local function cmp128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number, signed: boolean): number
	if signed then
		local na, nb = a3 >= 0x80000000, b3 >= 0x80000000
		if na ~= nb then return if na then -1 else 1 end
	end
	return cmpu128(a0, a1, a2, a3, b0, b1, b2, b3)
end
local function shl128(a0: number, a1: number, a2: number, a3: number, n: number): (number, number, number, number)
	local a = { a0, a1, a2, a3 }
	local w = n // 32
	local s = n % 32
	local r = { 0, 0, 0, 0 }
	for i = 4, 1, -1 do
		local src = i - w
		if src >= 1 then
			local v = bit32.lshift(a[src], s)
			if s > 0 and src - 1 >= 1 then v = bit32.bor(v, bit32.rshift(a[src - 1], 32 - s)) end
			r[i] = v
		end
	end
	return r[1], r[2], r[3], r[4]
end
local function shr128x(a0: number, a1: number, a2: number, a3: number, n: number, fill: number): (number, number, number, number)
	local a = { a0, a1, a2, a3 }
	local w = n // 32
	local s = n % 32
	local r = { fill, fill, fill, fill }
	for i = 1, 4 do
		local src = i + w
		if src <= 4 then
			local hiword = if src + 1 <= 4 then a[src + 1] else fill
			local v = bit32.rshift(a[src], s)
			if s > 0 then v = bit32.bor(v, bit32.lshift(hiword, 32 - s)) end
			r[i] = v
		end
	end
	return r[1], r[2], r[3], r[4]
end
local function shr128(a0: number, a1: number, a2: number, a3: number, n: number): (number, number, number, number)
	return shr128x(a0, a1, a2, a3, n, 0)
end
local function sar128(a0: number, a1: number, a2: number, a3: number, n: number): (number, number, number, number)
	return shr128x(a0, a1, a2, a3, n, if a3 >= 0x80000000 then 0xFFFFFFFF else 0)
end
local function udivmod128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number, number, number, number, number)
	if a2 == 0 and a3 == 0 and b2 == 0 and b3 == 0 then
		local ql, qh, rl, rh = udivmod64(a0, a1, b0, b1)
		return ql, qh, 0, 0, rl, rh, 0, 0
	end
	local q = { 0, 0, 0, 0 }
	local r0, r1, r2, r3 = 0, 0, 0, 0
	local a = { a0, a1, a2, a3 }
	for i = 127, 0, -1 do
		r0, r1, r2, r3 = shl128(r0, r1, r2, r3, 1)
		r0 = bit32.bor(r0, bit32.extract(a[i // 32 + 1], i % 32))
		if cmpu128(r0, r1, r2, r3, b0, b1, b2, b3) >= 0 then
			r0, r1, r2, r3 = sub128(r0, r1, r2, r3, b0, b1, b2, b3)
			q[i // 32 + 1] = bit32.replace(q[i // 32 + 1], 1, i % 32)
		end
	end
	return q[1], q[2], q[3], q[4], r0, r1, r2, r3
end
local function udiv128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number)
	local q0, q1, q2, q3 = udivmod128(a0, a1, a2, a3, b0, b1, b2, b3)
	return q0, q1, q2, q3
end
local function urem128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number)
	local _, _, _, _, r0, r1, r2, r3 = udivmod128(a0, a1, a2, a3, b0, b1, b2, b3)
	return r0, r1, r2, r3
end
local function idiv128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number)
	local na, nb = a3 >= 0x80000000, b3 >= 0x80000000
	if na then a0, a1, a2, a3 = neg128(a0, a1, a2, a3) end
	if nb then b0, b1, b2, b3 = neg128(b0, b1, b2, b3) end
	local q0, q1, q2, q3 = udivmod128(a0, a1, a2, a3, b0, b1, b2, b3)
	if na ~= nb then return neg128(q0, q1, q2, q3) end
	return q0, q1, q2, q3
end
local function irem128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number): (number, number, number, number)
	local na = a3 >= 0x80000000
	if na then a0, a1, a2, a3 = neg128(a0, a1, a2, a3) end
	if b3 >= 0x80000000 then b0, b1, b2, b3 = neg128(b0, b1, b2, b3) end
	local _, _, _, _, r0, r1, r2, r3 = udivmod128(a0, a1, a2, a3, b0, b1, b2, b3)
	if na then return neg128(r0, r1, r2, r3) end
	return r0, r1, r2, r3
end
local function addo128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number, signed: boolean): (number, number, number, number, number)
	local r0, r1, r2, r3 = add128(a0, a1, a2, a3, b0, b1, b2, b3)
	local o
	if signed then
		local sa, sb, sr = a3 >= 0x80000000, b3 >= 0x80000000, r3 >= 0x80000000
		o = sa == sb and sr ~= sa
	else
		o = cmpu128(r0, r1, r2, r3, a0, a1, a2, a3) < 0
	end
	return r0, r1, r2, r3, if o then 1 else 0
end
local function subo128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number, signed: boolean): (number, number, number, number, number)
	local r0, r1, r2, r3 = sub128(a0, a1, a2, a3, b0, b1, b2, b3)
	local o
	if signed then
		local sa, sb, sr = a3 >= 0x80000000, b3 >= 0x80000000, r3 >= 0x80000000
		o = sa ~= sb and sr ~= sa
	else
		o = cmpu128(a0, a1, a2, a3, b0, b1, b2, b3) < 0
	end
	return r0, r1, r2, r3, if o then 1 else 0
end
local function mulo128(a0: number, a1: number, a2: number, a3: number, b0: number, b1: number, b2: number, b3: number, signed: boolean): (number, number, number, number, number)
	local na, nb = false, false
	if signed then
		na, nb = a3 >= 0x80000000, b3 >= 0x80000000
		if na then a0, a1, a2, a3 = neg128(a0, a1, a2, a3) end
		if nb then b0, b1, b2, b3 = neg128(b0, b1, b2, b3) end
	end
	local r = mulwide({ a0, a1, a2, a3 }, { b0, b1, b2, b3 }, 8)
	local o = r[5] ~= 0 or r[6] ~= 0 or r[7] ~= 0 or r[8] ~= 0
	local r0, r1, r2, r3 = r[1], r[2], r[3], r[4]
	if signed then
		if na ~= nb then
			o = o or r3 > 0x80000000 or (r3 == 0x80000000 and (r0 ~= 0 or r1 ~= 0 or r2 ~= 0))
			r0, r1, r2, r3 = neg128(r0, r1, r2, r3)
		else
			o = o or r3 >= 0x80000000
		end
	end
	return r0, r1, r2, r3, if o then 1 else 0
end
local function i128tof(a0: number, a1: number, a2: number, a3: number, signed: boolean): number
	local neg = signed and a3 >= 0x80000000
	if neg then a0, a1, a2, a3 = neg128(a0, a1, a2, a3) end
	local v = ((a3 * 0x100000000 + a2) * 0x100000000 + a1) * 0x100000000 + a0
	return if neg then -v else v
end
local function ftoi128(x: number, signed: boolean): (number, number, number, number)
	if x ~= x then return 0, 0, 0, 0 end
	x = trunc(x)
	local neg = x < 0
	if neg and not signed then return 0, 0, 0, 0 end
	local m = math.abs(x)
	local limit = if signed then 2 ^ 127 else 2 ^ 128
	if m >= limit then
		if not signed then return 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF end
		if neg then return 0, 0, 0, 0x80000000 end
		return 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0x7FFFFFFF
	end
	local r = {}
	for i = 1, 4 do
		local q = math.floor(m / 0x100000000)
		r[i] = m - q * 0x100000000
		m = q
	end
	if neg then return neg128(r[1], r[2], r[3], r[4]) end
	return r[1], r[2], r[3], r[4]
end

-- static data decoder (base64, no padding)
local function b64write(at: number, s: string)
	local dec = table.create(256, 0)
	local abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
	for i = 1, 64 do dec[string.byte(abc, i)] = i - 1 end
	local n = #s
	local i = 1
	while i + 3 <= n do
		local a, b, c, d = string.byte(s, i, i + 3)
		local v = dec[a] * 262144 + dec[b] * 4096 + dec[c] * 64 + dec[d]
		buffer.writeu8(MEM, at, v // 65536)
		buffer.writeu8(MEM, at + 1, (v // 256) % 256)
		buffer.writeu8(MEM, at + 2, v % 256)
		at += 3
		i += 4
	end
	local rem = n - i + 1
	if rem >= 2 then
		local a, b = string.byte(s, i, i + 1)
		local v = dec[a] * 262144 + dec[b] * 4096
		if rem == 3 then v += dec[string.byte(s, i + 2)] * 64 end
		buffer.writeu8(MEM, at, v // 65536)
		if rem == 3 then buffer.writeu8(MEM, at + 1, (v // 256) % 256) end
	end
end
