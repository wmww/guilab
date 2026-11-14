//! guilab's viewer UI: session tabs, the session view (zoom/pan, input forwarding), lock
//! controls, and per session the drawing toolbar and chat panel (`guilab-markup`). Pure
//! egui: sessions come in as [`SessionView`]s, the human's actions go out as [`Action`]s,
//! which the server applies.
//!
//! Pointer scheme:
//! - "Use the app" tool: clicks, drags, scrolling and touchpad gestures over the image go
//!   to the app.
//! - Move tool: drag a mark to move it; drag anywhere else to pan.
//! - Scrolling that doesn't go to the app (drawing and Move tools, outside the image) moves
//!   the view, per [`NavMode`]. Ctrl+scroll always zooms at the pointer.
//! - Middle-drag anywhere, or any drag outside the image: pan. A middle click without
//!   movement still reaches the app (sent on release).
//! - Toolbar: zoom out/in, Fit (the default; follows the window size, up to the larger of
//!   1:1 and the session's logical size) and 1:1 (one session pixel per host pixel).
//!
//! Zoom is in host physical pixels per session pixel, so 100% is pixel-exact at any scale.

use std::collections::HashMap;

use egui::{Color32, CursorIcon, Event, PointerButton, Pos2, Rect, Sense, Vec2, pos2, vec2};
use guilab_markup::{Annotated, Chat, FeedItem, Sketch, Tool};

use crate::host::{Gesture, HostInput};

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
    /// Input that reached the apps (current session only).
    pub viz: Option<&'a guilab_proto::viz::VizLog>,
    pub under_pointer: Option<[u8; 3]>,
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
    /// The pointer left the app's image (or input to it stopped): apps get a pointer leave.
    Leave,
    /// Linux button code (BTN_LEFT = 0x110).
    Button { button: u32, pressed: bool },
    /// `steps`: wheel notches (None = smooth, e.g. touchpad). `stop`: fingers lifted.
    Scroll { dx: f64, dy: f64, steps: Option<(f64, f64)>, stop: bool },
    /// evdev key code.
    Key { code: u32, pressed: bool },
    PinchBegin { fingers: u32 },
    /// Delta in output pixels; `scale` absolute (vs. begin), `rotation` relative degrees.
    PinchUpdate { dx: f64, dy: f64, scale: f64, rotation: f64 },
    PinchEnd { cancelled: bool },
    SwipeBegin { fingers: u32 },
    SwipeUpdate { dx: f64, dy: f64 },
    SwipeEnd { cancelled: bool },
}

/// How scrolling moves the view when it doesn't go to the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavMode {
    /// Scrolling zooms at the pointer (mouse wheels; touchpads without gestures).
    ScrollZooms,
    /// Two-axis scrolling pans and pinching zooms at the pointer (touchpads).
    ScrollPans,
}

/// The [`NavMode`] to use: fixed, or picked per scroll event from the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavSetting {
    /// [`NavMode::ScrollPans`] for smooth (touchpad) scrolling when the host sends pinch
    /// gestures, else [`NavMode::ScrollZooms`].
    #[default]
    Auto,
    Fixed(NavMode),
}

impl NavSetting {
    fn resolve(self, smooth: bool, gestures: bool) -> NavMode {
        match self {
            NavSetting::Fixed(m) => m,
            NavSetting::Auto if smooth && gestures => NavMode::ScrollPans,
            NavSetting::Auto => NavMode::ScrollZooms,
        }
    }
}

/// Where the current touchpad gesture goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GestureTo {
    /// The app; a pinch, else a swipe.
    App { pinch: bool },
    /// The view: pinch zooms; the last absolute pinch scale.
    View { scale: f64 },
}

impl Input {
    /// Releases are always delivered, even while human input is blocked.
    pub fn is_release(&self) -> bool {
        matches!(self, Input::Button { pressed: false, .. } | Input::Key { pressed: false, .. } | Input::PinchEnd { .. } | Input::SwipeEnd { .. })
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
    /// File > Save screenshot: ask where, then save `image` (the open drawing with its
    /// marks), or the session's live output if None. Report back with [`ViewerUi::notice`].
    SaveScreenshot { session: String, image: Option<std::sync::Arc<egui::ColorImage>> },
}

/// UI state of one session.
struct PerSession {
    key: String,
    zoom: Zoom,
    sketch: Sketch,
    chat: Chat,
    /// Next event seq the feed wants (see [`ViewerUi::feed_cursor`]).
    feed_next: u64,
    viz: crate::viz::Overlay,
}

impl PerSession {
    fn new(key: &str) -> PerSession {
        let id = egui::Id::new(("session", key));
        PerSession { key: key.into(), zoom: Zoom::default(), sketch: Sketch::new(id.with("sketch")), chat: Chat::new(id.with("chat")), feed_next: 0, viz: Default::default() }
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
    /// The session our pointer motion last went to and that hasn't had a leave since.
    pointer_in: Option<String>,
    /// The host sends touchpad gestures (set by the viewer each frame).
    pub gestures: bool,
    pub nav: NavSetting,
    /// The touchpad gesture in progress: where it goes, and for the app, which session.
    gesture: Option<(GestureTo, String)>,
    /// When (egui time) the human last tried to use an app an agent has locked.
    blocked_at: Option<f64>,
    /// Messages for the human (e.g. "Saved to ..."), shown as a toast; and when shown.
    notices: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    notice: Option<(String, f64)>,
    pub chat_place: ChatPlace,
}

/// Where the chat panel goes (View menu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatPlace {
    Off,
    #[default]
    Bottom,
    Right,
}

/// Behind the session image.
const VIEW_BG: Color32 = Color32::from_gray(24);

/// How long toasts stay up after the thing that raised them (seconds).
const TOAST_SECS: f64 = 4.0;

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
        let mut out = Vec::new();
        if let Some((GestureTo::App { pinch }, session)) = self.gesture.take() {
            let input = if pinch { Input::PinchEnd { cancelled: true } } else { Input::SwipeEnd { cancelled: true } };
            out.push(Action::Input { session, input });
        }
        let Some(session) = self.held_in.take() else { return out };
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

    /// Where other threads post messages for the human (shown as a toast).
    pub fn notices(&self) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
        self.notices.clone()
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

    /// Whether keys belong to egui even without a focused text field (a drawing or a menu
    /// is open).
    pub fn keys_to_egui(&self, ctx: &egui::Context) -> bool {
        guilab_markup::menu::MenuBar::is_open(ctx, "menubar") || self.current.as_deref().and_then(|c| self.sessions.get(c)).is_some_and(|p| p.sketch.drawing_open())
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

        let now = ui.input(|i| i.time);
        self.menu_bar(ui, sessions, &mut out);

        // Session tabs, only when there's a choice. The selected one takes the view's color
        // below it, so it reads as attached to the session it shows.
        if sessions.len() > 1 {
            egui::Panel::top("tabs").show_separator_line(false).frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin { left: 6, right: 6, top: 2, bottom: 0 })).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for s in sessions {
                        let sel = self.current.as_deref() == Some(s.name);
                        if session_tab(ui, sel, s.name).clicked() && !sel {
                            self.current = Some(s.name.to_string());
                        }
                    }
                });
            });
        }

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
        if let Some(prev) = self.pointer_in.take_if(|p| p != s.name) {
            out.push(Action::Input { session: prev, input: Input::Leave });
        }
        // Taken out of the map while drawing, so `self` stays free to borrow.
        self.per(s.name, s.key);
        let mut p = self.sessions.remove(s.name).unwrap();

        egui::Panel::left("tools").resizable(false).exact_size(40.0).show(ui, |ui| {
            ui.vertical_centered(|ui| p.sketch.toolbar(ui));
        });
        let panel = match self.chat_place {
            ChatPlace::Off => None,
            ChatPlace::Bottom => Some(egui::Panel::bottom("chat").default_size(180.0).size_range(90.0..=800.0)),
            ChatPlace::Right => Some(egui::Panel::right("chat-side").default_size(360.0).size_range(220.0..=1200.0)),
        };
        if let Some(panel) = panel {
            panel.resizable(true).show(ui, |ui| {
                if let Some(m) = p.chat.show(ui, &mut p.sketch) {
                    out.push(Action::Message { session: s.name.to_string(), text: m.text, images: m.images });
                }
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let view = ui.available_rect_before_wrap();
            let resp = ui.allocate_rect(view, Sense::hover());
            let drawing = p.sketch.active();
            self.view(ui, s, &mut p.zoom, view, resp.hovered(), &p.sketch, inputs, &mut out);
            if let (Some(log), Some(img)) = (s.viz, self.image_rect) {
                p.viz.update(log, now);
                let light = s.under_pointer.is_some_and(|[r, g, b]| 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 185.0);
                p.viz.paint(ui, view, img, (s.size.0 as f32, s.size.1 as f32), now, light);
            }
            if drawing && let Some(rect) = self.image_rect {
                let size = [s.size.0 as usize, s.size.1 as usize];
                p.sketch.canvas(ui, guilab_markup::View { rect, clip: view, size, scale: s.scale as f32 });
            }
            // Toasts above the drawing's own (slot 0).
            let mut slot = usize::from(p.sketch.drawing_open());
            if !s.blocked() {
                self.blocked_at = None;
            }
            if let Some(t) = self.blocked_at {
                let mut stop = false;
                let r = guilab_markup::toast(ui, egui::Id::new("blocked-toast"), view, slot, |ui| {
                    ui.label("Agent using app");
                    stop = ui.button("Stop").on_hover_text("Take the app back from the agent").clicked();
                });
                slot += 1;
                if stop {
                    out.push(Action::Revoke { session: s.name.to_string() });
                    self.blocked_at = None;
                } else if r.hovered() {
                    self.blocked_at = Some(now);
                } else if now - t > TOAST_SECS {
                    self.blocked_at = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(TOAST_SECS - (now - t) + 0.05));
                }
            }
            if let Ok(mut n) = self.notices.lock()
                && let Some(text) = n.pop()
            {
                n.clear();
                self.notice = Some((text, now));
            }
            if let Some((text, t)) = &self.notice {
                guilab_markup::toast(ui, egui::Id::new("notice-toast"), view, slot, |ui| {
                    ui.label(text.as_str());
                });
                if now - t > TOAST_SECS {
                    self.notice = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(TOAST_SECS - (now - t) + 0.05));
                }
            }
        });
        if p.sketch.wants_screenshot() {
            out.push(Action::Freeze { session: s.name.to_string() });
        }
        self.sessions.insert(s.name.to_string(), p);
        out
    }

    /// File / Edit / View, for the current session.
    fn menu_bar(&mut self, ui: &mut egui::Ui, sessions: &[SessionView], out: &mut Vec<Action>) {
        let cur = self.current.clone();
        let mut chat_place = self.chat_place;
        let p = cur.as_deref().and_then(|c| {
            let key = sessions.iter().find(|s| s.name == c)?.key;
            Some(self.per(c, key))
        });
        egui::Panel::top("menu").show_separator_line(false).frame(egui::Frame::new().fill(ui.visuals().panel_fill)).show(ui, |ui| {
            guilab_markup::menu::MenuBar::new("menubar").show(ui, |bar| {
                let Some(p) = p else {
                    for m in ["File", "Edit", "View"] {
                        bar.menu_enabled(m, false, |_| {});
                    }
                    return;
                };
                let name = cur.clone().unwrap_or_default();
                bar.menu("File", |m| {
                    let label = if p.sketch.drawing_open() { "Save drawing as image…" } else { "Save screenshot…" };
                    if m.item(label, None, true) {
                        out.push(Action::SaveScreenshot { session: name.clone(), image: p.sketch.preview() });
                    }
                });
                bar.menu("Edit", |m| {
                    if m.item("Undo", Some("Ctrl+Z"), p.sketch.can_undo()) {
                        p.sketch.undo();
                    }
                    if m.item("Redo", Some("Ctrl+Shift+Z"), p.sketch.can_redo()) {
                        p.sketch.redo();
                    }
                });
                bar.menu("View", |m| {
                    let z = &mut p.zoom;
                    // Steppers: they keep the menu open.
                    m.custom(
                        |ui| {
                            ui.label("Zoom");
                        },
                        |ui| {
                            if ui.add(egui::Button::new("+").min_size(vec2(26.0, 22.0))).clicked() {
                                z.zoom = (z.zoom * 1.25).min(MAX_ZOOM);
                                z.fit = false;
                            }
                            if ui.add(egui::Button::new(format!("{:.0}%", z.zoom * 100.0)).min_size(vec2(56.0, 22.0))).on_hover_text("Actual size (100%)").clicked() {
                                *z = Zoom { fit: false, zoom: 1.0, pan: Vec2::ZERO };
                            }
                            if ui.add(egui::Button::new("−").min_size(vec2(26.0, 22.0))).clicked() {
                                z.zoom = (z.zoom / 1.25).max(MIN_ZOOM);
                                z.fit = false;
                            }
                        },
                    );
                    if m.item("Fit to window", None, !z.fit) {
                        *z = Zoom::default();
                    }
                    m.separator();
                    m.heading("Chat");
                    for (place, label) in [(ChatPlace::Off, "Off"), (ChatPlace::Bottom, "Bottom"), (ChatPlace::Right, "Right side")] {
                        if m.radio(chat_place == place, label) {
                            chat_place = place;
                        }
                    }
                });
            });
        });
        self.chat_place = chat_place;
    }

    #[allow(clippy::too_many_arguments)]
    /// `drawing`: the sketch owns the pointer over the image; nothing goes to the app.
    fn view(&mut self, ui: &mut egui::Ui, s: &SessionView, z: &mut Zoom, view: Rect, hovered: bool, sketch: &Sketch, inputs: Vec<HostInput>, out: &mut Vec<Action>) {
        let drawing = sketch.active();
        let move_tool = sketch.tool == Tool::Move;
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
        let now = ui.input(|i| i.time);
        // The human tried to use the app while an agent holds it: say so (a toast).
        let mut tried = false;
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
                    } else if blocked {
                        tried = true;
                    }
                    continue;
                }
                HostInput::Gesture(g) => {
                    // Over the app with "Use the app": the whole gesture goes to it. Else
                    // a pinch zooms the view (swipes do nothing).
                    let k = s.size.0 as f64 / img.width() as f64; // session px per point
                    match g {
                        Gesture::PinchBegin { fingers } | Gesture::SwipeBegin { fingers } => {
                            let pinch = matches!(g, Gesture::PinchBegin { .. });
                            let over_img = self.pointer.is_some_and(|p| ours(p) && img.contains(p));
                            tried |= blocked && !drawing && over_img;
                            let over_app = to_app && over_img;
                            if over_app {
                                self.gesture = Some((GestureTo::App { pinch }, s.name.to_string()));
                                send(out, if pinch { Input::PinchBegin { fingers } } else { Input::SwipeBegin { fingers } });
                            } else {
                                self.gesture = pinch.then(|| (GestureTo::View { scale: 1.0 }, s.name.to_string()));
                            }
                        }
                        Gesture::PinchUpdate { dx, dy, scale, rotation } => match &mut self.gesture {
                            Some((GestureTo::App { pinch: true }, _)) => send(out, Input::PinchUpdate { dx: dx * k, dy: dy * k, scale, rotation }),
                            Some((GestureTo::View { scale: last }, _)) => {
                                if scale > 0.0
                                    && *last > 0.0
                                    && let Some(p) = self.pointer
                                {
                                    zoom_at(z, &mut img, p, (scale / *last) as f32);
                                }
                                *last = scale;
                            }
                            _ => {}
                        },
                        Gesture::SwipeUpdate { dx, dy } => {
                            if matches!(self.gesture, Some((GestureTo::App { pinch: false }, _))) {
                                send(out, Input::SwipeUpdate { dx: dx * k, dy: dy * k });
                            }
                        }
                        Gesture::PinchEnd { cancelled } | Gesture::SwipeEnd { cancelled } => {
                            if let Some((GestureTo::App { pinch }, session)) = self.gesture.take() {
                                let input = if pinch { Input::PinchEnd { cancelled } } else { Input::SwipeEnd { cancelled } };
                                out.push(Action::Input { session, input });
                            }
                        }
                    }
                    continue;
                }
            };
            match ev {
                Event::PointerMoved(p) | Event::PointerButton { pos: p, .. } => self.pointer = Some(p),
                Event::PointerGone => {
                    self.pointer = None;
                    if self.held_buttons.is_empty() && self.pointer_in.take().is_some() {
                        send(out, Input::Leave);
                    }
                }
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
                        self.pointer_in = Some(s.name.to_string());
                    } else if self.pointer_in.take().is_some() {
                        send(out, Input::Leave);
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
                    // Off the image, or with the Move tool anywhere but on a mark: pan.
                    if !on_image || (move_tool && button == PointerButton::Primary && sketch.mark_at(pos).is_none()) {
                        if self.held_buttons.is_empty() {
                            self.panning = Some(pos);
                        }
                        continue;
                    }
                    if !to_app {
                        tried |= blocked && !drawing;
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
                        let smooth = unit == egui::MouseWheelUnit::Point;
                        if !modifiers.ctrl && self.nav.resolve(smooth, self.gestures) == NavMode::ScrollPans {
                            let k = match unit {
                                egui::MouseWheelUnit::Point => 1.0,
                                egui::MouseWheelUnit::Line => 40.0,
                                egui::MouseWheelUnit::Page => view.height(),
                            };
                            if delta != Vec2::ZERO {
                                z.pan += delta * k;
                                z.fit = false;
                                img = image(z);
                            }
                            continue;
                        }
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
                        tried = true;
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
                _ => {}
            }
        }

        if tried {
            self.blocked_at = Some(now);
        }
        // A drawing tool or the lock took over while the pointer was in the app.
        if !to_app && self.held_buttons.is_empty() && self.pointer_in.take().is_some() {
            send(out, Input::Leave);
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
        painter.rect_filled(view, 0.0, VIEW_BG);
        match s.texture {
            Some(tex) => {
                painter.image(tex, img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                painter.text(view.center(), egui::Align2::CENTER_CENTER, "(no frame yet)", egui::FontId::proportional(14.0), Color32::GRAY);
            }
        }
        painter.rect_stroke(img, 0.0, egui::Stroke::new(1.0, Color32::from_gray(60)), egui::StrokeKind::Outside);
        self.image_rect = Some(img);
        self.pixel_ratio = z.zoom;

        // Cursor: the app's over its image, a hand while panning.
        if self.panning.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if move_tool && hovered {
            ui.ctx().set_cursor_icon(CursorIcon::Grab); // the canvas shows Move over marks
        } else if hovered && !drawing && let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
            if img.contains(p) {
                ui.ctx().set_cursor_icon(s.cursor);
            }
        }
    }
}


/// One session tab: rounded top corners, an accent line on the selected one.
fn session_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let v = ui.visuals().clone();
    let text_color = if selected { v.strong_text_color() } else { v.weak_text_color() };
    let galley = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::proportional(14.0), text_color);
    let size = egui::vec2((galley.size().x + 32.0).max(80.0), 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected {
        VIEW_BG
    } else if resp.hovered() {
        v.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(rect, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, fill);
    if selected {
        let top = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 0.0), egui::vec2(rect.width() - 12.0, 2.0));
        p.rect_filled(top, 1.0, v.selection.bg_fill);
    }
    let text_color = if !selected && resp.hovered() { v.text_color() } else { text_color };
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, text_color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
