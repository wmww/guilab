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
use guilab_kit::{MenuBar, Theme, ThemePref, WindowButton};
use guilab_markup::{Annotated, Chat, FeedItem, Sketch, Tool};

use crate::{
    host::{CursorImage, Gesture, HostCursor, HostInput, WindowOp},
    sizing::{self, Resizing},
};

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
    /// The cursor the session's apps want.
    pub cursor: HostCursor,
    /// Input that reached the apps (current session only).
    pub viz: Option<&'a guilab_proto::viz::VizLog>,
    pub under_pointer: Option<[u8; 3]>,
    /// What the agent is working on (None while it waits for the human).
    pub agent: Option<guilab_markup::Status>,
}

impl SessionView<'_> {
    fn blocked(&self) -> bool {
        self.locked
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
    TouchDown { id: i32, x: f64, y: f64 },
    TouchMotion { id: i32, x: f64, y: f64 },
    TouchUp { id: i32 },
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
        matches!(self, Input::Button { pressed: false, .. } | Input::Key { pressed: false, .. } | Input::PinchEnd { .. } | Input::SwipeEnd { .. } | Input::TouchUp { .. })
    }
}

pub enum Action {
    Input { session: String, input: Input },
    Revoke { session: String },
    /// A drawing started: answer with [`ViewerUi::set_screenshot`] (whole output, no cursor;
    /// the tag names the screenshot, e.g. its image id, and comes back in `Annotated::tag`).
    Freeze { session: String },
    /// The human sent a message (text, composited drawings with their text annotations).
    Message { session: String, text: String, images: Vec<Annotated> },
    /// Change the session's output (View menu, handles, "Lock size to window").
    Resize { session: String, size: Option<(u32, u32)>, scale: Option<f64> },
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
    sizing: Sizing,
}

/// Session sizing state (View menu).
#[derive(Default)]
struct Sizing {
    /// "Lock size to window": the session's logical size follows the view; no zoom/pan.
    follow: bool,
    /// The last size "follow" asked for, and when (egui time).
    sent: Option<((u32, u32), f64)>,
    /// The View menu's size and scale fields (synced to the session while it's closed).
    edit_size: (u32, u32),
    edit_scale: f64,
}

impl PerSession {
    fn new(key: &str) -> PerSession {
        let id = egui::Id::new(("session", key));
        PerSession { key: key.into(), zoom: Zoom::default(), sketch: Sketch::new(id.with("sketch")), chat: Chat::new(id.with("chat")), feed_next: 0, viz: Default::default(), sizing: Sizing::default() }
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
    /// UI passes in which that happened (for tests and debugging).
    pub blocked_tries: u64,
    /// Messages for the human (e.g. "Saved to ..."), shown as a toast; and when shown.
    notices: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    notice: Option<(String, f64)>,
    pub chat_place: ChatPlace,
    /// The app's cursor image, when the pointer is over an app that gets input and the app
    /// uses an image cursor (egui's cursor is then `Default`; the viewer shows this instead).
    app_image: Option<std::sync::Arc<CursorImage>>,
    /// The session view rect in the last frame (points).
    last_view: Option<Rect>,
    /// A resize handle drag, in this session.
    resizing: Option<(String, Resizing)>,
    /// Touch points we put down in `held_in` (to lift when input goes elsewhere).
    held_touches: Vec<i32>,
    /// Where touches go to the app as of the last frame: the image, with "Use the app".
    touch_area: Option<Rect>,
    /// Client-side decorations: draw a title bar (set by the viewer from the host).
    pub csd: bool,
    pub maximized: bool,
    window_ops: Vec<WindowOp>,
    /// Light/dark (View menu); System follows the desktop.
    pub theme: ThemePref,
}

/// View menu choices that outlive the window (the server keeps them across hide/show).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    pub theme: ThemePref,
    pub chat: ChatPlace,
}

/// Where the chat panel goes (View menu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatPlace {
    Off,
    #[default]
    Bottom,
    Right,
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
    pub fn prefs(&self) -> Prefs {
        Prefs { theme: self.theme, chat: self.chat_place }
    }

    pub fn set_prefs(&mut self, p: Prefs) {
        self.theme = p.theme;
        self.chat_place = p.chat;
    }

    /// The selected session (last frame).
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Shows a session's tab (from the next frame; ignored if there's no such session then).
    pub fn select(&mut self, name: &str) {
        self.current = Some(name.to_string());
    }

    /// Whether the current session's texture should be sampled with NEAREST (zoomed in).
    pub fn nearest(&self) -> bool {
        self.pixel_ratio >= 2.0
    }

    /// Whether a touch starting at `p` is the app's (its image, no drawing tool, no menu
    /// open; as of the last frame). Blocked ones go there too (and are dropped).
    pub fn touch_to_app(&self, ctx: &egui::Context, p: Pos2) -> bool {
        self.touch_area.is_some_and(|r| r.contains(p)) && !MenuBar::is_open(ctx, MENU_BAR)
    }

    /// The cursor the host should show, given the cursor egui picked this frame.
    pub fn host_cursor(&self, egui: CursorIcon) -> HostCursor {
        match &self.app_image {
            Some(img) if egui == CursorIcon::Default => HostCursor::Image(img.clone()),
            _ => HostCursor::Icon(egui),
        }
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
        for id in self.held_touches.drain(..) {
            out.push(Action::Input { session: session.clone(), input: Input::TouchUp { id } });
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
    pub fn set_screenshot(&mut self, name: &str, image: egui::ColorImage, tag: String) {
        if let Some(p) = self.sessions.get_mut(name) {
            p.sketch.set_screenshot(image, tag);
        }
    }

    /// Whether keys belong to egui even without a focused text field (a drawing or a menu
    /// is open).
    pub fn keys_to_egui(&self, ctx: &egui::Context) -> bool {
        MenuBar::is_open(ctx, MENU_BAR) || self.current.as_deref().and_then(|c| self.sessions.get(c)).is_some_and(|p| p.sketch.drawing_open())
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
        if ui.ctx().options(|o| o.theme_preference) != self.theme.egui() {
            ui.ctx().set_theme(self.theme.egui());
        }
        let t = Theme::get(ui.ctx());
        // A press outside an open menu only closes it (as on the desktop), so the view
        // must ignore presses this frame.
        let menu_was_open = MenuBar::is_open(ui.ctx(), MENU_BAR);
        self.menu_bar(ui, sessions, &mut out);

        // Session tabs, only when there's a choice. The selected one takes the view's color
        // below it, so it reads as attached to the session it shows.
        if sessions.len() > 1 {
            let (sm, xs) = ((t.space.sm + 2.0) as i8, t.space.xs as i8);
            egui::Panel::top("tabs").show_separator_line(false).frame(egui::Frame::new().fill(t.colors.panel).inner_margin(egui::Margin { left: sm, right: sm, top: xs, bottom: 0 })).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = t.space.xs;
                    for s in sessions {
                        let sel = self.current.as_deref() == Some(s.name);
                        if guilab_kit::tab(ui, sel, s.name, t.colors.view_bg).clicked() && !sel {
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

        egui::Panel::left("tools").resizable(false).exact_size(t.size.toolbar_w).show(ui, |ui| {
            ui.vertical_centered(|ui| p.sketch.toolbar(ui));
        });
        let panel = match self.chat_place {
            ChatPlace::Off => None,
            ChatPlace::Bottom => Some(egui::Panel::bottom("chat").default_size(180.0).size_range(90.0..=800.0)),
            ChatPlace::Right => Some(egui::Panel::right("chat-side").default_size(360.0).size_range(220.0..=1200.0)),
        };
        if let Some(panel) = panel {
            p.chat.set_status(s.agent.clone());
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
            self.last_view = Some(view);
            self.follow_window(s, &mut p.sizing, view, now, &mut out, ui.ctx());
            self.view(ui, s, &mut p, view, resp.contains_pointer() && !menu_was_open, inputs, menu_was_open, &mut out);
            if let (Some(log), Some(img)) = (s.viz, self.image_rect) {
                p.viz.update(log, now, &t.overlay);
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
            let secs = t.motion.toast_secs;
            if let Some(at) = self.blocked_at {
                let mut stop = false;
                let r = guilab_kit::toast(ui, egui::Id::new("blocked-toast"), view, slot, |ui| {
                    ui.label("Agent using app");
                    stop = ui.button("Stop").on_hover_text("Take the app back from the agent").clicked();
                });
                slot += 1;
                if stop {
                    out.push(Action::Revoke { session: s.name.to_string() });
                    self.blocked_at = None;
                } else if r.hovered() {
                    self.blocked_at = Some(now);
                } else if now - at > secs {
                    self.blocked_at = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(secs - (now - at) + 0.05));
                }
            }
            if let Ok(mut n) = self.notices.lock()
                && let Some(text) = n.pop()
            {
                n.clear();
                self.notice = Some((text, now));
            }
            if let Some((text, at)) = &self.notice {
                guilab_kit::toast(ui, egui::Id::new("notice-toast"), view, slot, |ui| {
                    ui.label(text.as_str());
                });
                if now - at > secs {
                    self.notice = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(secs - (now - at) + 0.05));
                }
            }
        });
        if p.sketch.wants_screenshot() {
            out.push(Action::Freeze { session: s.name.to_string() });
        }
        self.sessions.insert(s.name.to_string(), p);
        out
    }

    /// Client-side decorations in the menu bar's free part: a drag area (move; double-click
    /// maximizes) with the title, and minimize / maximize / close buttons at the right.
    fn title_bar(&mut self, ui: &mut egui::Ui, free: Rect) {
        let t = Theme::get(ui.ctx());
        let bw = t.size.window_button_w;
        let buttons = Rect::from_min_max(pos2(free.max.x - 3.0 * bw, free.min.y), free.max);
        let drag = Rect::from_min_max(free.min, pos2(buttons.min.x, free.max.y));
        let resp = ui.interact(drag, egui::Id::new("title-drag"), Sense::click_and_drag());
        if resp.double_clicked() {
            self.window_ops.push(WindowOp::ToggleMaximize);
        } else if resp.drag_started() {
            self.window_ops.push(WindowOp::Move);
        }
        ui.painter().text(free.center(), egui::Align2::CENTER_CENTER, "guilab", t.font.body.clone(), t.colors.weak);
        let max = if self.maximized { WindowButton::Restore } else { WindowButton::Maximize };
        let kinds = [(WindowButton::Minimize, WindowOp::Minimize), (max, WindowOp::ToggleMaximize), (WindowButton::Close, WindowOp::Close)];
        for (i, (kind, op)) in kinds.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(buttons.min.x + i as f32 * bw, free.min.y), vec2(bw, free.height()));
            if guilab_kit::window_button(ui, egui::Id::new(("window-button", i)), r, kind).clicked() {
                self.window_ops.push(op);
            }
        }
        // Resize borders around the window (not when maximized).
        if !self.maximized
            && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
            && let Some((x, y)) = window_edge(ui.ctx().content_rect(), t.size.window_border, pos)
        {
            ui.ctx().set_cursor_icon(crate::sizing::Edge { x, y }.cursor());
        }
    }

    /// Window operations the UI asked for (client-side decorations); the viewer applies them.
    pub fn take_window_ops(&mut self) -> Vec<WindowOp> {
        std::mem::take(&mut self.window_ops)
    }

    /// "Lock size to window": asks for the output size whose logical size fills the view,
    /// throttled. Not while an agent holds the lock (that freezes the size).
    fn follow_window(&self, s: &SessionView, sz: &mut Sizing, view: Rect, now: f64, out: &mut Vec<Action>, ctx: &egui::Context) {
        if !sz.follow || s.locked {
            sz.sent = None;
            return;
        }
        let want = sizing::fit_pixels(view, s.scale);
        if want == s.size {
            return;
        }
        let due = sz.sent.is_none_or(|(w, t)| now - t >= sizing::THROTTLE && (w != want || now - t >= 1.0));
        if due {
            out.push(Action::Resize { session: s.name.to_string(), size: Some(want), scale: None });
            sz.sent = Some((want, now));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(sizing::THROTTLE));
        }
    }

    /// File / Edit / View, for the current session.
    fn menu_bar(&mut self, ui: &mut egui::Ui, sessions: &[SessionView], out: &mut Vec<Action>) {
        let cur = self.current.clone();
        let mut chat_place = self.chat_place;
        let sv = cur.as_deref().and_then(|c| sessions.iter().find(|s| s.name == c));
        let last_view = self.last_view;
        let menu_open = MenuBar::is_open(ui.ctx(), MENU_BAR);
        let mut theme = self.theme;
        let t = Theme::get(ui.ctx());
        let p = sv.map(|s| self.per(s.name, s.key));
        let free = egui::Panel::top("menu").show_separator_line(false).frame(egui::Frame::new().fill(t.colors.panel)).show(ui, |ui| {
            MenuBar::new(MENU_BAR).show(ui, |bar| {
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
                    let fixed = p.sizing.follow;
                    let z = &mut p.zoom;
                    // Steppers: they keep the menu open.
                    let step = vec2(t.size.menu_item_h, t.size.menu_item_h - t.space.sm);
                    m.custom(
                        |ui| {
                            ui.label("Zoom");
                        },
                        |ui| {
                            ui.add_enabled_ui(!fixed, |ui| {
                                if ui.add(egui::Button::new("+").min_size(step)).on_hover_text("Zoom in").clicked() {
                                    z.zoom = (z.zoom * 1.25).min(MAX_ZOOM);
                                    z.fit = false;
                                }
                                if ui.add(egui::Button::new(format!("{:.0}%", z.zoom * 100.0)).min_size(vec2(step.x * 2.0, step.y))).on_hover_text("Actual size (100%)").clicked() {
                                    *z = Zoom { fit: false, zoom: 1.0, pan: Vec2::ZERO };
                                }
                                if ui.add(egui::Button::new("−").min_size(step)).on_hover_text("Zoom out").clicked() {
                                    z.zoom = (z.zoom / 1.25).max(MIN_ZOOM);
                                    z.fit = false;
                                }
                            });
                        },
                    );
                    if m.item("Fit to window", None, !z.fit && !fixed) {
                        *z = Zoom::default();
                    }
                    if let Some(sv) = sv {
                        session_size(m, sv, &mut p.sizing, z, last_view, menu_open, &name, out);
                    }
                    m.separator();
                    m.heading("Chat");
                    for (place, label) in [(ChatPlace::Off, "Off"), (ChatPlace::Bottom, "Bottom"), (ChatPlace::Right, "Right side")] {
                        if m.radio(chat_place == place, label) {
                            chat_place = place;
                        }
                    }
                    m.separator();
                    m.heading("Appearance");
                    for (pref, label) in ThemePref::ALL {
                        if m.radio(theme == pref, label) {
                            theme = pref;
                        }
                    }
                });
            })
        });
        self.chat_place = chat_place;
        self.theme = theme;
        if self.csd {
            self.title_bar(ui, free.inner);
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// `drawing`: the sketch owns the pointer over the image; nothing goes to the app.
    /// `menu_was_open`: presses this frame only close the menu.
    fn view(&mut self, ui: &mut egui::Ui, s: &SessionView, p: &mut PerSession, view: Rect, hovered: bool, inputs: Vec<HostInput>, menu_was_open: bool, out: &mut Vec<Action>) {
        let t = Theme::get(ui.ctx());
        let (z, sketch) = (&mut p.zoom, &p.sketch);
        let drawing = sketch.active();
        // "Lock size to window": always fit, no zoom or pan.
        let fixed = p.sizing.follow;
        if fixed {
            z.fit = true;
        }
        if self.resizing.as_ref().is_some_and(|(n, _)| n != s.name) {
            self.resizing = None;
        }
        // Handles: not while the agent holds the session, following the window or drawing.
        let handles = !s.locked && !fixed && !sketch.drawing_open() && s.texture.is_some();
        if !handles && let Some((_, r)) = self.resizing.take() {
            z.fit = r.was_fit;
        }
        // Drags on the image pan: the Move tool (off marks), or a frozen screenshot with no
        // drawing tool.
        let pan_tool = sketch.tool == Tool::Move || (sketch.tool == Tool::Interact && sketch.drawing_open());
        let ppp = ui.ctx().pixels_per_point();
        // Session pixels in points at zoom 1.
        let px = vec2(s.size.0 as f32, s.size.1 as f32) / ppp;
        if z.fit {
            let cap = (ppp / s.scale as f32).max(1.0);
            let fit = (view.width() / px.x).min(view.height() / px.y).min(cap);
            z.zoom = fit.max(MIN_ZOOM);
            z.pan = Vec2::ZERO;
        }
        // While a handle drags, the image's opposite side stays put.
        if let Some((_, r)) = &self.resizing {
            z.pan = r.place(px * z.zoom).center() - view.center();
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
            if fixed {
                return;
            }
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
                HostInput::Touch(t) => {
                    let (x, y) = to_session(img, t.pos);
                    match t.phase {
                        egui::TouchPhase::Start => {
                            if !to_app {
                                tried |= blocked;
                                continue;
                            }
                            if self.held_in.as_deref() != Some(s.name) {
                                out.extend(self.release_all());
                                self.held_in = Some(s.name.to_string());
                            }
                            self.held_touches.push(t.id);
                            send(out, Input::TouchDown { id: t.id, x, y });
                        }
                        _ if !self.held_touches.contains(&t.id) => {}
                        egui::TouchPhase::Move => send(out, Input::TouchMotion { id: t.id, x, y }),
                        egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                            self.held_touches.retain(|i| *i != t.id);
                            send(out, Input::TouchUp { id: t.id });
                        }
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
                    if let Some((_, r)) = &mut self.resizing {
                        r.moved(p);
                        continue;
                    }
                    if let Some(last) = self.panning {
                        z.pan += p - last;
                        z.fit = false;
                        self.panning = Some(p);
                        img = image(z);
                        continue;
                    }
                    if let Some((start, _)) = self.middle
                        && (p - start).length() > DRAG_SLOP
                        && !fixed
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
                        if button == PointerButton::Primary
                            && let Some((_, r)) = self.resizing.take()
                        {
                            out.push(Action::Resize { session: s.name.to_string(), size: Some(r.size), scale: None });
                            z.fit = r.was_fit;
                            continue;
                        }
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
                    if !ours(pos) || menu_was_open || self.panning.is_some() || self.resizing.is_some() {
                        continue;
                    }
                    let on_image = img.contains(pos);
                    if handles
                        && button == PointerButton::Primary
                        && self.held_buttons.is_empty()
                        && let Some(edge) = sizing::handle_at(&t, img, view, pos)
                    {
                        self.resizing = Some((s.name.to_string(), Resizing::new(edge, pos, img, s.size, z.fit)));
                        z.fit = false;
                        continue;
                    }
                    if button == PointerButton::Middle && self.held_buttons.is_empty() {
                        self.middle = Some((pos, to_session(img, pos)));
                        continue;
                    }
                    // Off the image, or with the Move tool anywhere but on a mark: pan.
                    if !on_image || (pan_tool && button == PointerButton::Primary && sketch.mark_at(pos).is_none()) {
                        if self.held_buttons.is_empty() && !fixed {
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
                            if delta != Vec2::ZERO && !fixed {
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
            self.blocked_tries += 1;
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
        painter.rect_filled(view, 0.0, t.colors.view_bg);
        match s.texture {
            Some(tex) => {
                painter.image(tex, img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                painter.text(view.center(), egui::Align2::CENTER_CENTER, "(no frame yet)", t.font.body.clone(), t.colors.weak);
            }
        }
        painter.rect_stroke(img, 0.0, egui::Stroke::new(t.size.line, t.colors.outline), egui::StrokeKind::Outside);
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let hot_handle = match &self.resizing {
            Some((_, r)) => Some(r.edge),
            None if handles && hovered && self.panning.is_none() => pointer.and_then(|p| sizing::handle_at(&t, img, view, p)),
            None => None,
        };
        if handles {
            sizing::paint_handles(&painter, &t, img, view, hot_handle);
        }
        if let Some((_, r)) = &mut self.resizing {
            painter.rect_stroke(r.target(), 0.0, egui::Stroke::new(t.size.line, t.colors.accent), egui::StrokeKind::Outside);
            if let Some(p) = pointer {
                guilab_kit::pointer_label(ui.ctx(), egui::Id::new("resize-readout"), view, p, format!("{} × {}", r.size.0, r.size.1));
            }
            if r.due(now) {
                out.push(Action::Resize { session: s.name.to_string(), size: Some(r.size), scale: None });
                r.sent = Some((r.size, now));
            } else if r.sent.is_some_and(|(sz, _)| sz != r.size) {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(sizing::THROTTLE));
            }
        }
        self.image_rect = Some(img);
        self.touch_area = (!drawing).then_some(img.intersect(view));
        self.pixel_ratio = z.zoom;

        // Cursor. Over the image: the app's when input goes to it, else a tool cursor, never
        // the arrow (the app's own cursor is usually an arrow; it would look like input goes
        // through). Drawing tools get theirs from the canvas, which runs after this.
        self.app_image = None;
        let over_img = hovered && ui.input(|i| i.pointer.latest_pos()).is_some_and(|p| img.contains(p));
        let ctx = ui.ctx();
        if let Some(e) = hot_handle {
            ctx.set_cursor_icon(e.cursor());
        } else if self.panning.is_some() {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        } else if pan_tool && hovered {
            ctx.set_cursor_icon(CursorIcon::Grab); // the canvas shows Move over marks
        } else if over_img && !drawing {
            if blocked {
                ctx.set_cursor_icon(CursorIcon::NotAllowed);
            } else {
                match &s.cursor {
                    HostCursor::Icon(i) => ctx.set_cursor_icon(*i),
                    HostCursor::Image(img) => {
                        ctx.set_cursor_icon(CursorIcon::Default);
                        self.app_image = Some(img.clone());
                    }
                }
            }
        }
    }
}

/// The menu bar's id.
const MENU_BAR: &str = "menubar";

/// The window edge `p` is on, if within `border` of it.
pub fn window_edge(screen: Rect, border: f32, p: Pos2) -> Option<(i8, i8)> {
    let side = |v: f32, lo: f32, hi: f32| if v < lo + border { -1 } else if v > hi - border { 1 } else { 0 };
    let e = (side(p.x, screen.min.x, screen.max.x), side(p.y, screen.min.y, screen.max.y));
    (e != (0, 0)).then_some(e)
}

/// Scale presets in the View menu.
const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

/// The View menu's session size section: resize to / lock to the window, size and scale.
#[allow(clippy::too_many_arguments)]
fn session_size(m: &mut guilab_kit::menu::MenuUi, s: &SessionView, sz: &mut Sizing, z: &mut Zoom, view: Option<Rect>, menu_open: bool, name: &str, out: &mut Vec<Action>) {
    if !menu_open || sz.follow || s.locked {
        // Opening (or not editable): show the session's current values.
        sz.edit_size = s.size;
        sz.edit_scale = s.scale;
    }
    let free = !s.locked;
    let resize = |out: &mut Vec<Action>, size: Option<(u32, u32)>, scale: Option<f64>| out.push(Action::Resize { session: name.to_string(), size, scale });
    m.separator();
    m.heading("Session size");
    if m.item("Resize to window", None, free && view.is_some()) {
        resize(out, view.map(|v| sizing::fit_pixels(v, s.scale)), None);
        *z = Zoom::default();
    }
    if m.check(sz.follow, "Lock size to window", true) {
        sz.follow = !sz.follow;
        *z = Zoom::default();
    }
    let edit_size = free && !sz.follow;
    m.custom(
        |ui| {
            ui.add_enabled(edit_size, egui::Label::new("Size"));
        },
        |ui| {
            ui.add_enabled_ui(edit_size, |ui| {
                let set = ui.button("Set").on_hover_text("Output size in pixels");
                let range = sizing::MIN_PX..=sizing::MAX_PX;
                let h = ui.add(egui::DragValue::new(&mut sz.edit_size.1).range(range.clone()).speed(4));
                ui.label("×");
                let w = ui.add(egui::DragValue::new(&mut sz.edit_size.0).range(range).speed(4));
                let enter = (w.lost_focus() || h.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (set.clicked() || enter) && sz.edit_size != s.size {
                    resize(out, Some(sz.edit_size), None);
                }
            });
        },
    );
    m.custom(
        |ui| {
            ui.add_enabled(free, egui::Label::new("Scale"));
        },
        |ui| {
            ui.add_enabled_ui(free, |ui| {
                let mut pick = None;
                let custom = ui.add(egui::DragValue::new(&mut sz.edit_scale).range(0.25..=8.0).speed(0.01).max_decimals(2)).on_hover_text("Other scale");
                if (custom.drag_stopped() || custom.lost_focus()) && sz.edit_scale != s.scale {
                    pick = Some(sz.edit_scale);
                }
                for sc in SCALES.iter().rev() {
                    if ui.selectable_label(s.scale == *sc, format!("{sc}")).clicked() && s.scale != *sc {
                        pick = Some(*sc);
                    }
                }
                if let Some(sc) = pick {
                    sz.edit_scale = sc;
                    // Following the window keeps the logical size; otherwise the pixel size.
                    let size = sz.follow.then_some(view).flatten().map(|v| sizing::fit_pixels(v, sc));
                    resize(out, size, Some(sc));
                }
            });
        },
    );
    if s.locked {
        m.note("The agent is using the app; its size is frozen");
    }
}
