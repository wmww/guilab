//! A small egui toolkit: theme tokens in light and dark ([`theme`]), eased motion
//! ([`anim`]), a generic undo [`History`], and desktop-style widgets ([`menu`],
//! [`widgets`]). Depends only on egui; no app types.
//!
//! Call [`theme::install`] once on the context; widgets then read [`Theme::get`].

pub mod anim;
pub mod headless;
pub mod history;
pub mod menu;
pub mod theme;
pub mod widgets;

pub use anim::{Easing, Motion, tween};
pub use history::History;
pub use menu::MenuBar;
pub use theme::{Theme, ThemePref};
pub use widgets::*;

#[cfg(test)]
mod tests;
