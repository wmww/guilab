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
pub mod menu;
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

/// A toast: a floating bar at the bottom center of `clip`, for status with a few buttons.
/// `slot` 0 is lowest; higher slots stack above it.
pub fn toast(ui: &egui::Ui, id: egui::Id, clip: egui::Rect, slot: usize, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let y = clip.max.y - 20.0 - slot as f32 * 64.0;
    egui::Area::new(id)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(clip.center().x, y))
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(16, 10)).corner_radius(10.0).show(ui, |ui| {
                ui.style_mut().text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
                ui.style_mut().text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
                ui.spacing_mut().button_padding = egui::vec2(10.0, 4.0);
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.horizontal(add);
            });
        })
        .response
}
