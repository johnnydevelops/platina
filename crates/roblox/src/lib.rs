//! Roblox engine API for Rust programs compiled to Luau by [Platina].
//!
//! Every scriptable class, property, method, event and enum in the Roblox API is available
//! with Rust types (generated from Roblox's API dump), plus datatypes (`Vector3`, `CFrame`,
//! `Color3`, `UDim2`, ...) implemented natively in Rust.
//!
//! ```ignore
//! use roblox::prelude::*;
//!
//! #[unsafe(no_mangle)]
//! pub extern "C" fn main() {
//!     let part = Part::new();
//!     part.set_name("Hello");
//!     part.set_position(Vector3::new(0.0, 10.0, 0.0));
//!     part.set_color(Color3::from_rgb(255, 0, 0));
//!     part.set_parent(Some(&Workspace::get()));
//!     part.touched().connect(|other: Option<BasePart>| {
//!         if let Some(other) = other {
//!             println!("touched by {}", other.name());
//!         }
//!     });
//! }
//! ```
//!
//! Instances and other Luau values are held through handles into a table on the Luau side;
//! cloning a Rust value clones the handle, dropping it releases it.
//!
//! [Platina]: https://github.com/johnnydevelops/platina
#![no_std]
#![allow(clippy::too_many_arguments, clippy::new_without_default)]

extern crate alloc;

#[doc(hidden)]
pub mod rt;

mod datatypes;
mod signal;
pub mod task;
mod value;

#[allow(non_upper_case_globals, non_snake_case, clippy::all)]
pub mod enums;
#[allow(non_snake_case, clippy::all)]
mod classes;

pub use classes::*;
pub use datatypes::*;
pub use signal::{Connection, Signal, SignalArgs};
pub use value::{FromValue, IntoValue, Value};

/// Common imports: classes, datatypes, enums and signals.
pub mod prelude {
    pub use crate::classes::*;
    pub use crate::datatypes::*;
    pub use crate::enums;
    pub use crate::signal::{Connection, Signal};
    pub use crate::task;
    pub use crate::value::{FromValue, IntoValue, Value};
    pub use crate::{Class, game};
}

use rt::Handle;

/// A Roblox class: a handle to an Instance known to be of this class (or a subclass).
pub trait Class: Sized + Clone {
    /// The Roblox class name.
    const NAME: &'static str;
    #[doc(hidden)]
    fn from_handle(h: Handle) -> Self;
    #[doc(hidden)]
    fn handle(&self) -> &Handle;
    /// The instance as a plain `Instance`.
    fn as_instance(&self) -> Instance {
        Instance::from_handle(self.handle().clone())
    }
    /// Downcast (`IsA`): the same instance as `T` if it is one.
    fn cast<T: Class>(&self) -> Option<T> {
        if self.as_instance().is_a(T::NAME) { Some(T::from_handle(self.handle().clone())) } else { None }
    }
}

/// The `game` DataModel.
pub fn game() -> DataModel {
    rt::luau_fn! { fn game_h() -> u32 = "function() return rt_href(game) end"; }
    DataModel::from_handle(unsafe { Handle::from_raw(game_h()) })
}

/// Create an instance of class `T` (`Instance.new(T.Name)`).
pub fn new_instance<T: Class>() -> T {
    let h = unsafe { rt::instance_new(T::NAME.as_ptr(), T::NAME.len()) };
    T::from_handle(unsafe { Handle::from_raw(h) })
}
