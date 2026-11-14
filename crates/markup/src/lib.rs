//! Drawing on screenshots, queueing them, and a chat panel, as plain egui widgets.
//!
//! Depends only on egui and `guilab-kit` (theme and shared widgets); no windowing, IPC or
//! app types. The host:
//! - shows [`Sketch::toolbar`] somewhere (a narrow side panel),
//! - while [`Sketch::active`], calls [`Sketch::canvas`] with where its image is on screen
//!   and stops using pointer input over the image itself,
//! - answers [`Sketch::wants_screenshot`] with [`Sketch::set_screenshot`],
//! - shows [`Chat::show`] (a bottom panel), pushes [`FeedItem`]s into it, keeps
//!   [`Chat::set_status`] current, and sends the [`Outgoing`] messages it returns
//!   (composited images plus text annotations).
//!
//! [`Renderer`] composites marks onto an image on the CPU (no GPU needed); the
//! `compose` example uses it standalone.

pub mod chat;
pub mod mark;
pub mod raster;
pub mod sketch;

pub use egui;
pub use chat::{Chat, FeedItem, Outgoing, Status, Who};
pub use mark::{COLORS, Kind, Mark, Tool};
pub use raster::Renderer;
pub use sketch::{Annotated, Note, Sketch, View};

#[cfg(test)]
mod tests;
