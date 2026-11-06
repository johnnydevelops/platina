# roblox

The Roblox engine API for Rust programs compiled to Luau by [Platina](../../README.md).

```toml
[dependencies]
roblox = { git = "https://github.com/johnnydevelops/platina" }
```

```rust
use roblox::prelude::*;

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let part = Part::new();
    part.set_size(Vector3::new(4.0, 1.0, 4.0));
    part.set_cframe(CFrame::new(0.0, 10.0, 0.0) * CFrame::angles(0.0, 0.5, 0.0));
    part.set_color(Color3::from_rgb(255, 80, 0));
    part.set_material(enums::Material::Neon);
    part.set_parent(Some(&Workspace::get()));

    part.touched().connect(|other: Option<BasePart>| {
        if let Some(other) = other {
            println!("touched by {}", other.name());
        }
    });
    RunService::get().heartbeat().connect(move |dt: f64| { /* every frame */ });

    task::wait(1.0); // Rust code can yield
}
```

A one-line Script runs it: `require(path.to.Module).main()`.

## What's included

**Every scriptable class, property, method, event and enum,** generated from Roblox's API dump
(`gen/gen.py`):

| Roblox | Rust |
| --- | --- |
| `part.Anchored`, `part.Anchored = true` | `part.anchored()`, `part.set_anchored(true)` |
| `inst:FindFirstChild("X")` | `inst.find_first_child("X", false) -> Option<Instance>` |
| `inst:GetChildren()` | `inst.get_children() -> Vec<Instance>` |
| `inst:Clone()` | `inst.clone_instance()` (`.clone()` clones the Rust handle) |
| `game:GetService("Players")` | `Players::get()` |
| `Instance.new("Part")` | `Part::new()` |
| `Enum.Material.Neon` | `enums::Material::Neon` |
| `inst:IsA("BasePart")` | `inst.cast::<BasePart>() -> Option<BasePart>` |
| `part.Touched:Connect(f)` | `part.touched().connect(\|other: Option<BasePart>\| ...)` |
| `remote.OnServerEvent` (`player, ...`) | `.connect(\|player: Option<Player>, args: Varargs\| ...)` |
| `signal:Wait()` | `signal.wait()` |

- **Inheritance:** a subclass dereferences to its superclass, so a `Part` has every `BasePart`
  and `Instance` method.
- **Arguments:** methods that take an instance accept any subclass. Optional instance
  properties take `Option<&T>`.
- **Yielding methods** (`WaitForChild`, `InvokeServer`, ...) work from Rust.

**Datatypes are plain Rust structs** with the math done in Rust: `Vector3`, `Vector2`, `CFrame`
(`angles`, `look_at`, `inverse`, `*`), `Color3` (`from_rgb`, `from_hsv`), `UDim`, `UDim2`,
`Rect`, `NumberRange` and `BrickColor`. They cross into Luau as plain numbers.

**`Value`, for anything else:** any Luau value, with `get`/`set`, `call`/`call_method`,
`Value::global("Content")`, `Value::require(module)`, tables, buffers (`buffer_from`,
`to_bytes`, `read_into`) and conversions. Members whose types have no Rust mapping (`Content`,
`Font`, `RaycastParams`, ...) take and return `Value`, so the whole API stays reachable.

**`task`:** `wait`, `spawn`, `delay`, `defer`, `clock`.

## How it works

- **Glue functions.** Each binding is an `extern "C"` function whose link name is a Luau
  snippet, for example `luau::function(h) return rt_hget(h).Anchored end`. Platina's backend
  emits each snippet once, and only if it's called, so binding the whole API costs nothing for
  unused parts.
- **Handles.** Luau values (Instances, signals, tables) live in a handle table on the Luau
  side. Rust holds `u32` handles: `Clone` duplicates a handle and `Drop` releases it.
- **Callbacks.** Rust closures are stored in a slab. Luau calls them through the function
  pointer table, with the event's arguments packed into one handle.
- **Yielding.** When Rust code yields, the shadow stack moves to a fresh segment, so other Rust
  code that runs meanwhile can't overwrite the suspended frames.
- **Regenerating** after a Roblox update:
  `python3 gen/gen.py API-Dump.json` (from
  [Roblox-Client-Tracker](https://github.com/MaximumADHD/Roblox-Client-Tracker)).

## Limitations

- **Instances only by API type.** Events and properties typed as a class give you that class.
  Use `cast` to downcast.
- **Exact names only.** There's no `__index` sugar: `inst.Foo` (a child by name) is
  `inst.find_first_child("Foo", false)`.
- **Panics in a handler** surface as a Luau error from that event connection. The program's
  other state is left as it was.
