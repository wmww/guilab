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

/// Toasts in one stack sit above each other in call order, each by its real height; another
/// stack starts again at the bottom, and every pass starts over.
#[test]
fn toasts_stack() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let clip = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
    let stack = egui::Id::new("view");
    let mut rects = Vec::new();
    for _ in 0..4 {
        rects.clear();
        frame(&ctx, vec![], |ui| {
            rects.push(crate::toast(ui, egui::Id::new("a"), stack, clip, |ui| {
                ui.label("one line");
            }).rect);
            rects.push(crate::toast(ui, egui::Id::new("b"), stack, clip, |ui| {
                ui.vertical(|ui| {
                    ui.label("two");
                    ui.label("lines");
                });
            }).rect);
            rects.push(crate::toast(ui, egui::Id::new("c"), stack, clip, |ui| {
                ui.label("top");
            }).rect);
            rects.push(crate::toast(ui, egui::Id::new("d"), egui::Id::new("other"), clip, |ui| {
                ui.label("elsewhere");
            }).rect);
        });
    }
    let t = Theme::get(&ctx);
    let [a, b, c, d] = [rects[0], rects[1], rects[2], rects[3]];
    assert!((a.max.y - (600.0 - t.size.toast_bottom)).abs() < 1.0, "{a:?}");
    assert!(b.height() > a.height() + 5.0, "{a:?} {b:?}");
    assert!((b.max.y - (a.min.y - t.size.toast_gap)).abs() < 1.0, "{a:?} {b:?}");
    assert!((c.max.y - (b.min.y - t.size.toast_gap)).abs() < 1.0, "{b:?} {c:?}");
    assert!((d.max.y - a.max.y).abs() < 1.0, "{d:?}");
}

/// Runs a frame on an 800x100 screen with `File` open (pressed at its title) and returns
/// the native popups taken at the end of the pass.
fn popup_frame(ctx: &egui::Context, events: Vec<Event>) -> Vec<crate::popup::Native> {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 100.0))), events, ..Default::default() };
    let mut natives = Vec::new();
    let _ = ctx.run_ui(raw, |ui| {
        MenuBar::new("bar").show(ui, |b| {
            b.menu("File", |m| {
                for label in ["One", "Two", "Three", "Four", "Five"] {
                    m.item(label, None, true);
                }
            });
        });
        natives = crate::popup::take_native(ui.ctx());
    });
    natives
}

#[test]
fn native_popups_leave_the_window() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let title = pos2(20.0, 10.0);
    // In the window: kept inside the 100 pt screen (it's taller, so it covers the bar).
    popup_frame(&ctx, vec![Event::PointerMoved(title)]);
    popup_frame(&ctx, vec![button(title, true)]);
    popup_frame(&ctx, vec![button(title, false)]);
    assert!(MenuBar::is_open(&ctx, "bar"));
    for _ in 0..3 {
        assert!(popup_frame(&ctx, vec![]).is_empty(), "nothing native before enable_native");
    }
    let inside = ctx.memory(|m| m.area_rect(egui::Id::new("bar").with(("menu", "File")))).unwrap();
    assert!(inside.min.y < 10.0, "pushed up to stay in the window as far as it can: {inside:?}");

    crate::popup::enable_native(&ctx);
    let mut n = popup_frame(&ctx, vec![]);
    for _ in 0..3 {
        n = popup_frame(&ctx, vec![]);
    }
    assert_eq!(n.len(), 1);
    let p = &n[0];
    assert!(p.req.grab && p.req.interactive && p.req.parent.is_none());
    assert!(p.rect.max.y > 100.0, "below the title, past the window: {:?}", p.rect);
    assert!(!p.shapes.is_empty());
    assert_eq!(p.rect, p.nominal);

    // The compositor moved it: egui follows.
    crate::popup::nudge(&ctx, p.req.id, vec2(0.0, -60.0));
    let moved = popup_frame(&ctx, vec![]);
    assert_eq!(moved[0].rect, p.rect.translate(vec2(0.0, -60.0)));
    assert_eq!(moved[0].nominal, p.rect);

    // Dismissed by the compositor: the menu closes.
    crate::popup::dismiss(&ctx, [p.req.id]);
    popup_frame(&ctx, vec![]);
    assert!(!MenuBar::is_open(&ctx, "bar"));
    assert!(popup_frame(&ctx, vec![]).is_empty());
}

#[test]
fn nested_popups_name_their_parent() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    crate::popup::enable_native(&ctx);
    let (a, b) = (egui::Id::new("a"), egui::Id::new("b"));
    let mut natives = Vec::new();
    for _ in 0..2 {
        let _ = ctx.run_ui(RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), ..Default::default() }, |ui| {
            crate::Popup::menu(a, Rect::from_min_size(pos2(10.0, 10.0), vec2(20.0, 20.0))).show(ui.ctx(), |ui| {
                let r = ui.label("item");
                crate::Popup::tooltip(b, r.rect, crate::popup::Side::Right, 4.0).show(ui.ctx(), |ui| ui.label("tip"));
            });
            natives = crate::popup::take_native(ui.ctx());
        });
    }
    let ids: Vec<_> = natives.iter().map(|n| (n.req.id, n.req.parent)).collect();
    let tip = egui::Tooltip::tooltip_id(b, 0);
    assert_eq!(ids, vec![(a, None), (tip, Some(a))], "parents first");
    assert!(!natives[1].req.interactive && !natives[1].req.grab);
}
