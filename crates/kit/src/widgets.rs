//! Small shared widgets. All colors and sizes come from [`Theme`]; each has hover, pressed
//! and disabled states and the right cursor.

use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, Painter, Pos2, Rect, Response, Sense, Stroke, TextureId, Ui, Vec2, pos2, vec2};

use crate::Theme;

/// Consumes a press of Enter with no modifiers. Unlike `consume_key`, which ignores Shift,
/// this leaves Shift+Enter for text boxes to turn into a newline.
pub fn consume_plain_enter(i: &mut egui::InputState) -> bool {
    let before = i.events.len();
    i.events.retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.matches_exact(egui::Modifiers::NONE)));
    i.events.len() != before
}

/// Fill of a flat widget (toolbar button, title bar button, tab) for its state.
pub fn flat_fill(t: &Theme, resp: &Response, selected: bool) -> Color32 {
    let c = &t.colors;
    if resp.is_pointer_button_down_on() {
        c.press_fill
    } else if selected {
        c.selected_fill
    } else if resp.hovered() {
        c.hover_fill
    } else {
        Color32::TRANSPARENT
    }
}

/// A toast: a floating bar at the bottom center of `clip`, for status with a few buttons.
/// `slot` 0 is lowest; higher slots stack above it.
pub fn toast(ui: &Ui, id: Id, clip: Rect, slot: usize, add: impl FnOnce(&mut Ui)) -> Response {
    let t = Theme::get(ui.ctx());
    let y = clip.max.y - t.size.toast_bottom - slot as f32 * t.size.toast_slot;
    egui::Area::new(id)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(clip.center().x, y))
        // egui sizes a new area at its default size for a pass, and that rect takes
        // presses in the next hit test: keep it small.
        .default_size(Vec2::ZERO)
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(t.space.xl as i8, (t.space.md + 2.0) as i8)).corner_radius(t.radius.toast).show(ui, |ui| {
                ui.style_mut().text_styles.insert(egui::TextStyle::Body, t.font.toast.clone());
                ui.style_mut().text_styles.insert(egui::TextStyle::Button, t.font.toast.clone());
                ui.spacing_mut().button_padding = vec2(t.space.md + 2.0, t.space.sm);
                ui.spacing_mut().item_spacing.x = t.space.lg;
                ui.horizontal(add);
            });
        })
        .response
}

/// A square toolbar button: flat, filled on hover, press and selection; `icon` paints into
/// the inner rect with the given color.
pub fn tool_button(ui: &mut Ui, selected: bool, tooltip: &str, icon: impl FnOnce(&Painter, Rect, Color32)) -> Response {
    let t = Theme::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(t.size.tool_button), Sense::click());
    let fill = flat_fill(&t, &resp, selected);
    ui.painter().rect_filled(rect, t.radius.button, fill);
    let fg = if selected || resp.hovered() { t.colors.strong } else { t.colors.text };
    icon(ui.painter(), rect.shrink(t.size.tool_icon_pad), fg);
    resp.on_hover_text(tooltip)
}

/// A palette swatch in a toolbar: a dot with a thin ring, a bigger ring when selected.
pub fn swatch(ui: &mut Ui, color: Color32, selected: bool, name: &str) -> Response {
    let t = Theme::get(ui.ctx());
    let s = &t.size;
    let (rect, resp) = ui.allocate_exact_size(vec2(s.tool_button, s.swatch_row_h), Sense::click());
    let p = ui.painter();
    let r = if resp.hovered() && !selected { s.swatch_r + 1.0 } else { s.swatch_r };
    p.circle_filled(rect.center(), r, color);
    p.circle_stroke(rect.center(), r, Stroke::new(s.line, t.colors.swatch_ring));
    if selected {
        p.circle_stroke(rect.center(), s.swatch_r + 3.0, Stroke::new(2.0, t.colors.strong));
    }
    resp.on_hover_text(name)
}

/// A round button with an up arrow: accent-filled when there's something to send.
pub fn send_button(ui: &mut Ui, enabled: bool) -> Response {
    let t = Theme::get(ui.ctx());
    let c = &t.colors;
    let size = t.size.send_button;
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    let (fill, fg) = if enabled {
        let fill = if resp.is_pointer_button_down_on() {
            c.accent
        } else if resp.hovered() {
            c.accent_hover
        } else {
            c.accent
        };
        (fill, c.on_accent)
    } else {
        (c.hover_fill, c.weak)
    };
    let p = ui.painter();
    let center = rect.center();
    let r = size / 2.0;
    p.circle_filled(center, r, fill);
    let stroke = Stroke::new(2.0, fg);
    let (top, bottom) = (center - vec2(0.0, r * 0.45), center + vec2(0.0, r * 0.45));
    p.line_segment([bottom, top], stroke);
    p.line_segment([top, top + vec2(-r * 0.38, r * 0.38)], stroke);
    p.line_segment([top, top + vec2(r * 0.38, r * 0.38)], stroke);
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    resp.on_hover_text(if enabled { "Send (Enter)" } else { "Type a message or draw something to send" })
}

/// The round × badge (remove) centered at `center`; red on hover.
pub fn close_badge(ui: &mut Ui, id: Id, center: Pos2, tooltip: &str) -> Response {
    let painter = ui.painter().clone();
    close_badge_on(ui, &painter, id, center, tooltip)
}

/// [`close_badge`] painted with `painter` (e.g. on a layer above `ui`); input stays `ui`'s.
pub fn close_badge_on(ui: &mut Ui, painter: &Painter, id: Id, center: Pos2, tooltip: &str) -> Response {
    let t = Theme::get(ui.ctx());
    let c = &t.colors;
    let rect = Rect::from_center_size(center, Vec2::splat(t.size.close_badge));
    let resp = ui.interact(rect, id, Sense::click());
    painter.circle_filled(center, rect.width() / 2.0, if resp.hovered() { c.danger } else { c.badge });
    let s = Stroke::new(1.5, c.badge_fg);
    let h = rect.width() * 0.22;
    painter.line_segment([center + vec2(-h, -h), center + vec2(h, h)], s);
    painter.line_segment([center + vec2(h, -h), center + vec2(-h, h)], s);
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tooltip)
}

/// A tab: rounded top corners, the selected one filled with `selected_fill` (the color of
/// what it shows, so it reads as attached) and an accent line on top.
pub fn tab(ui: &mut Ui, selected: bool, label: &str, selected_fill: Color32) -> Response {
    let t = Theme::get(ui.ctx());
    let c = &t.colors;
    let galley = ui.painter().layout_no_wrap(label.to_string(), t.font.body.clone(), c.text);
    let size = vec2((galley.size().x + 2.0 * t.size.tab_pad).max(t.size.tab_min_w), t.size.tab_h);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected { selected_fill } else { flat_fill(&t, &resp, false) };
    let r = t.radius.tab as u8;
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, fill);
    if selected {
        let top = Rect::from_min_size(rect.min + vec2(t.radius.tab, 0.0), vec2(rect.width() - 2.0 * t.radius.tab, 2.0));
        p.rect_filled(top, 1.0, c.accent);
    }
    let color = if selected {
        c.strong
    } else if resp.hovered() {
        c.text
    } else {
        c.weak
    };
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// A thumbnail: the image in `rect` with a thin outline, lit on hover.
pub fn thumb_frame(ui: &mut Ui, texture: TextureId, size: Vec2, sense: Sense) -> Response {
    let t = Theme::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let p = ui.painter();
    p.image(texture, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    let color = if resp.hovered() { t.colors.outline_focus } else { t.colors.outline };
    p.rect_stroke(rect, t.radius.small, Stroke::new(t.size.line, color), egui::StrokeKind::Outside);
    resp
}

/// Shows `texture` (natural size `size`) at most `preview_max` big, e.g. in a hover tooltip.
pub fn preview(ui: &mut Ui, texture: TextureId, size: Vec2) {
    let max = Theme::get(ui.ctx()).size.preview_max;
    let k = (max.x / size.x).min(max.y / size.y).min(1.0);
    ui.image((texture, size * k));
}

/// The frame of a text box (message box): field fill, outline brighter while focused.
pub fn field_frame(t: &Theme, focused: bool) -> egui::Frame {
    let c = &t.colors;
    egui::Frame::new()
        .fill(c.field)
        .stroke(Stroke::new(t.size.line, if focused { c.outline_focus } else { c.outline }))
        .corner_radius(t.radius.field)
}

/// Title bar buttons (client-side decorations).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowButton {
    Minimize,
    Maximize,
    /// Maximize while maximized: two overlapping squares.
    Restore,
    Close,
}

/// A title bar button filling `rect`. Close turns red on hover.
pub fn window_button(ui: &mut Ui, id: Id, rect: Rect, kind: WindowButton) -> Response {
    let t = Theme::get(ui.ctx());
    let c = &t.colors;
    let resp = ui.interact(rect, id, Sense::click());
    let close = kind == WindowButton::Close;
    let fill = if close && resp.hovered() { c.danger } else { flat_fill(&t, &resp, false) };
    let p = ui.painter();
    p.rect_filled(rect, 0.0, fill);
    let fg = if close && resp.hovered() { c.on_accent } else { c.text };
    let stroke = Stroke::new(t.size.line, fg);
    let center = rect.center();
    let h = 5.0;
    match kind {
        WindowButton::Minimize => {
            p.line_segment([center + vec2(-h, h * 0.6), center + vec2(h, h * 0.6)], stroke);
        }
        WindowButton::Restore => {
            p.rect_stroke(Rect::from_center_size(center + vec2(-1.5, 1.5), vec2(2.0 * h - 2.0, 2.0 * h - 2.0)), 0.0, stroke, egui::StrokeKind::Middle);
            p.line_segment([center + vec2(-h + 2.5, -h), center + vec2(h, -h)], stroke);
            p.line_segment([center + vec2(h, -h), center + vec2(h, h - 2.5)], stroke);
        }
        WindowButton::Maximize => {
            p.rect_stroke(Rect::from_center_size(center, vec2(2.0 * h, 2.0 * h)), 0.0, stroke, egui::StrokeKind::Middle);
        }
        WindowButton::Close => {
            p.line_segment([center + vec2(-h, -h), center + vec2(h, h)], stroke);
            p.line_segment([center + vec2(-h, h), center + vec2(h, -h)], stroke);
        }
    }
    let tip = match kind {
        WindowButton::Minimize => "Minimize",
        WindowButton::Maximize => "Maximize",
        WindowButton::Restore => "Restore",
        WindowButton::Close => "Close",
    };
    resp.on_hover_text(tip)
}

/// A small label floating next to the pointer `at` (e.g. a size readout while dragging),
/// flipped to the pointer's other side where it would leave `bounds`.
pub fn pointer_label(ctx: &egui::Context, id: Id, bounds: Rect, at: Pos2, text: String) {
    let t = Theme::get(ctx);
    let c = &t.colors;
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, id));
    let galley = painter.layout_no_wrap(text, t.font.shortcut.clone(), c.strong);
    let pad = vec2(t.space.md, t.space.sm);
    let box_size = galley.size() + 2.0 * pad;
    let off = t.space.xl;
    let mut min = at + Vec2::splat(off);
    if min.x + box_size.x > bounds.max.x {
        min.x = at.x - off - box_size.x;
    }
    if min.y + box_size.y > bounds.max.y {
        min.y = at.y - off - box_size.y;
    }
    let r = Rect::from_min_size(min, box_size);
    painter.rect(r, t.radius.button, c.popup, Stroke::new(t.size.line, c.popup_stroke), egui::StrokeKind::Inside);
    painter.galley(r.min + pad, galley, c.strong);
}
