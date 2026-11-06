//! The task scheduler (`task.wait`, `task.spawn`, `task.delay`, `task.defer`). Rust code may
//! yield: while it is suspended, other Rust code runs on a separate shadow stack.

use alloc::boxed::Box;

use crate::rt::luau_fn;
use crate::signal::{dispatch_slot, register};
use crate::value::Value;

luau_fn! {
    fn t_wait(s: f64) -> f64 = "function(s) return rt_yield(task.wait, s) end";
    fn t_run(how: u32, s: f64, slot: u32, id: u32) = "function(how, s, slot, id)
        local f = FN[slot]
        local function run()
            local sp = SP
            local ok, err = pcall(f, id, 0)
            SP = sp
            if not ok then error(err, 0) end
        end
        if how == 0 then task.spawn(run) elseif how == 1 then task.delay(s, run) else task.defer(run) end
    end";
    fn t_clock() -> f64 = "os.clock";
}

/// Yield for `seconds` (at least one frame); returns the time actually waited.
pub fn wait(seconds: f64) -> f64 {
    unsafe { t_wait(seconds) }
}

fn run(how: u32, seconds: f64, f: impl FnOnce() + 'static) {
    let mut f = Some(f);
    let id = register(
        Box::new(move |_: Value| {
            if let Some(f) = f.take() {
                f()
            }
        }),
        true,
    );
    unsafe { t_run(how, seconds, dispatch_slot(), id) }
}

/// Run `f` now in a new thread, until it first yields.
pub fn spawn(f: impl FnOnce() + 'static) {
    run(0, 0.0, f)
}

/// Run `f` after `seconds`.
pub fn delay(seconds: f64, f: impl FnOnce() + 'static) {
    run(1, seconds, f)
}

/// Run `f` at the end of the current resumption cycle.
pub fn defer(f: impl FnOnce() + 'static) {
    run(2, 0.0, f)
}

/// `os.clock()`.
pub fn clock() -> f64 {
    unsafe { t_clock() }
}
