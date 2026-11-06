-- cargo roblox build --release   (in examples/hello), then: lune run run.luau
local M = require("./out/hello-luau")
M.greet()
print("fib(80) =", M.fib(80))
