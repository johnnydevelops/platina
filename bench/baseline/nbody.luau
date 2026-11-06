--!native
--!optimize 2
local function bench(steps: number): number
	local x = { 0.0, 4.84, 8.34, 12.89, 15.37 }
	local y = { 0.0, -1.16, 4.12, -15.11, -25.91 }
	local z = { 0.0, -0.10, -0.40, -0.22, 0.179 }
	local vx = { 0.0, 0.606, -1.01, 1.08, 0.979 }
	local vy = { 0.0, 2.81, 1.82, 0.868, 0.594 }
	local vz = { 0.0, -0.02, 0.008, -0.01, -0.034 }
	local m = { 39.47, 0.037, 0.011, 0.0017, 0.002 }
	local dt = 0.01
	for _ = 1, steps do
		for i = 1, 5 do
			for j = i + 1, 5 do
				local dx, dy, dz = x[i] - x[j], y[i] - y[j], z[i] - z[j]
				local d2 = dx * dx + dy * dy + dz * dz
				local mag = dt / (d2 * math.sqrt(d2))
				local mj, mi = m[j], m[i]
				vx[i] -= dx * mj * mag
				vy[i] -= dy * mj * mag
				vz[i] -= dz * mj * mag
				vx[j] += dx * mi * mag
				vy[j] += dy * mi * mag
				vz[j] += dz * mi * mag
			end
		end
		for i = 1, 5 do
			x[i] += dt * vx[i]
			y[i] += dt * vy[i]
			z[i] += dt * vz[i]
		end
	end
	local e = 0.0
	for i = 1, 5 do e += m[i] * (vx[i] * vx[i] + vy[i] * vy[i] + vz[i] * vz[i]) end
	return math.floor(e * 1e6)
end
return { bench = bench }
