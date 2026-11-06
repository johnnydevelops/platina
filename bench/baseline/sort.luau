--!native
--!optimize 2
local function bench(n: number): number
	local v = table.create(n)
	for i = 0, n - 1 do
		local p = (i * 2654435761) % 0x100000000 -- exact enough? use split multiply
		v[i + 1] = bit32.bxor(p, bit32.rshift(i, 3))
	end
	table.sort(v)
	return v[n // 2 + 1]
end
return { bench = bench }
