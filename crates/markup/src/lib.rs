//! Drawing on screenshots, queueing them, and a chat panel, as plain egui widgets.
//!
//! Depends only on egui; no windowing, IPC or app types, so it can be copied into another
//! egui app. The host:
//! - shows [`Sketch::toolbar`] somewhere (a narrow side panel),
//! - while [`Sketch::active`], calls [`Sketch::canvas`] with where its image is on screen
//!   and stops using pointer input over the image itself,
//! - answers [`Sketch::wants_screenshot`] with [`Sketch::set_screenshot`],
//! - shows [`Chat::show`] (a bottom panel), pushes [`FeedItem`]s into it, and sends the
//!   [`Outgoing`] messages it returns (composited images plus text annotations).
//!
//! [`Renderer`] composites marks onto an image on the CPU (no GPU needed); the
//! `compose` example uses it standalone.

pub mod chat;
pub mod mark;
pub mod raster;
pub mod sketch;

pub use egui;
pub use chat::{Chat, FeedItem, Outgoing, Who};
pub use mark::{COLORS, Kind, Mark, Tool};
pub use raster::Renderer;
pub use sketch::{Annotated, Note, Sketch, View};

#[cfg(test)]
mod tests;

/// Consumes a press of Enter with no modifiers. Unlike `consume_key`, which ignores Shift,
/// this leaves Shift+Enter for text boxes to turn into a newline.
pub(crate) fn consume_plain_enter(i: &mut egui::InputState) -> bool {
    let before = i.events.len();
    i.events.retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.matches_exact(egui::Modifiers::NONE)));
    i.events.len() != before
}
