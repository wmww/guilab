//! Headless smithay compositor: one [`Session`] per Wayland display.
//!
//! No IPC or CLI here; the server crate drives sessions (event loop nesting, frame timer,
//! requests). See notes/code-map.md.

mod actions;
mod bus;
mod clipboard;
mod cursor;
mod focus;
mod frame;
mod grabs;
mod handlers;
mod input;
mod keys;
pub mod procs;
pub mod render;
mod session;
mod settle;
mod shell;
mod tree;
mod xscale;
mod xwayland;

pub use actions::Chain;
pub use clipboard::{ClipRead, Clipboard};
pub use cursor::{AppCursor, CursorImage};
pub use input::{InputEvent, InputSource};
pub use settle::{Settle, ShotReady};
pub use handlers::{SurfaceId, surface_id};
pub use render::Gpu;
pub use xwayland::XState;
pub use session::{SOCKET_NAME, Session, SessionConfig, SharedGpu};
pub use smithay::reexports::calloop;
