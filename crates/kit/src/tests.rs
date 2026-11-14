//! Headless tests with a bare `egui::Context`.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};

use crate::{MenuBar, Theme, ThemePref, theme};

fn frame(ctx: &egui::Context, events: Vec<Event>, f: impl FnMut(&mut egui::Ui)) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
    let _ = ctx.run_ui(raw, f);
}

fn key(k: Key) -> Event {
    Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

/// A bar with File (Open, disabled Save, Quit) and View; returns what was activated.
fn bar(ctx: &egui::Context, events: Vec<Event>) -> Vec<&'static str> {
    let mut hit = Vec::new();
    frame(ctx, events, |ui| {
        MenuBar::new("bar").show(ui, |b| {
            b.menu("File", |m| {
                for (label, enabled) in [("Open", true), ("Save", false), ("Quit", true)] {
                    if m.item(label, None, enabled) {
                        hit.push(label);
                    }
                }
            });
            b.menu("View", |m| {
                if m.item("Zoom", None, true) {
                    hit.push("Zoom");
                }
            });
        });
    });
    hit
}

#[test]
fn menu_mouse_and_keys() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let file = pos2(15.0, 12.0);
    bar(&ctx, vec![Event::PointerMoved(file)]);
    bar(&ctx, vec![button(file, true)]);
    assert!(MenuBar::is_open(&ctx, "bar"), "a press on the title opens");
    bar(&ctx, vec![button(file, false)]);
    assert!(MenuBar::is_open(&ctx, "bar"), "releasing on the title keeps it open");
    // Down twice skips the disabled item; Enter activates Quit and closes.
    assert_eq!(bar(&ctx, vec![key(Key::ArrowDown), key(Key::ArrowDown), key(Key::Enter)]), vec!["Quit"]);
    assert!(!MenuBar::is_open(&ctx, "bar"));

    bar(&ctx, vec![button(file, true), button(file, false)]);
    bar(&ctx, vec![key(Key::ArrowRight)]);
    bar(&ctx, vec![]);
    assert_eq!(bar(&ctx, vec![key(Key::ArrowDown), key(Key::Enter)]), vec!["Zoom"], "Right switched to View");

    bar(&ctx, vec![button(file, true), button(file, false)]);
    assert_eq!(bar(&ctx, vec![key(Key::End), key(Key::ArrowUp), key(Key::Enter)]), vec!["Open"], "keys apply in order");

    bar(&ctx, vec![button(file, true), button(file, false)]);
    bar(&ctx, vec![key(Key::Escape)]);
    assert!(!MenuBar::is_open(&ctx, "bar"), "Esc closes");

    bar(&ctx, vec![button(file, true), button(file, false)]);
    let outside = pos2(500.0, 400.0);
    bar(&ctx, vec![Event::PointerMoved(outside), button(outside, true)]);
    assert!(!MenuBar::is_open(&ctx, "bar"), "a press outside closes");
}

#[test]
fn themes_follow_preference() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    assert!(Theme::get(&ctx).dark, "dark without a system preference");
    ctx.set_theme(ThemePref::Light.egui());
    assert!(!Theme::get(&ctx).dark);
    assert_eq!(ctx.style_of(ctx.theme()).visuals.panel_fill, Theme::light().colors.panel, "egui style follows the tokens");
    ctx.set_theme(ThemePref::System.egui());
    let raw = RawInput { system_theme: Some(egui::Theme::Light), ..Default::default() };
    let _ = ctx.run_ui(raw, |_| {});
    assert!(!Theme::get(&ctx).dark, "follows the system");
}
