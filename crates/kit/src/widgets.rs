//! Small shared widgets. All colors and sizes come from [`Theme`]; each has hover, pressed,
//! disabled and keyboard-focus states. Flat: no borders, a fill per state; a focus ring
//! only while focused (egui gives focus by Tab, not by clicks). Buttons keep the arrow
//! cursor, as on the desktop.

use egui::{Align2, Color32, CornerRadius, Id, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::{Theme, Tip};

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

/// The focus ring around `rect` while `resp` has keyboard focus.
pub fn focus_ring(painter: &Painter, t: &Theme, resp: &Response, rect: Rect, radius: impl Into<CornerRadius>) {
    if resp.has_focus() {
        painter.rect_stroke(rect, radius, Stroke::new(t.size.focus_w, t.colors.focus), StrokeKind::Inside);
    }
}

/// Glyphs drawn with strokes, so they scale with their box and follow the theme's widths.
pub mod glyph {
    use egui::{Painter, Pos2, Stroke, Vec2, vec2};

    /// An arrow pointing `dir` (unit), `len` long, centered at `c`.
    pub fn arrow(p: &Painter, c: Pos2, dir: Vec2, len: f32, stroke: Stroke) {
        let (tail, tip) = (c - dir * len / 2.0, c + dir * len / 2.0);
        let n = vec2(-dir.y, dir.x);
        let head = len * 0.42;
        p.line_segment([tail, tip], stroke);
        p.line_segment([tip, tip - dir * head + n * head], stroke);
        p.line_segment([tip, tip - dir * head - n * head], stroke);
    }

    /// A × of half-size `h` centered at `c`.
    pub fn cross(p: &Painter, c: Pos2, h: f32, stroke: Stroke) {
        p.line_segment([c + vec2(-h, -h), c + vec2(h, h)], stroke);
        p.line_segment([c + vec2(h, -h), c + vec2(-h, h)], stroke);
    }
}

/// A toast: a floating bar at the bottom center of `clip`, for status with a few buttons.
/// Toasts sharing a `stack` id (e.g. the view's) stack up in call order each pass, the first
/// lowest, each above the real height of the ones below. So independent callers over the
/// same area just call in the order they want, bottom first.
pub fn toast(ui: &Ui, id: Id, stack: Id, clip: Rect, add: impl FnOnce(&mut Ui)) -> Response {
    let t = Theme::get(ui.ctx());
    let ctx = ui.ctx();
    let pass = ctx.cumulative_pass_nr();
    let key = stack.with("toast-stack");
    // (pass, bottom of the next toast)
    let bottom = match ctx.data(|d| d.get_temp::<(u64, f32)>(key)) {
        Some((p, y)) if p == pass => y,
        _ => clip.max.y - t.size.toast_bottom,
    };
    let pad = t.size.toast_pad;
    let r = egui::Area::new(id)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(clip.center().x, bottom))
        // egui sizes a new area at its default size for a pass, and that rect takes
        // presses in the next hit test: keep it small.
        .default_size(Vec2::ZERO)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(pad.x as i8, pad.y as i8)).corner_radius(t.radius.toast).show(ui, |ui| {
                ui.style_mut().text_styles.insert(egui::TextStyle::Body, t.font.toast.clone());
                ui.style_mut().text_styles.insert(egui::TextStyle::Button, t.font.toast.clone());
                ui.spacing_mut().item_spacing.x = t.space.lg;
                ui.horizontal(add);
            });
        })
        .response;
    // Above other middle-order areas (an editor's text field) even after they were clicked.
    ctx.move_to_top(r.layer_id);
    ctx.data_mut(|d| d.insert_temp(key, (pass, bottom - r.rect.height() - t.size.toast_gap)));
    r
}

/// A square toolbar button: flat, filled on hover, press and selection; `icon` paints into
/// the inner rect with the given color. The tooltip shows beside it (`Tip::side_tip`).
pub fn tool_button(ui: &mut Ui, selected: bool, tooltip: &str, icon: impl FnOnce(&Painter, Rect, Color32)) -> Response {
    let t = Theme::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(t.size.tool_button), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, t.radius.button, flat_fill(&t, &resp, selected));
    focus_ring(p, &t, &resp, rect, t.radius.button);
    let fg = if selected || resp.hovered() { t.colors.strong } else { t.colors.text };
    icon(p, rect.shrink(t.size.tool_icon_pad), fg);
    resp.side_tip(tooltip)
}

/// Relative luminance difference, 0..1.
fn contrast(a: Color32, b: Color32) -> f32 {
    let l = |c: Color32| (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0;
    (l(a) - l(b)).abs()
}

/// A palette swatch in a toolbar: a dot (with a faint ring only where it would vanish into
/// the panel, e.g. white on light), growing on hover, a ring around it when selected.
pub fn swatch(ui: &mut Ui, color: Color32, selected: bool, name: &str) -> Response {
    let t = Theme::get(ui.ctx());
    let s = &t.size;
    let (rect, resp) = ui.allocate_exact_size(vec2(s.tool_button, s.swatch_row_h), Sense::click());
    let p = ui.painter();
    let grow = resp.hovered() || resp.is_pointer_button_down_on() || resp.has_focus();
    let r = if grow && !selected { s.swatch_r + s.swatch_hover } else { s.swatch_r };
    p.circle_filled(rect.center(), r, color);
    if contrast(color, t.colors.panel) < 0.15 {
        p.circle_stroke(rect.center(), r, Stroke::new(s.line, t.colors.swatch_ring));
    }
    if selected {
        p.circle_stroke(rect.center(), s.swatch_r + s.swatch_ring_gap, Stroke::new(s.ring, t.colors.strong));
    } else if resp.has_focus() {
        p.circle_stroke(rect.center(), s.swatch_r + s.swatch_ring_gap, Stroke::new(s.focus_w, t.colors.focus));
    }
    resp.side_tip(name)
}

/// What a [`round_button`] shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundIcon {
    Up,
    Down,
}

/// A round icon button at `rect`. `accent`: the main action (accent fill); else a plain
/// floating button (popup fill and a shadow). Disabled: muted, no input.
pub fn round_button_at(ui: &mut Ui, id: Id, rect: Rect, icon: RoundIcon, accent: bool, enabled: bool) -> Response {
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    paint_round_button(ui, &resp, icon, accent, enabled);
    resp
}

/// [`round_button_at`], allocated in `ui`'s layout.
pub fn round_button(ui: &mut Ui, icon: RoundIcon, accent: bool, enabled: bool) -> Response {
    let t = Theme::get(ui.ctx());
    let (_, resp) = ui.allocate_exact_size(Vec2::splat(t.size.send_button), if enabled { Sense::click() } else { Sense::hover() });
    paint_round_button(ui, &resp, icon, accent, enabled);
    resp
}

fn paint_round_button(ui: &Ui, resp: &Response, icon: RoundIcon, accent: bool, enabled: bool) {
    let t = Theme::get(ui.ctx());
    let c = &t.colors;
    let rect = resp.rect;
    let down = resp.is_pointer_button_down_on();
    let (fill, fg) = match (enabled, accent) {
        (false, _) => (c.hover_fill, c.weak),
        (true, true) => (if down { c.accent_press } else if resp.hovered() { c.accent_hover } else { c.accent }, c.on_accent),
        (true, false) => (if down { c.press_fill } else if resp.hovered() { c.hover_fill } else { c.popup }, c.text),
    };
    let p = ui.painter();
    let r = rect.width() / 2.0;
    if !accent && enabled {
        p.add(t.shadow.float.as_shape(rect, r));
    }
    p.circle_filled(rect.center(), r, fill);
    if resp.has_focus() {
        p.circle_stroke(rect.center(), r - t.size.focus_w / 2.0, Stroke::new(t.size.focus_w, c.focus));
    }
    let dir = match icon {
        RoundIcon::Up => vec2(0.0, -1.0),
        RoundIcon::Down => vec2(0.0, 1.0),
    };
    glyph::arrow(p, rect.center(), dir, r * 0.9, Stroke::new(t.size.icon_bold, fg));
}

/// The send button: an accent round button with an up arrow. The caller adds tooltips.
pub fn send_button(ui: &mut Ui, enabled: bool) -> Response {
    round_button(ui, RoundIcon::Up, true, enabled)
}

/// Paints the round × badge centered at `center`; red while `hot`.
pub fn paint_close_badge(painter: &Painter, t: &Theme, center: Pos2, hot: bool, opacity: f32) {
    let c = &t.colors;
    let r = t.size.close_badge / 2.0;
    painter.circle_filled(center, r, if hot { c.danger } else { c.badge }.gamma_multiply(opacity));
    glyph::cross(painter, center, r * 0.44, Stroke::new(t.size.icon_stroke, c.badge_fg.gamma_multiply(opacity)));
}

/// The round × badge (remove) centered at `center`; red on hover.
pub fn close_badge(ui: &mut Ui, id: Id, center: Pos2, tooltip: &str) -> Response {
    let painter = ui.painter().clone();
    close_badge_on(ui, &painter, id, center, tooltip)
}

/// [`close_badge`] painted with `painter` (e.g. on a layer above `ui`); input stays `ui`'s.
pub fn close_badge_on(ui: &mut Ui, painter: &Painter, id: Id, center: Pos2, tooltip: &str) -> Response {
    let t = Theme::get(ui.ctx());
    let rect = Rect::from_center_size(center, Vec2::splat(t.size.close_badge));
    let resp = ui.interact(rect, id, Sense::click());
    paint_close_badge(painter, &t, center, resp.hovered() || resp.has_focus(), 1.0);
    resp.tip(tooltip)
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
    let corners = CornerRadius { nw: r, ne: r, sw: 0, se: 0 };
    let p = ui.painter();
    p.rect_filled(rect, corners, fill);
    focus_ring(p, &t, &resp, rect, corners);
    if selected {
        let line = t.size.tab_line;
        let top = Rect::from_min_size(rect.min + vec2(t.radius.tab, 0.0), vec2(rect.width() - 2.0 * t.radius.tab, line));
        p.rect_filled(top, line / 2.0, c.accent);
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

/// The frame of a text box (message box): the field fill, a focus ring while focused (kept
/// transparent otherwise, so focus doesn't shift the layout).
pub fn field_frame(t: &Theme, focused: bool) -> egui::Frame {
    let c = &t.colors;
    egui::Frame::new()
        .fill(c.field)
        .stroke(Stroke::new(t.size.focus_w, if focused { c.focus } else { Color32::TRANSPARENT }))
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

impl WindowButton {
    pub fn tooltip(self) -> &'static str {
        match self {
            WindowButton::Minimize => "Minimize",
            WindowButton::Maximize => "Maximize",
            WindowButton::Restore => "Restore",
            WindowButton::Close => "Close",
        }
    }
}

/// A title bar button filling `rect`. Close turns red on hover.
pub fn window_button(ui: &mut Ui, id: Id, rect: Rect, kind: WindowButton) -> Response {
    let t = Theme::get(ui.ctx());
    let c = &t.colors;
    let resp = ui.interact(rect, id, Sense::click());
    let close = kind == WindowButton::Close;
    let hot = resp.hovered() || resp.is_pointer_button_down_on();
    let fill = if close && hot { c.danger } else { flat_fill(&t, &resp, false) };
    let p = ui.painter();
    p.rect_filled(rect, 0.0, fill);
    focus_ring(p, &t, &resp, rect, 0.0);
    let fg = if close && hot { c.on_accent } else { c.text };
    let stroke = Stroke::new(t.size.line, fg);
    let center = rect.center();
    let h = t.size.window_glyph;
    match kind {
        WindowButton::Minimize => {
            p.line_segment([center + vec2(-h, h * 0.6), center + vec2(h, h * 0.6)], stroke);
        }
        WindowButton::Restore => {
            // A front square down-left, the back one's top and right edges peeking out.
            let o = h * 0.3;
            p.rect_stroke(Rect::from_center_size(center + vec2(-o, o), Vec2::splat(2.0 * (h - o))), 0.0, stroke, StrokeKind::Middle);
            p.line_segment([center + vec2(-h + 2.0 * o, -h), center + vec2(h, -h)], stroke);
            p.line_segment([center + vec2(h, -h), center + vec2(h, h - 2.0 * o)], stroke);
        }
        WindowButton::Maximize => {
            p.rect_stroke(Rect::from_center_size(center, Vec2::splat(2.0 * h)), 0.0, stroke, StrokeKind::Middle);
        }
        WindowButton::Close => glyph::cross(p, center, h, stroke),
    }
    resp.tip(kind.tooltip())
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
    painter.add(t.shadow.popup.as_shape(r, t.radius.button));
    painter.rect_filled(r, t.radius.button, c.popup);
    painter.galley(r.min + pad, galley, c.strong);
}
