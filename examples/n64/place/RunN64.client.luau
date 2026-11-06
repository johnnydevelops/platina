--!optimize 2
-- Super Mario 64 on RustyN64 (a Rust N64 emulator) compiled to Luau by rustc_codegen_luau.
-- Controls: WASD = stick, Space = A, LeftShift = B, Q = Z, Enter = Start, E = R,
--           arrow keys = C buttons.
local AssetService = game:GetService("AssetService")
local ReplicatedStorage = game:GetService("ReplicatedStorage")
local UserInputService = game:GetService("UserInputService")
local Players = game:GetService("Players")

local N = require(ReplicatedStorage:WaitForChild("N64"))

-- screen
local gui = Instance.new("ScreenGui")
gui.Name = "N64"
gui.IgnoreGuiInset = true
gui.ResetOnSpawn = false
local bg = Instance.new("Frame")
bg.Size = UDim2.fromScale(1, 1)
bg.BackgroundColor3 = Color3.new(0, 0, 0)
bg.Parent = gui
local label = Instance.new("ImageLabel")
label.AnchorPoint = Vector2.new(0.5, 0.5)
label.Position = UDim2.fromScale(0.5, 0.5)
label.Size = UDim2.fromScale(0.9, 0.9)
label.BackgroundTransparency = 1
label.ResampleMode = Enum.ResamplerMode.Pixelated
label.Parent = bg
local aspect = Instance.new("UIAspectRatioConstraint")
aspect.AspectRatio = 4 / 3
aspect.Parent = label
local status = Instance.new("TextLabel")
status.Size = UDim2.new(1, 0, 0, 24)
status.BackgroundTransparency = 1
status.TextColor3 = Color3.new(1, 1, 1)
status.Font = Enum.Font.Code
status.TextSize = 16
status.Text = "loading ROM..."
status.Parent = bg
gui.Parent = Players.LocalPlayer:WaitForChild("PlayerGui")

-- let the first-load hitch settle before doing heavy work
for i = 10, 1, -1 do
	status.Text = string.format("starting in %d...", i)
	task.wait(1)
end

-- ROM: base64 chunks in ReplicatedStorage.Rom (Rom.Part01 ...)
local romFolder = ReplicatedStorage:WaitForChild("Rom")
local parts = romFolder:GetChildren()
table.sort(parts, function(a, b) return a.Name < b.Name end)
local B64 = table.create(256, 0)
local abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
for i = 1, 64 do B64[string.byte(abc, i)] = i - 1 end

local romSize = romFolder:GetAttribute("Size")
local ptr = N.n64_alloc(romSize)
local at = ptr
for i, part in parts do
	local s: string = require(part)
	local mem = N.memory()
	local n = #s
	local j = 1
	while j + 3 <= n do
		local a, b, c, d = string.byte(s, j, j + 3)
		local v = B64[a] * 262144 + B64[b] * 4096 + B64[c] * 64 + B64[d]
		buffer.writeu8(mem, at, v // 65536)
		buffer.writeu8(mem, at + 1, (v // 256) % 256)
		buffer.writeu8(mem, at + 2, v % 256)
		at += 3
		j += 4
	end
	status.Text = string.format("loading ROM... %d/%d", i, #parts)
	task.wait()
end
assert(N.n64_load(ptr, romSize) == 0, "N64 boot failed")

-- input
local BUTTONS = {
	[Enum.KeyCode.Space] = 0x8000, -- A
	[Enum.KeyCode.LeftShift] = 0x4000, -- B
	[Enum.KeyCode.Q] = 0x2000, -- Z
	[Enum.KeyCode.Return] = 0x1000, -- Start
	[Enum.KeyCode.E] = 0x0010, -- R
	[Enum.KeyCode.Up] = 0x0008, -- C up
	[Enum.KeyCode.Down] = 0x0004,
	[Enum.KeyCode.Left] = 0x0002,
	[Enum.KeyCode.Right] = 0x0001,
}
local function padWord(): number
	local buttons = 0
	for key, bit in BUTTONS do
		if UserInputService:IsKeyDown(key) then buttons = bit32.bor(buttons, bit) end
	end
	local x, y = 0, 0
	if UserInputService:IsKeyDown(Enum.KeyCode.A) then x -= 80 end
	if UserInputService:IsKeyDown(Enum.KeyCode.D) then x += 80 end
	if UserInputService:IsKeyDown(Enum.KeyCode.W) then y += 80 end
	if UserInputService:IsKeyDown(Enum.KeyCode.S) then y -= 80 end
	return buttons * 65536 + (x % 256) * 256 + (y % 256)
end

-- main loop: one emulated frame per heartbeat
local image: EditableImage? = nil
local imgW, imgH = 0, 0
local frames, t0 = 0, os.clock()
while true do
	N.n64_set_pad(padWord())
	local t = os.clock()
	N.n64_run_frame()
	local dt = os.clock() - t
	frames += 1
	local w, h = N.n64_w(), N.n64_h()
	if w > 0 and h > 0 then
		if w ~= imgW or h ~= imgH then
			image = AssetService:CreateEditableImage({ Size = Vector2.new(w, h) })
			label.ImageContent = Content.fromObject(image)
			imgW, imgH = w, h
		end
		local pixels = buffer.create(w * h * 4)
		buffer.copy(pixels, 0, N.memory(), N.n64_fb(), w * h * 4)
		image:WritePixelsBuffer(Vector2.zero, Vector2.new(w, h), pixels)
	end
	status.Text = string.format("frame %d   %.2fs/frame   %.2f fps avg", frames, dt, frames / (os.clock() - t0))
	task.wait()
end
