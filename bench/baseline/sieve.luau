--!native
--!optimize 2
local function bench(n: number): number
	local v = buffer.create(n)
	buffer.fill(v, 0, 1, n)
	buffer.writeu8(v, 0, 0)
	buffer.writeu8(v, 1, 0)
	local i = 2
	while i * i < n do
		if buffer.readu8(v, i) ~= 0 then
			for j = i * i, n - 1, i do
				buffer.writeu8(v, j, 0)
			end
		end
		i += 1
	end
	local c = 0
	for k = 0, n - 1 do
		if buffer.readu8(v, k) ~= 0 then c += 1 end
	end
	return c
end
return { bench = bench }
