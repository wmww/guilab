//! Headless smithay compositor: one [`Session`] per Wayland display.
//!
//! No IPC or CLI here; the server crate drives sessions (event loop nesting, frame timer,
//! requests). See notes/code-map.md.

mod actions;
mod clipboard;
mod frame;
mod handlers;
mod input;
mod keys;
pub mod procs;
pub mod render;
mod session;
mod shell;
mod tree;

pub use actions::Chain;
pub use clipboard::ClipRead;
pub use input::{InputEvent, InputSource};
pub use frame::{SETTLE_QUIET, Settle};
pub use handlers::{SurfaceId, surface_id};
pub use render::Gpu;
pub use session::{ClientState, SOCKET_NAME, Session, SessionConfig, SharedGpu};
pub use smithay::reexports::calloop;
