//! guilab's viewer UI: session tabs, the session view (zoom/pan, input forwarding), lock
//! controls, and per session the drawing toolbar and chat panel (`guilab-markup`). Pure
//! egui: sessions come in as [`SessionView`]s, the human's actions go out as [`Action`]s,
//! which the server applies.
//!
//! Mouse scheme (the app gets plain clicks, drags and scrolling over its image):
//! - Ctrl+scroll anywhere, or scroll outside the image: zoom at the pointer. Pinch: zoom.
//! - Middle-drag anywhere, or any drag outside the image: pan. A middle click without
//!   movement still reaches the app (sent on release).
//! - Toolbar: zoom out/in, Fit (the default; follows the window size, up to the larger of
//!   1:1 and the session's logical size) and 1:1 (one session pixel per host pixel).
//!
//! Zoom is in host physical pixels per session pixel, so 100% is pixel-exact at any scale.

use std::collections::HashMap;

use egui::{Color32, CursorIcon, Event, PointerButton, Pos2, Rect, Sense, Vec2, pos2, vec2};
use guilab_markup::{Annotated, Chat, FeedItem, Sketch};

use crate::host::HostInput;

/// A session as the UI sees it.
pub struct SessionView<'a> {
    pub name: &'a str,
    /// Unique per session instance (a new session with an old name starts fresh).
    pub key: &'a str,
    /// The session's latest frame, if any.
    pub texture: Option<egui::TextureId>,
    /// Output size in pixels.
    pub size: (u32, u32),
    pub scale: f64,
    pub locked: bool,
    pub shared: bool,
    /// The cursor the session's apps want.
    pub cursor: CursorIcon,
}

impl SessionView<'_> {
    fn blocked(&self) -> bool {
        self.locked && !self.shared
    }
}

/// Human input for a session, in output pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Motion { x: f64, y: f64 },
    /// Linux button code (BTN_LEFT = 0x110).
    Button { button: u32, pressed: bool },
    /// `steps`: wheel notches (None = smooth, e.g. touchpad). `stop`: fingers lifted.
    Scroll { dx: f64, dy: f64, steps: Option<(f64, f64)>, stop: bool },
    /// evdev key code.
    Key { code: u32, pressed: bool },
}

impl Input {
    /// Releases are always delivered, even while human input is blocked.
    pub fn is_release(&self) -> bool {
        matches!(self, Input::Button { pressed: false, .. } | Input::Key { pressed: false, .. })
    }
}

pub enum Action {
    Input { session: String, input: Input },
    Revoke { session: String },
    Share { session: String, on: bool },
    /// A drawing started: answer with [`ViewerUi::set_screenshot`] (whole output, no cursor).
    Freeze { session: String },
    /// The human sent a message (text, composited drawings with their text annotations).
    Message { session: String, text: String, images: Vec<Annotated> },
}

/// UI state of one session.
struct PerSession {
    key: String,
    zoom: Zoom,
    sketch: Sketch,
    chat: Chat,
    /// Next event seq the feed wants (see [`ViewerUi::feed_cursor`]).
    feed_next: u64,
}

impl PerSession {
    fn new(key: &str) -> PerSession {
        let id = egui::Id::new(("session", key));
        PerSession { key: key.into(), zoom: Zoom::default(), sketch: Sketch::new(id.with("sketch")), chat: Chat::new(id.with("chat")), feed_next: 0 }
    }
}

/// Zoom and pan of one session's view.
#[derive(Clone, Copy)]
struct Zoom {
    /// Follow the window: fit, at most 100%.
    fit: bool,
    /// Host physical pixels per session pixel.
    zoom: f32,
    /// Image center minus view center, in points.
    pan: Vec2,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom { fit: true, zoom: 1.0, pan: Vec2::ZERO }
    }
}

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 32.0;
/// Middle-button movement (points) that turns a click into a pan.
const DRAG_SLOP: f32 = 4.0;
/// Session pixels per wheel notch, at scale 1 (matches agent `scroll`).
const WHEEL_PX: f64 = 15.0;

#[derive(Default)]
pub struct ViewerUi {
    current: Option<String>,
    sessions: HashMap<String, PerSession>,
    /// Buttons and keys we pressed in `held_in`, to release when input goes elsewhere.
    held_buttons: Vec<u32>,
    held_keys: Vec<u32>,
    held_in: Option<String>,
    /// A middle press waiting to become a click or a pan: (where, session px there).
    middle: Option<(Pos2, (f64, f64))>,
    /// Panning since this pointer position.
    panning: Option<Pos2>,
    /// The image rect of the current session in the last frame.
    image_rect: Option<Rect>,
    /// Host physical pixels per session pixel in the last frame.
    pixel_ratio: f32,
    /// Pointer position as of the input processed so far.
    pointer: Option<Pos2>,
}

fn linux_button(b: PointerButton) -> u32 {
    match b {
        PointerButton::Primary => 0x110,
        PointerButton::Secondary => 0x111,
        PointerButton::Middle => 0x112,
        PointerButton::Extra1 => 0x113,
        PointerButton::Extra2 => 0x114,
    }
}

impl ViewerUi {
    /// The selected session (last frame).
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Whether the current session's texture should be sampled with NEAREST (zoomed in).
    pub fn nearest(&self) -> bool {
        self.pixel_ratio >= 2.0
    }

    /// Releases everything the human holds in a session (on hide, tab switch, lock).
    pub fn release_all(&mut self) -> Vec<Action> {
        let Some(session) = self.held_in.take() else { return Vec::new() };
        let mut out = Vec::new();
        for button in self.held_buttons.drain(..) {
            out.push(Action::Input { session: session.clone(), input: Input::Button { button, pressed: false } });
        }
        for code in self.held_keys.drain(..) {
            out.push(Action::Input { session: session.clone(), input: Input::Key { code, pressed: false } });
        }
        self.middle = None;
        out
    }

    fn release_keys(&mut self, out: &mut Vec<Action>) {
        let Some(session) = self.held_in.clone() else { return };
        for code in self.held_keys.drain(..) {
            out.push(Action::Input { session: session.clone(), input: Input::Key { code, pressed: false } });
        }
    }

    fn per(&mut self, name: &str, key: &str) -> &mut PerSession {
        let p = self.sessions.entry(name.to_string()).or_insert_with(|| PerSession::new(key));
        if p.key != key {
            *p = PerSession::new(key);
        }
        p
    }

    /// The next session event seq its feed wants (0 = it has nothing yet).
    pub fn feed_next(&mut self, name: &str, key: &str) -> u64 {
        self.per(name, key).feed_next
    }

    /// Appends feed items for a session; `next`: the seq after the last event they cover.
    pub fn push_feed(&mut self, name: &str, key: &str, items: Vec<FeedItem>, next: u64) {
        let p = self.per(name, key);
        for i in items {
            p.chat.push(i);
        }
        p.feed_next = next;
    }

    /// The frozen screenshot for a drawing that asked for one ([`Action::Freeze`]).
    pub fn set_screenshot(&mut self, name: &str, image: egui::ColorImage) {
        if let Some(p) = self.sessions.get_mut(name) {
            p.sketch.set_screenshot(image);
        }
    }

    /// Whether keys belong to egui even without a focused text field (a drawing is open).
    pub fn keys_to_egui(&self) -> bool {
        self.current.as_deref().and_then(|c| self.sessions.get(c)).is_some_and(|p| p.sketch.drawing_open())
    }

    /// Draws the whole viewer. `inputs`: this frame's host input in order; raw keys in it
    /// are meant for the session (egui doesn't want keyboard input). `keyboard_left`: the
    /// host keyboard focus left the window.
    pub fn show(&mut self, ui: &mut egui::Ui, sessions: &[SessionView], inputs: Vec<HostInput>, keyboard_left: bool) -> Vec<Action> {
        let mut out = Vec::new();
        if self.current.as_deref().is_none_or(|c| !sessions.iter().any(|s| s.name == c)) {
            self.current = sessions.first().map(|s| s.name.to_string());
        }
        self.sessions.retain(|k, p| sessions.iter().any(|s| s.name == k && s.key == p.key));

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                for s in sessions {
                    let sel = self.current.as_deref() == Some(s.name);
                    let label = if s.locked { format!("🔒 {}", s.name) } else { s.name.to_string() };
                    if ui.selectable_label(sel, label).clicked() && !sel {
                        self.current = Some(s.name.to_string());
                    }
                }
            });
        });

        let cur = self.current.as_deref().and_then(|c| sessions.iter().find(|s| s.name == c));
        if self.held_in.is_some() && (self.held_in.as_deref() != cur.map(|s| s.name) || cur.is_some_and(|s| s.blocked())) {
            out.extend(self.release_all());
        }
        if keyboard_left {
            self.release_keys(&mut out);
        }
        let Some(s) = cur else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| ui.label("No sessions. Start one with `guilab new`."));
            });
            self.image_rect = None;
            return out;
        };
        // Taken out of the map while drawing, so `self` stays free to borrow.
        self.per(s.name, s.key);
        let mut p = self.sessions.remove(s.name).unwrap();

        egui::Panel::left("tools").resizable(false).exact_size(40.0).show(ui, |ui| {
            ui.vertical_centered(|ui| p.sketch.toolbar(ui));
        });
        egui::Panel::bottom("chat").resizable(true).default_size(180.0).size_range(90.0..=800.0).show(ui, |ui| {
            if let Some(m) = p.chat.show(ui, &mut p.sketch) {
                out.push(Action::Message { session: s.name.to_string(), text: m.text, images: m.images });
            }
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            egui::Panel::top("session-bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.lock_bar(ui, s, &mut out);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        zoom_bar(ui, &mut p.zoom);
                    });
                });
            });
            let view = ui.available_rect_before_wrap();
            let resp = ui.allocate_rect(view, Sense::hover());
            let drawing = p.sketch.active();
            self.view(ui, s, &mut p.zoom, view, resp.hovered(), drawing, inputs, &mut out);
            if drawing && let Some(rect) = self.image_rect {
                let size = [s.size.0 as usize, s.size.1 as usize];
                p.sketch.canvas(ui, guilab_markup::View { rect, clip: view, size, scale: s.scale as f32 });
            }
        });
        if p.sketch.wants_screenshot() {
            out.push(Action::Freeze { session: s.name.to_string() });
        }
        self.sessions.insert(s.name.to_string(), p);
        out
    }

    fn lock_bar(&mut self, ui: &mut egui::Ui, s: &SessionView, out: &mut Vec<Action>) {
        if !s.locked {
            ui.label("Unlocked: you and agents can use it.");
            return;
        }
        // Buttons first, so they don't move when the text changes.
        if ui.button("Revoke").on_hover_text("Take the lock away from the agent").clicked() {
            out.push(Action::Revoke { session: s.name.to_string() });
        }
        if ui.selectable_label(s.shared, "Share").on_hover_text("Use the app too while the agent keeps its lock").clicked() {
            out.push(Action::Share { session: s.name.to_string(), on: !s.shared });
        }
        if s.shared {
            ui.label("🔒 An agent holds the lock; shared, so you can use it too.");
        } else {
            ui.colored_label(Color32::from_rgb(240, 180, 80), "🔒 An agent holds the lock; your input is blocked.");
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// `drawing`: the sketch owns the pointer over the image; nothing goes to the app.
    fn view(&mut self, ui: &mut egui::Ui, s: &SessionView, z: &mut Zoom, view: Rect, hovered: bool, drawing: bool, inputs: Vec<HostInput>, out: &mut Vec<Action>) {
        let ppp = ui.ctx().pixels_per_point();
        // Session pixels in points at zoom 1.
        let px = vec2(s.size.0 as f32, s.size.1 as f32) / ppp;
        if z.fit {
            let cap = (ppp / s.scale as f32).max(1.0);
            let fit = (view.width() / px.x).min(view.height() / px.y).min(cap);
            z.zoom = fit.max(MIN_ZOOM);
            z.pan = Vec2::ZERO;
        }
        let image = |z: &Zoom| {
            let size = px * z.zoom;
            let min = view.center() + z.pan - size / 2.0;
            // Snap to physical pixels so 100% stays sharp.
            let min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
            Rect::from_min_size(min, size)
        };
        let mut img = image(z);
        let to_session = |img: Rect, p: Pos2| {
            let rel = (p - img.min) / img.size();
            (rel.x as f64 * s.size.0 as f64, rel.y as f64 * s.size.1 as f64)
        };
        let send = |out: &mut Vec<Action>, input: Input| out.push(Action::Input { session: s.name.to_string(), input });
        let blocked = s.blocked();
        let to_app = !blocked && !drawing;
        let layer = ui.layer_id();
        let ours = |p: Pos2| view.contains(p) && ui.ctx().layer_id_at(p).is_none_or(|l| l == layer);
        let zoom_at = |z: &mut Zoom, img: &mut Rect, p: Pos2, factor: f32| {
            let new = (z.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            let c = view.center();
            z.pan = (p - c) - (p - c - z.pan) * (new / z.zoom);
            z.zoom = new;
            z.fit = false;
            *img = image(z);
        };
        for input in inputs {
            let ev = match input {
                HostInput::Egui(ev) => ev,
                HostInput::Key(k) => {
                    // Keys go to the session while egui doesn't want them (see Viewer).
                    if !k.pressed {
                        if let Some(i) = self.held_keys.iter().position(|c| *c == k.code) {
                            self.held_keys.remove(i);
                            send(out, Input::Key { code: k.code, pressed: false });
                        }
                    } else if !blocked && !self.held_keys.contains(&k.code) {
                        if self.held_in.as_deref() != Some(s.name) {
                            out.extend(self.release_all());
                            self.held_in = Some(s.name.to_string());
                        }
                        self.held_keys.push(k.code);
                        send(out, Input::Key { code: k.code, pressed: true });
                    }
                    continue;
                }
            };
            match ev {
                Event::PointerMoved(p) | Event::PointerButton { pos: p, .. } => self.pointer = Some(p),
                Event::PointerGone => self.pointer = None,
                _ => {}
            }
            match ev {
                Event::PointerMoved(p) => {
                    if let Some(last) = self.panning {
                        z.pan += p - last;
                        z.fit = false;
                        self.panning = Some(p);
                        img = image(z);
                        continue;
                    }
                    if let Some((start, _)) = self.middle
                        && (p - start).length() > DRAG_SLOP
                    {
                        self.middle = None;
                        z.pan += p - start;
                        z.fit = false;
                        self.panning = Some(p);
                        img = image(z);
                        continue;
                    }
                    let grabbed = !self.held_buttons.is_empty();
                    if grabbed || (to_app && ours(p) && img.contains(p)) {
                        let (x, y) = to_session(img, p);
                        send(out, Input::Motion { x, y });
                    }
                }
                Event::PointerButton { pos, button, pressed, .. } => {
                    let code = linux_button(button);
                    if !pressed {
                        if self.panning.is_some() && (button == PointerButton::Middle || self.held_buttons.is_empty()) {
                            self.panning = None;
                            continue;
                        }
                        if button == PointerButton::Middle
                            && let Some((_, (x, y))) = self.middle.take()
                        {
                            // A middle click that never moved: deliver it now.
                            if to_app {
                                send(out, Input::Motion { x, y });
                                send(out, Input::Button { button: code, pressed: true });
                                send(out, Input::Button { button: code, pressed: false });
                            }
                            continue;
                        }
                        if let Some(i) = self.held_buttons.iter().position(|b| *b == code) {
                            self.held_buttons.remove(i);
                            send(out, Input::Button { button: code, pressed: false });
                        }
                        continue;
                    }
                    if !ours(pos) || self.panning.is_some() {
                        continue;
                    }
                    let on_image = img.contains(pos);
                    if button == PointerButton::Middle && self.held_buttons.is_empty() {
                        self.middle = Some((pos, to_session(img, pos)));
                        continue;
                    }
                    if !on_image {
                        if self.held_buttons.is_empty() {
                            self.panning = Some(pos);
                        }
                        continue;
                    }
                    if !to_app {
                        continue;
                    }
                    self.held_in = Some(s.name.to_string());
                    self.held_buttons.push(code);
                    let (x, y) = to_session(img, pos);
                    send(out, Input::Motion { x, y });
                    send(out, Input::Button { button: code, pressed: true });
                }
                Event::MouseWheel { unit, delta, phase, modifiers } => {
                    let Some(p) = self.pointer else { continue };
                    if !ours(p) {
                        continue;
                    }
                    if modifiers.ctrl || drawing || !img.contains(p) {
                        let notches = match unit {
                            egui::MouseWheelUnit::Point => delta.y / 50.0,
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Page => delta.y * 3.0,
                        };
                        if notches != 0.0 {
                            zoom_at(z, &mut img, p, 1.15f32.powf(notches));
                        }
                        continue;
                    }
                    if blocked {
                        continue;
                    }
                    let stop = phase == egui::TouchPhase::End;
                    let input = match unit {
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                            let m = if unit == egui::MouseWheelUnit::Page { 3.0 } else { 1.0 };
                            let (sx, sy) = (-delta.x as f64 * m, -delta.y as f64 * m);
                            let px = WHEEL_PX * s.scale;
                            Input::Scroll { dx: sx * px, dy: sy * px, steps: Some((sx, sy)), stop }
                        }
                        egui::MouseWheelUnit::Point => {
                            // Content follows the fingers: host pixels -> session pixels.
                            let k = (s.size.0 as f32 / img.width()) as f64;
                            Input::Scroll { dx: -delta.x as f64 * k, dy: -delta.y as f64 * k, steps: None, stop }
                        }
                    };
                    send(out, input);
                }
                Event::Zoom(f) => {
                    if let Some(p) = self.pointer.filter(|p| ours(*p)) {
                        zoom_at(z, &mut img, p, f);
                    }
                }
                _ => {}
            }
        }

        // Clamp the pan so some of the image stays in view.
        let keep = 48.0;
        let max = (img.size() + view.size()) / 2.0 - Vec2::splat(keep);
        let clamped = z.pan.clamp(-max.max(Vec2::ZERO), max.max(Vec2::ZERO));
        if clamped != z.pan {
            z.pan = clamped;
            img = image(z);
        }

        let painter = ui.painter_at(view);
        painter.rect_filled(view, 0.0, Color32::from_gray(24));
        match s.texture {
            Some(tex) => {
                painter.image(tex, img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                painter.text(view.center(), egui::Align2::CENTER_CENTER, "(no frame yet)", egui::FontId::proportional(14.0), Color32::GRAY);
            }
        }
        painter.rect_stroke(img, 0.0, egui::Stroke::new(1.0, Color32::from_gray(60)), egui::StrokeKind::Outside);
        if blocked && !drawing {
            let banner = Rect::from_min_size(view.min + vec2(8.0, 8.0), vec2(view.width() - 16.0, 26.0));
            painter.rect_filled(banner, 4.0, Color32::from_black_alpha(190));
            painter.text(banner.center(), egui::Align2::CENTER_CENTER, "Input blocked: an agent holds the lock", egui::FontId::proportional(14.0), Color32::from_rgb(240, 180, 80));
        }
        self.image_rect = Some(img);
        self.pixel_ratio = z.zoom;

        // Cursor: the app's over its image, a hand while panning.
        if self.panning.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if hovered && !drawing && let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
            if img.contains(p) && !blocked {
                ui.ctx().set_cursor_icon(s.cursor);
            } else if img.contains(p) {
                ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
            }
        }
    }
}

fn zoom_bar(ui: &mut egui::Ui, z: &mut Zoom) {
    // Right to left.
    if ui.selectable_label(!z.fit && z.zoom == 1.0, "1:1").on_hover_text("One session pixel per screen pixel").clicked() {
        *z = Zoom { fit: false, zoom: 1.0, pan: Vec2::ZERO };
    }
    if ui.selectable_label(z.fit, "Fit").on_hover_text("Fit the window (reset zoom and pan)").clicked() {
        *z = Zoom::default();
    }
    if ui.button("+").clicked() {
        z.zoom = (z.zoom * 1.25).min(MAX_ZOOM);
        z.fit = false;
    }
    ui.label(format!("{:.0}%", z.zoom * 100.0));
    if ui.button("−").clicked() {
        z.zoom = (z.zoom / 1.25).max(MIN_ZOOM);
        z.fit = false;
    }
    ui.weak("Ctrl+scroll: zoom · middle-drag: pan");
}
