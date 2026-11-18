//! A desktop-style menu bar: flat titles edge to edge, a faint fill on hover and a stronger
//! one while open. Behaves like GTK/Qt menus:
//! - a press on a title opens its menu (press, drag to an item and release works too);
//!   hovering another title while one is open switches to it;
//! - releasing over an item activates it; a press outside, on the open title, or Esc closes;
//! - keys: Up/Down (Home/End) move the highlight over enabled items, Enter/Space activate,
//!   Left/Right switch menus.
//!
//! While a menu is open the host should route keys to egui ([`MenuBar::is_open`]) and
//! ignore presses outside it ([`MenuBar::is_open`] checked before showing the bar).
//! Menus hold items ([`MenuUi::item`], [`MenuUi::radio`], [`MenuUi::check`]), gaps between
//! groups, headings, notes and custom rows. Flat: the popup is set off by its shadow, groups
//! by space.

use egui::{Align2, Color32, Id, Key, Modifiers, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::Theme;

#[derive(Clone, Default)]
struct State {
    /// The open menu's title.
    open: Option<String>,
    /// Width of the open menu: its widest row as of last frame.
    width: f32,
    /// The highlighted item (index among items), by hover or keys.
    hot: Option<usize>,
    /// Items of the open menu last frame: enabled or not.
    items: Vec<bool>,
    /// Titles last frame: enabled or not.
    titles: Vec<(String, bool)>,
}

/// What the keyboard asked for this frame.
#[derive(Clone, Copy, Default)]
struct Keys {
    activate: Option<usize>,
}

pub struct MenuBar {
    id: Id,
}

/// The bar while it's being filled: add menus with [`Bar::menu`].
pub struct Bar<'a> {
    ui: &'a mut Ui,
    id: Id,
    state: State,
    keys: Keys,
    titles: Vec<(String, bool)>,
    /// Screen areas that count as "inside" (titles and the open popup).
    inside: Vec<Rect>,
    /// Right end of the last title.
    end: f32,
}

fn step(items: &[bool], from: Option<usize>, dir: i32) -> Option<usize> {
    let n = items.len() as i32;
    if n == 0 || !items.iter().any(|e| *e) {
        return None;
    }
    let mut i = from.map_or(if dir > 0 { -1 } else { n }, |i| i as i32);
    for _ in 0..n {
        i = (i + dir).rem_euclid(n);
        if items[i as usize] {
            return Some(i as usize);
        }
    }
    None
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
        let t = Theme::get(ui.ctx());
        let mut state = ui.data(|d| d.get_temp::<State>(self.id)).unwrap_or_default();
        let keys = if state.open.is_some() { Self::keys(ui, &mut state) } else { Keys::default() };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), t.size.menu_bar_h), Sense::hover());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.spacing_mut().item_spacing = Vec2::ZERO;
        let mut bar = Bar { ui: &mut child, id: self.id, state, keys, titles: Vec::new(), inside: Vec::new(), end: rect.min.x };
        add(&mut bar);
        let Bar { mut state, inside, end, titles, .. } = bar;
        state.titles = titles;
        if state.open.is_some() {
            let (pressed, pos) = ui.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos()));
            if pressed && pos.is_some_and(|p| !inside.iter().any(|r| r.contains(p))) {
                state.open = None;
            }
        }
        if state.open.is_none() {
            state.hot = None;
            state.items.clear();
        }
        ui.data_mut(|d| d.insert_temp(self.id, state));
        Rect::from_min_max(pos2(end, rect.min.y), rect.max)
    }

    /// Takes the open menu's navigation keys.
    fn keys(ui: &Ui, state: &mut State) -> Keys {
        let mut keys = Keys::default();
        // A focused field in a custom row (e.g. a number being typed) keeps its keys.
        if ui.memory(|m| m.focused().is_some()) {
            return keys;
        }
        // Taken in order: "End, Up" must land on the second-to-last item.
        const NAV: [Key; 9] = [Key::Escape, Key::ArrowDown, Key::ArrowUp, Key::Home, Key::End, Key::ArrowRight, Key::ArrowLeft, Key::Enter, Key::Space];
        let pressed: Vec<Key> = ui.input_mut(|i| {
            let mut out = Vec::new();
            i.events.retain(|e| match e {
                egui::Event::Key { key, pressed, modifiers, .. } if NAV.contains(key) && modifiers.matches_exact(Modifiers::NONE) => {
                    if *pressed {
                        out.push(*key);
                    }
                    false
                }
                _ => true,
            });
            out
        });
        for k in pressed {
            match k {
                Key::Escape => {
                    state.open = None;
                    break;
                }
                Key::ArrowDown => state.hot = step(&state.items, state.hot, 1),
                Key::ArrowUp => state.hot = step(&state.items, state.hot, -1),
                Key::Home => state.hot = step(&state.items, None, 1),
                Key::End => state.hot = step(&state.items, None, -1),
                Key::ArrowRight | Key::ArrowLeft => {
                    let dir = if k == Key::ArrowRight { 1 } else { -1 };
                    let enabled: Vec<bool> = state.titles.iter().map(|t| t.1).collect();
                    let cur = state.titles.iter().position(|t| Some(&t.0) == state.open.as_ref());
                    if let Some(i) = step(&enabled, cur, dir) {
                        state.open = Some(state.titles[i].0.clone());
                        state.width = 0.0;
                        state.items.clear();
                        state.hot = None;
                    }
                    // The new menu's items are known next frame.
                    break;
                }
                _ => {
                    keys.activate = state.hot;
                    break;
                }
            }
        }
        keys
    }
}

impl Bar<'_> {
    /// A top-level menu. `add` fills it while it's open.
    pub fn menu(&mut self, title: &str, add: impl FnOnce(&mut MenuUi)) {
        self.menu_enabled(title, true, add);
    }

    pub fn menu_enabled(&mut self, title: &str, enabled: bool, add: impl FnOnce(&mut MenuUi)) {
        let ui = &mut *self.ui;
        let t = Theme::get(ui.ctx());
        let c = &t.colors;
        self.titles.push((title.to_string(), enabled));
        let galley = ui.painter().layout_no_wrap(title.to_string(), t.font.body.clone(), c.text);
        let size = vec2(galley.size().x + 2.0 * t.size.menu_title_pad, t.size.menu_bar_h);
        let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
        self.inside.push(rect);
        self.end = rect.max.x;
        let mut is_open = self.state.open.as_deref() == Some(title);
        if enabled {
            let pressed = resp.hovered() && ui.input(|i| i.pointer.primary_pressed());
            // Only when the pointer moves: Left/Right may have switched away from it.
            let moved = ui.input(|i| i.pointer.delta() != Vec2::ZERO);
            let switch = resp.hovered() && moved && self.state.open.is_some() && !is_open;
            if pressed || switch {
                self.state.open = if is_open { None } else { Some(title.to_string()) };
                self.state.width = 0.0;
                self.state.hot = None;
                self.state.items.clear();
            }
        }
        is_open = self.state.open.as_deref() == Some(title);
        let fill = if is_open {
            c.press_fill
        } else if resp.hovered() && enabled {
            c.hover_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 0.0, fill);
        let color = if enabled { c.text } else { c.weak };
        ui.painter().galley_with_override_text_color(pos2(rect.min.x + t.size.menu_title_pad, rect.center().y - galley.size().y / 2.0), galley, color);
        if !is_open {
            return;
        }
        let ctx = ui.ctx().clone();
        let min_w = t.size.menu_min_w;
        let width = self.state.width.max(min_w);
        let (hot, activate) = (self.state.hot, self.keys.activate);
        let mut out = None;
        let popup = crate::Popup::menu(self.id.with(("menu", title)), rect).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(0, t.space.sm as i8)).corner_radius(t.radius.menu).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let mut m = MenuUi { ui, theme: t.clone(), close: false, width, natural: min_w, hot, activate, items: Vec::new(), hovered: None };
                add(&mut m);
                out = Some((m.close, m.natural, m.items, m.hovered));
            });
        });
        self.inside.push(popup.response.rect);
        if popup.dismissed {
            self.state.open = None;
            return;
        }
        let Some((close, natural, items, hovered)) = out else { return };
        if natural != self.state.width {
            self.state.width = natural;
            ctx.request_repaint();
        }
        // The pointer moving takes the highlight (over an item, or off them all).
        let moved = ctx.input(|i| i.pointer.delta() != Vec2::ZERO);
        if moved {
            self.state.hot = hovered;
        }
        if items != self.state.items {
            self.state.items = items;
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
    theme: std::sync::Arc<Theme>,
    close: bool,
    /// Row width, and the widest row's natural width (next frame's width).
    width: f32,
    natural: f32,
    hot: Option<usize>,
    activate: Option<usize>,
    /// Items so far: enabled or not.
    items: Vec<bool>,
    hovered: Option<usize>,
}

impl MenuUi<'_> {
    /// Closes the menu after this frame.
    pub fn close(&mut self) {
        self.close = true;
    }

    /// A full-width row: hover fill edge to edge, label left, `shortcut` right. Activating
    /// it closes the menu. Returns whether it was activated.
    pub fn item(&mut self, label: &str, shortcut: Option<&str>, enabled: bool) -> bool {
        self.row(label, shortcut, None, enabled)
    }

    /// An item with a radio mark.
    pub fn radio(&mut self, selected: bool, label: &str) -> bool {
        self.row(label, None, Some(Mark::Radio(selected)), true)
    }

    /// An item with a check box. Returns whether it was activated (the caller toggles).
    pub fn check(&mut self, checked: bool, label: &str, enabled: bool) -> bool {
        self.row(label, None, Some(Mark::Check(checked)), enabled)
    }

    /// A dim, non-interactive line of text (e.g. why items are disabled).
    pub fn note(&mut self, text: &str) {
        let t = self.theme.clone();
        let ui = &mut *self.ui;
        let w = ui.painter().layout_no_wrap(text.to_string(), t.font.small.clone(), t.colors.weak).size().x;
        self.natural = self.natural.max(2.0 * t.size.menu_item_pad + w);
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, t.size.menu_heading_h), Sense::hover());
        ui.painter().text(pos2(rect.min.x + t.size.menu_item_pad, rect.center().y), Align2::LEFT_CENTER, text, t.font.small.clone(), t.colors.weak);
    }

    fn row(&mut self, label: &str, shortcut: Option<&str>, mark: Option<Mark>, enabled: bool) -> bool {
        let t = self.theme.clone();
        let (c, s) = (&t.colors, &t.size);
        let index = self.items.len();
        self.items.push(enabled);
        let ui = &mut *self.ui;
        let label_w = ui.painter().layout_no_wrap(label.to_string(), t.font.body.clone(), c.text).size().x;
        let short_w = shortcut.map_or(0.0, |sc| ui.painter().layout_no_wrap(sc.to_string(), t.font.shortcut.clone(), c.weak).size().x + s.menu_shortcut_gap);
        let mark_w = if mark.is_some() { s.menu_mark_w } else { 0.0 };
        self.natural = self.natural.max(2.0 * s.menu_item_pad + mark_w + label_w + short_w);
        let (rect, resp) = ui.allocate_exact_size(vec2(self.width, s.menu_item_h), if enabled { Sense::click() } else { Sense::hover() });
        if resp.hovered() {
            self.hovered = enabled.then_some(index);
        }
        let hot = enabled && (self.hot == Some(index) || (resp.hovered() && self.hot.is_none()));
        if hot {
            let fill = if resp.is_pointer_button_down_on() { c.press_fill } else { c.hover_fill };
            ui.painter().rect_filled(rect, 0.0, fill);
        }
        let color = if enabled { c.text } else { c.weak };
        let mut x = rect.min.x + s.menu_item_pad;
        if let Some(mark) = mark {
            let center = pos2(x + s.menu_radio_r, rect.center().y);
            let ring = Stroke::new(s.line, c.weak);
            match mark {
                Mark::Radio(on) => {
                    ui.painter().circle_stroke(center, s.menu_radio_r, ring);
                    if on {
                        ui.painter().circle_filled(center, s.menu_radio_dot, color);
                    }
                }
                Mark::Check(on) => {
                    let side = s.menu_check;
                    let r = Rect::from_center_size(center, vec2(side, side));
                    ui.painter().rect_stroke(r, t.radius.small, ring, egui::StrokeKind::Inside);
                    if on {
                        // A tick in the box's own proportions.
                        let k = side / 12.0;
                        let pts = vec![center + vec2(-3.5, 0.0) * k, center + vec2(-1.0, 2.8) * k, center + vec2(3.8, -3.0) * k];
                        ui.painter().add(egui::Shape::line(pts, Stroke::new(s.icon_bold, color)));
                    }
                }
            }
            x += s.menu_mark_w;
        }
        ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, t.font.body.clone(), color);
        if let Some(sc) = shortcut {
            ui.painter().text(pos2(rect.max.x - s.menu_item_pad, rect.center().y), Align2::RIGHT_CENTER, sc, t.font.shortcut.clone(), c.weak);
        }
        // Release over the item activates it, also after pressing on the title and dragging.
        let released = resp.hovered() && ui.input(|i| i.pointer.primary_released());
        let chosen = enabled && (released || self.activate == Some(index));
        if chosen {
            self.close = true;
        }
        chosen
    }

    /// Space between groups of rows (no line).
    pub fn gap(&mut self) {
        let t = self.theme.clone();
        self.ui.allocate_exact_size(vec2(self.width, t.size.menu_gap), Sense::hover());
    }

    /// A small dim label above a group of items.
    pub fn heading(&mut self, text: &str) {
        let t = self.theme.clone();
        let ui = &mut *self.ui;
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, t.size.menu_heading_h), Sense::hover());
        ui.painter().text(pos2(rect.min.x + t.size.menu_item_pad, rect.center().y), Align2::LEFT_CENTER, text, t.font.small.clone(), t.colors.weak);
    }

    /// A custom row: `left` (e.g. a label) and `right` (e.g. buttons, laid out right to
    /// left), padded like items. Its widgets don't close the menu and aren't reached by the
    /// arrow keys (Tab focuses them).
    pub fn custom(&mut self, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
        let t = self.theme.clone();
        let s = &t.size;
        let ui = &mut *self.ui;
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, s.menu_item_h + t.space.sm), Sense::hover());
        let inner = rect.shrink2(vec2(s.menu_item_pad, t.space.xs));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.spacing_mut().item_spacing = vec2(t.space.sm, 0.0);
        left(&mut child);
        let used = child.min_rect().width();
        let right_w = child.with_layout(egui::Layout::right_to_left(egui::Align::Center), right).response.rect.width();
        self.natural = self.natural.max(2.0 * s.menu_item_pad + used + right_w + 2.0 * t.space.lg);
    }
}
