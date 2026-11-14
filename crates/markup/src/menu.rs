//! A desktop-style menu bar for egui: flat top-level items edge to edge (no gaps or inset
//! rounding), a faint fill on hover and a stronger one while open, and the usual behavior:
//! click opens, hovering another item while one is open switches to it, clicking an item,
//! outside, the open title again, or Esc closes. Menus hold full-width flat items
//! ([`MenuUi::item`], [`MenuUi::radio`]), separators, headings and custom rows.
//!
//! Use it for every menu bar, so they all look and behave the same.

use egui::{Align2, Color32, FontId, Id, Order, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

const BAR_H: f32 = 24.0;
const TITLE_PAD: f32 = 10.0;
const ITEM_H: f32 = 26.0;
const ITEM_PAD: f32 = 12.0;
const MIN_W: f32 = 200.0;

#[derive(Clone, Default)]
struct State {
    /// The open menu's title.
    open: Option<String>,
    /// Width of the open menu: its widest row as of last frame.
    width: f32,
}

pub struct MenuBar {
    id: Id,
}

/// The bar while it's being filled: add menus with [`Bar::menu`].
pub struct Bar<'a> {
    ui: &'a mut Ui,
    id: Id,
    state: State,
    /// Screen areas that count as "inside" (titles and the open popup).
    inside: Vec<Rect>,
    /// Right end of the last title.
    end: f32,
}

impl MenuBar {
    pub fn new(id: impl Into<Id>) -> MenuBar {
        MenuBar { id: id.into() }
    }

    /// Whether one of this bar's menus is open (then keys belong to the UI, e.g. Esc).
    pub fn is_open(ctx: &egui::Context, id: impl Into<Id>) -> bool {
        ctx.data(|d| d.get_temp::<State>(id.into())).is_some_and(|s| s.open.is_some())
    }

    /// Lays out a full-width bar in `ui` (e.g. a top panel with no margins). Returns the
    /// bar's free part right of the titles (e.g. for a window title or controls).
    pub fn show(self, ui: &mut Ui, add: impl FnOnce(&mut Bar)) -> Rect {
        let state = ui.data(|d| d.get_temp::<State>(self.id)).unwrap_or_default();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.spacing_mut().item_spacing = Vec2::ZERO;
        let mut bar = Bar { ui: &mut child, id: self.id, state, inside: Vec::new(), end: rect.min.x };
        add(&mut bar);
        let Bar { mut state, inside, end, .. } = bar;
        if state.open.is_some() {
            let (pressed, pos, esc) = ui.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos(), i.key_pressed(egui::Key::Escape)));
            if esc || (pressed && pos.is_some_and(|p| !inside.iter().any(|r| r.contains(p)))) {
                state.open = None;
            }
        }
        ui.data_mut(|d| d.insert_temp(self.id, state));
        Rect::from_min_max(pos2(end, rect.min.y), rect.max)
    }
}

impl Bar<'_> {
    /// A top-level menu. `add` fills it while it's open.
    pub fn menu(&mut self, title: &str, add: impl FnOnce(&mut MenuUi)) {
        self.menu_enabled(title, true, add);
    }

    pub fn menu_enabled(&mut self, title: &str, enabled: bool, add: impl FnOnce(&mut MenuUi)) {
        let ui = &mut *self.ui;
        let v = ui.visuals().clone();
        let galley = ui.painter().layout_no_wrap(title.to_string(), FontId::proportional(14.0), v.text_color());
        let size = vec2(galley.size().x + 2.0 * TITLE_PAD, BAR_H);
        let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
        self.inside.push(rect);
        self.end = rect.max.x;
        let is_open = self.state.open.as_deref() == Some(title);
        if enabled {
            if resp.clicked() {
                self.state.open = if is_open { None } else { Some(title.to_string()) };
                self.state.width = 0.0;
            } else if resp.hovered() && self.state.open.is_some() && !is_open {
                self.state.open = Some(title.to_string());
                self.state.width = 0.0;
            }
        }
        let is_open = self.state.open.as_deref() == Some(title);
        let fill = if is_open {
            v.widgets.active.weak_bg_fill
        } else if resp.hovered() && enabled {
            v.widgets.hovered.weak_bg_fill.gamma_multiply(0.5)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 0.0, fill);
        let color = if enabled { v.text_color() } else { v.weak_text_color() };
        ui.painter().galley_with_override_text_color(pos2(rect.min.x + TITLE_PAD, rect.center().y - galley.size().y / 2.0), galley, color);
        if !is_open {
            return;
        }
        let ctx = ui.ctx().clone();
        let mut close = false;
        let width = self.state.width.max(MIN_W);
        let mut natural = MIN_W;
        let area = egui::Area::new(self.id.with(("menu", title))).order(Order::Foreground).fixed_pos(rect.left_bottom()).constrain(true).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(0, 4)).corner_radius(4.0).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let mut m = MenuUi { ui, close: false, width, natural: MIN_W };
                add(&mut m);
                close = m.close;
                natural = m.natural;
            });
        });
        self.inside.push(area.response.rect);
        if natural != self.state.width {
            self.state.width = natural;
            ctx.request_repaint();
        }
        if close {
            self.state.open = None;
        }
    }
}

#[derive(Clone, Copy)]
enum Mark {
    Radio(bool),
    Check(bool),
}

/// The inside of an open menu.
pub struct MenuUi<'a> {
    pub ui: &'a mut Ui,
    close: bool,
    /// Row width, and the widest row's natural width (next frame's width).
    width: f32,
    natural: f32,
}

impl MenuUi<'_> {
    /// Closes the menu after this frame.
    pub fn close(&mut self) {
        self.close = true;
    }

    /// A full-width row: hover fill edge to edge, label left, `shortcut` right. Clicking
    /// closes the menu. Returns whether it was clicked.
    pub fn item(&mut self, label: &str, shortcut: Option<&str>, enabled: bool) -> bool {
        self.row(label, shortcut, None, enabled)
    }

    /// An item with a radio mark.
    pub fn radio(&mut self, selected: bool, label: &str) -> bool {
        self.row(label, None, Some(Mark::Radio(selected)), true)
    }

    /// An item with a check box. Returns whether it was clicked (the caller toggles).
    pub fn check(&mut self, checked: bool, label: &str, enabled: bool) -> bool {
        self.row(label, None, Some(Mark::Check(checked)), enabled)
    }

    /// A dim, non-interactive line of text (e.g. why items are disabled).
    pub fn note(&mut self, text: &str) {
        let ui = &mut *self.ui;
        let font = FontId::proportional(12.0);
        let w = ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x;
        self.natural = self.natural.max(2.0 * ITEM_PAD + w);
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, 20.0), Sense::hover());
        ui.painter().text(pos2(rect.min.x + ITEM_PAD, rect.center().y), Align2::LEFT_CENTER, text, font, ui.visuals().weak_text_color());
    }

    fn row(&mut self, label: &str, shortcut: Option<&str>, mark: Option<Mark>, enabled: bool) -> bool {
        let ui = &mut *self.ui;
        let v = ui.visuals().clone();
        let font = FontId::proportional(14.0);
        let label_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE).size().x;
        let short_w = shortcut.map_or(0.0, |s| ui.painter().layout_no_wrap(s.to_string(), FontId::proportional(13.0), Color32::WHITE).size().x + 32.0);
        self.natural = self.natural.max(2.0 * ITEM_PAD + if mark.is_some() { 20.0 } else { 0.0 } + label_w + short_w);
        let (rect, resp) = ui.allocate_exact_size(vec2(self.width, ITEM_H), if enabled { Sense::click() } else { Sense::hover() });
        if enabled && resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, v.widgets.hovered.weak_bg_fill);
        }
        let color = if enabled { v.text_color() } else { v.weak_text_color() };
        let mut x = rect.min.x + ITEM_PAD;
        if let Some(mark) = mark {
            let c = pos2(x + 6.0, rect.center().y);
            match mark {
                Mark::Radio(on) => {
                    ui.painter().circle_stroke(c, 6.0, Stroke::new(1.0, v.weak_text_color()));
                    if on {
                        ui.painter().circle_filled(c, 3.5, color);
                    }
                }
                Mark::Check(on) => {
                    let r = Rect::from_center_size(c, vec2(12.0, 12.0));
                    ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, v.weak_text_color()), egui::StrokeKind::Inside);
                    if on {
                        let pts = vec![c + vec2(-3.5, 0.0), c + vec2(-1.0, 2.8), c + vec2(3.8, -3.0)];
                        ui.painter().add(egui::Shape::line(pts, Stroke::new(1.8, color)));
                    }
                }
            }
            x += 20.0;
        }
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, color);
        if let Some(s) = shortcut {
            ui.painter().text(pos2(rect.max.x - ITEM_PAD, rect.center().y), Align2::RIGHT_CENTER, s, FontId::proportional(13.0), v.weak_text_color());
        }
        let clicked = enabled && resp.clicked();
        if clicked {
            self.close = true;
        }
        clicked
    }

    pub fn separator(&mut self) {
        let ui = &mut *self.ui;
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, 9.0), Sense::hover());
        ui.painter().hline(rect.x_range(), rect.center().y, ui.visuals().widgets.noninteractive.bg_stroke);
    }

    /// A small dim label above a group of items.
    pub fn heading(&mut self, text: &str) {
        let ui = &mut *self.ui;
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, 20.0), Sense::hover());
        ui.painter().text(pos2(rect.min.x + ITEM_PAD, rect.center().y), Align2::LEFT_CENTER, text, FontId::proportional(12.0), ui.visuals().weak_text_color());
    }

    /// A custom row: `left` (e.g. a label) and `right` (e.g. buttons, laid out right to
    /// left), padded like items. Its widgets don't close the menu.
    pub fn custom(&mut self, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
        let ui = &mut *self.ui;
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, ITEM_H + 4.0), Sense::hover());
        let inner = rect.shrink2(vec2(ITEM_PAD, 2.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.spacing_mut().item_spacing = vec2(6.0, 0.0);
        left(&mut child);
        let used = child.min_rect().width();
        let right_w = child.with_layout(egui::Layout::right_to_left(egui::Align::Center), right).response.rect.width();
        self.natural = self.natural.max(2.0 * ITEM_PAD + used + right_w + 24.0);
    }
}

