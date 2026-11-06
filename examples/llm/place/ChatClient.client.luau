-- Chat UI for the server-side LLM.
local Players = game:GetService("Players")
local ReplicatedStorage = game:GetService("ReplicatedStorage")

local gui = Instance.new("ScreenGui")
gui.Name = "LLMChat"
gui.ResetOnSpawn = false
gui.IgnoreGuiInset = true
local bg = Instance.new("Frame")
bg.Size = UDim2.fromScale(1, 1)
bg.BackgroundColor3 = Color3.fromRGB(24, 24, 28)
bg.Parent = gui

local status = Instance.new("TextLabel")
status.Size = UDim2.new(1, -32, 0, 28)
status.Position = UDim2.fromOffset(16, 8)
status.BackgroundTransparency = 1
status.TextColor3 = Color3.fromRGB(150, 150, 160)
status.Font = Enum.Font.Code
status.TextSize = 16
status.TextXAlignment = Enum.TextXAlignment.Left
status.Text = "waiting for the server..."
status.Parent = bg

local log = Instance.new("ScrollingFrame")
log.Size = UDim2.new(1, -32, 1, -100)
log.Position = UDim2.fromOffset(16, 40)
log.BackgroundTransparency = 1
log.BorderSizePixel = 0
log.ScrollBarThickness = 6
log.AutomaticCanvasSize = Enum.AutomaticSize.Y
log.CanvasSize = UDim2.new()
log.Parent = bg
local layout = Instance.new("UIListLayout")
layout.Padding = UDim.new(0, 10)
layout.SortOrder = Enum.SortOrder.LayoutOrder
layout.Parent = log

local box = Instance.new("TextBox")
box.Size = UDim2.new(1, -32, 0, 40)
box.Position = UDim2.new(0, 16, 1, -52)
box.BackgroundColor3 = Color3.fromRGB(40, 40, 48)
box.TextColor3 = Color3.new(1, 1, 1)
box.PlaceholderText = "Message SmolLM (Enter to send)"
box.PlaceholderColor3 = Color3.fromRGB(120, 120, 130)
box.Font = Enum.Font.Gotham
box.TextSize = 18
box.ClearTextOnFocus = false
box.Text = ""
box.TextXAlignment = Enum.TextXAlignment.Left
box.Parent = bg
Instance.new("UICorner").Parent = box
local pad = Instance.new("UIPadding")
pad.PaddingLeft = UDim.new(0, 10)
pad.Parent = box

gui.Parent = Players.LocalPlayer:WaitForChild("PlayerGui")

local order = 0
local function bubble(text: string, user: boolean): TextLabel
	order += 1
	local t = Instance.new("TextLabel")
	t.LayoutOrder = order
	t.Size = UDim2.new(1, -12, 0, 0)
	t.AutomaticSize = Enum.AutomaticSize.Y
	t.BackgroundTransparency = 1
	t.TextWrapped = true
	t.RichText = false
	t.Font = Enum.Font.Gotham
	t.TextSize = 18
	t.TextXAlignment = Enum.TextXAlignment.Left
	t.TextColor3 = if user then Color3.fromRGB(130, 180, 255) else Color3.fromRGB(235, 235, 240)
	t.Text = text
	t.Parent = log
	return t
end

local remotes = ReplicatedStorage:WaitForChild("ChatRemotes")
local ask: RemoteEvent = remotes:WaitForChild("Ask")
local token: RemoteEvent = remotes:WaitForChild("Token")

local current: TextLabel? = nil
local raw = ""
token.OnClientEvent:Connect(function(kind: string, text: string)
	if kind == "status" then
		status.Text = text
	elseif kind == "user" then
		bubble(text, true)
		raw = ""
		current = bubble("SmolLM: ...", false)
	elseif kind == "text" and current then
		raw ..= text
		current.Text = "SmolLM: " .. raw
		log.CanvasPosition = Vector2.new(0, 1e9)
	elseif kind == "done" then
		current = nil
	end
end)

box.FocusLost:Connect(function(enter)
	if enter and #box.Text > 0 then
		ask:FireServer(box.Text)
		box.Text = ""
	end
end)
