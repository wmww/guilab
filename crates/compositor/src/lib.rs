//! Headless smithay compositor: one [`Session`] per Wayland display.
//!
//! No IPC or CLI here; the server crate drives sessions (event loop nesting, frame timer,
//! requests). See notes/code-map.md.

mod actions;
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
mod shell;
mod tree;
mod xwayland;

pub use actions::Chain;
pub use clipboard::ClipRead;
pub use cursor::{AppCursor, CursorImage};
pub use input::{InputEvent, InputSource};
pub use frame::{SETTLE_QUIET, Settle, write_rgba_png};
pub use handlers::{SurfaceId, surface_id};
pub use render::Gpu;
pub use xwayland::XState;
pub use session::{ClientState, SOCKET_NAME, Session, SessionConfig, SharedGpu};
pub use smithay::reexports::calloop;
