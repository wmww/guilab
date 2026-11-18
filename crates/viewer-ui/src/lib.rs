//! guilab's viewer UI: session tabs, the session view (zoom/pan, input forwarding), lock
//! controls, and per session the image editor (`guilab-editor`) and the chat
//! (`guilab-chat`), glued by a [`workspace::Workspace`]. Pure egui: sessions come in as
//! [`SessionView`]s, store images are read from the server's `Store`, and the human's
//! actions go out as [`Action`]s, which the server applies.
//!
//! Pointer scheme:
//! - "Use the app" tool on the live app: clicks, drags, scrolling and touchpad gestures over
//!   the image go to the app.
//! - Other tools, or an image open in the editor: primary-button input over the view is the
//!   editor's (drawing on the live app starts a draft on a screenshot); the Move tool's
//!   drags on empty space pan.
//! - Scrolling that doesn't go to the app moves the view, per [`NavMode`]. Ctrl+scroll
//!   always zooms at the pointer.
//! - Middle-drag anywhere, or a drag outside the image: pan. A middle click without
//!   movement still reaches the app (sent on release).
//!
//! Zoom is in host physical pixels per image pixel, so 100% is pixel-exact at any scale.

pub mod demo;
pub mod flight;
pub mod input;
pub mod sizing;
mod thumbs;
mod view;
pub mod viz;
pub mod workspace;

use std::{collections::HashMap, sync::Arc};

use egui::{CursorIcon, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Vec2, pos2, vec2};
use guilab_chat::Item;
use guilab_editor::Tool;
use guilab_image::{Asset, AssetId, Doc, Fonts, Image, ImageId, Store};
use guilab_kit::{MenuBar, Theme, ThemePref, WindowButton};

use crate::{
    input::{CursorImage, HostCursor, HostInput, WindowOp},
    sizing::Resizing,
};
use thumbs::ThumbSource;
use workspace::{Att, Open, Request, Workspace};

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
    pub agent: Option<guilab_chat::Status>,
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
    /// A drawing started on the live app: answer with [`ViewerUi::captured`] (a screenshot
    /// of the whole output, no cursor, into the store).
    Capture { session: String, ticket: u64 },
    /// The human sent a message: text, the drafts to freeze into images (with their
    /// parents), and the past image open in the editor, if any (context, not attached).
    Message { session: String, text: String, drafts: Vec<(Option<ImageId>, Doc)>, viewing: Option<ImageId> },
    /// The human started typing a message: capture what they see now (the screen a message
    /// without images refers to), for the message they send.
    Typing { session: String },
    /// Change the session's output (Session menu, handles, "Lock to window").
    Resize { session: String, size: Option<(u32, u32)>, scale: Option<f64> },
    /// File > Save: ask where, then save `png` (the open image, rendered and marked), or a
    /// fresh screenshot of the session if None. Report back with [`ViewerUi::notices`].
    Save { session: String, png: Option<Vec<u8>> },
    /// File > Insert image: ask for a file, then answer with [`ViewerUi::inserted`].
    Insert { session: String },
}

/// UI state of one session.
struct PerSession {
    key: String,
    /// The live app's zoom and pan.
    zoom: Zoom,
    ws: Workspace,
    /// Next event seq the feed wants (see [`ViewerUi::feed_next`]).
    feed_next: u64,
    viz: crate::viz::Overlay,
    sizing: Sizing,
}

/// Session sizing state (Session menu).
#[derive(Default)]
struct Sizing {
    /// "Lock to window": the session's logical size follows the view; no zoom/pan.
    follow: bool,
    /// The size before "Lock to window", shown (disabled) while locked and restored after.
    before_follow: Option<(u32, u32)>,
    /// The last size "follow" asked for, and when (egui time).
    sent: Option<((u32, u32), f64)>,
    /// The Session menu's size and scale fields (synced to the session while it's closed).
    edit_size: (u32, u32),
    edit_scale: f64,
}

impl PerSession {
    fn new(key: &str, fonts: &Arc<Fonts>) -> PerSession {
        let id = egui::Id::new(("session", key));
        PerSession { key: key.into(), zoom: Zoom::default(), ws: Workspace::new(id, fonts.clone()), feed_next: 0, viz: Default::default(), sizing: Sizing::default() }
    }
}

/// Zoom and pan of a view (the live app, or an open image).
#[derive(Clone, Copy, Debug)]
pub struct Zoom {
    /// Follow the window: fit, at most 100% (or the content's logical size).
    fit: bool,
    /// Host physical pixels per image pixel.
    zoom: f32,
    /// Image center minus view center, in points.
    pan: Vec2,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom { fit: true, zoom: 1.0, pan: Vec2::ZERO }
    }
}

/// Chat panel ids (bottom, right); the view leaves presses on their dividers to egui.
const CHAT_PANELS: [&str; 2] = ["chat", "chat-side"];
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 32.0;
/// Middle-button movement (points) that turns a click into a pan.
const DRAG_SLOP: f32 = 4.0;
/// Session pixels per wheel notch, at scale 1 (matches agent `scroll`).
const WHEEL_PX: f64 = 15.0;

/// The viewer's UI state. The server keeps it while the window is hidden (drafts, the open
/// image, chats and View menu choices survive hide/show); it holds no GPU state but
/// textures of the egui context it last drew in, which [`ViewerUi::detach`] drops.
pub struct ViewerUi {
    fonts: Arc<Fonts>,
    /// The clock egui time counts from (kept across windows, so stored times stay valid).
    pub start: std::time::Instant,
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
    /// Host physical pixels per image pixel in the last frame.
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
    notices: Arc<std::sync::Mutex<Vec<String>>>,
    notice: Option<(String, f64)>,
    pub chat_place: ChatPlace,
    /// The app's cursor image, when the pointer is over an app that gets input and the app
    /// uses an image cursor (egui's cursor is then `Default`; the viewer shows this instead).
    app_image: Option<Arc<CursorImage>>,
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
    /// `fonts`: the image text fonts (the server's, shared with its renderer).
    pub fn new(fonts: Arc<Fonts>) -> ViewerUi {
        ViewerUi {
            fonts,
            start: std::time::Instant::now(),
            current: None,
            sessions: HashMap::new(),
            held_buttons: Vec::new(),
            held_keys: Vec::new(),
            held_in: None,
            middle: None,
            panning: None,
            image_rect: None,
            pixel_ratio: 1.0,
            pointer: None,
            pointer_in: None,
            gestures: false,
            nav: NavSetting::default(),
            gesture: None,
            blocked_at: None,
            blocked_tries: 0,
            notices: Default::default(),
            notice: None,
            chat_place: ChatPlace::default(),
            app_image: None,
            last_view: None,
            resizing: None,
            held_touches: Vec::new(),
            touch_area: None,
            csd: false,
            maximized: false,
            window_ops: Vec::new(),
            theme: ThemePref::default(),
        }
    }

    /// The window is going away: drop what belongs to its egui context (textures) and to
    /// the pointer in it. Unsent drafts, the open image and the chats stay.
    pub fn detach(&mut self) {
        for p in self.sessions.values_mut() {
            p.ws.flights.finish();
            p.ws.thumbs.clear();
            p.ws.editor.forget_textures();
        }
        self.panning = None;
        self.resizing = None;
        self.pointer = None;
        self.pointer_in = None;
        self.app_image = None;
        self.image_rect = None;
        self.touch_area = None;
    }

    /// The current session's drafts: each one's element count (for tests).
    pub fn drafts(&self) -> Vec<usize> {
        self.current.as_deref().and_then(|c| self.sessions.get(c)).map_or(Vec::new(), |p| p.ws.pending.iter().map(|d| d.doc.body().elements.len()).collect())
    }

    /// The selected session's workspace (tests, demos).
    pub fn workspace(&self) -> Option<&Workspace> {
        self.current.as_deref().and_then(|c| self.sessions.get(c)).map(|p| &p.ws)
    }

    pub fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.current.as_deref().and_then(|c| self.sessions.get_mut(c)).map(|p| &mut p.ws)
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

    /// Whether a touch starting at `p` is the app's (its image, no editing, no menu open; as
    /// of the last frame). Blocked ones go there too (and are dropped).
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
        let fonts = &self.fonts;
        let p = self.sessions.entry(name.to_string()).or_insert_with(|| PerSession::new(key, fonts));
        if p.key != key {
            *p = PerSession::new(key, fonts);
        }
        p
    }

    /// Where other threads post messages for the human (shown as a toast).
    pub fn notices(&self) -> Arc<std::sync::Mutex<Vec<String>>> {
        self.notices.clone()
    }

    fn notify(&self, text: String) {
        if let Ok(mut n) = self.notices.lock() {
            n.push(text);
        }
    }

    /// The next session event seq its feed wants (0 = it has nothing yet).
    pub fn feed_next(&mut self, name: &str, key: &str) -> u64 {
        self.per(name, key).feed_next
    }

    /// Appends feed items for a session; `next`: the seq after the last event they cover.
    pub fn push_feed(&mut self, name: &str, key: &str, items: Vec<Item<ImageId>>, next: u64) {
        let p = self.per(name, key);
        for i in items {
            let Item { author, time, text, attachments, faint } = i;
            p.ws.chat.push(Item { author, time, text, attachments: attachments.into_iter().map(Att::Image).collect(), faint });
        }
        p.feed_next = next;
    }

    /// The screenshot an [`Action::Capture`] asked for, or why there's none.
    pub fn captured(&mut self, name: &str, ticket: u64, img: Result<Arc<Image>, String>, store: &Store) {
        if let Some(p) = self.sessions.get_mut(name) {
            p.ws.captured(ticket, img, store, p.zoom);
        }
    }

    /// The picture an [`Action::Insert`] asked for (an asset the store issued an id for).
    pub fn inserted(&mut self, name: &str, picture: Result<(AssetId, Arc<Asset>), String>) {
        match picture {
            Ok((id, asset)) => {
                if let Some(p) = self.sessions.get_mut(name) {
                    p.ws.insert(id, asset, p.zoom);
                }
            }
            Err(e) => self.notify(e),
        }
    }

    /// Whether keys belong to egui even without a focused text field (the editor has an
    /// image open or text being typed, or a menu is open).
    pub fn keys_to_egui(&self, ctx: &egui::Context) -> bool {
        MenuBar::is_open(ctx, MENU_BAR) || self.current.as_deref().and_then(|c| self.sessions.get(c)).is_some_and(|p| p.ws.wants_keys)
    }

    /// Draws the whole viewer. `inputs`: this frame's host input in order; raw keys in it
    /// are meant for the session (egui doesn't want keyboard input). `keyboard_left`: the
    /// host keyboard focus left the window. `store`: the server's images (read only).
    pub fn show(&mut self, ui: &mut egui::Ui, sessions: &[SessionView], inputs: Vec<HostInput>, keyboard_left: bool, store: &Store) -> Vec<Action> {
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
        self.menu_bar(ui, sessions, store, &mut out);

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
        p.ws.thumbs.tick(now);
        p.ws.flights.begin(now, &t.motion);

        egui::Panel::left("tools").resizable(false).show_separator_line(false).exact_size(t.size.toolbar_w).frame(egui::Frame::new().fill(t.colors.panel)).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(t.space.sm);
                if p.ws.editor.toolbar(ui) == Some(Tool::Interact) {
                    p.ws.close(p.zoom);
                }
            });
        });
        let panel = match self.chat_place {
            ChatPlace::Off => None,
            ChatPlace::Bottom => Some(egui::Panel::bottom(CHAT_PANELS[0]).default_size(t.size.chat_h[0]).size_range(t.size.chat_h[1]..=t.size.chat_h[2])),
            ChatPlace::Right => Some(egui::Panel::right(CHAT_PANELS[1]).default_size(t.size.chat_w[0]).size_range(t.size.chat_w[1]..=t.size.chat_w[2])),
        };
        if panel.is_none() {
            p.ws.slots.clear();
        }
        if let Some(panel) = panel {
            p.ws.chat.set_status(s.agent.clone());
            let ws = &mut p.ws;
            let cout = panel.resizable(true).show_separator_line(false).frame(egui::Frame::new().fill(t.colors.panel)).show(ui, |ui| {
                // Fill the panel, or it shrinks to its content (egui keeps the content's size).
                ui.set_min_size(ui.available_size());
                let keys = ws.pending_keys();
                let mut src = ThumbSource { store, fonts: &self.fonts, drafts: &ws.pending, shelf: &ws.shelf, open: &ws.open, flights: &ws.flights, cache: &mut ws.thumbs, now };
                ws.chat.show(ui, &keys, &mut src)
            });
            let cout = cout.inner;
            ws.slots = cout.slots;
            ws.feed_clip = cout.feed_clip;
            ws.composer = cout.composer;
            for ev in cout.events {
                if ev == guilab_chat::ChatEvent::Started {
                    out.push(Action::Typing { session: s.name.to_string() });
                }
                if let Some(text) = ws.chat_event(ev, store, p.zoom) {
                    let viewing = ws.viewing();
                    let drafts = ws.send(p.zoom).into_iter().map(|d| (d.parent, d.doc)).collect();
                    out.push(Action::Message { session: s.name.to_string(), text, drafts, viewing });
                }
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let view = ui.available_rect_before_wrap();
            let resp = ui.allocate_rect(view, Sense::hover());
            self.last_view = Some(view);
            self.follow_window(s, &mut p.sizing, view, now, &mut out, ui.ctx());
            self.view(ui, s, &mut p, view, resp.contains_pointer() && !menu_was_open, inputs, menu_was_open, &mut out);
            if let (Some(log), Some(img)) = (s.viz, self.image_rect)
                && !p.ws.image_open()
            {
                p.viz.update(log, now, &t.overlay);
                let light = s.under_pointer.is_some_and(|[r, g, b]| 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > t.overlay.light_above);
                p.viz.paint(ui, view, img, (s.size.0 as f32, s.size.1 as f32), now, light);
            }
            // Toasts stack above the editor's own.
            if !s.blocked() {
                self.blocked_at = None;
            }
            let secs = t.motion.toast_secs;
            if let Some(at) = self.blocked_at {
                let mut stop = false;
                let r = guilab_kit::toast(ui, egui::Id::new("blocked-toast"), egui::Id::new(VIEW_TOASTS), view, |ui| {
                    ui.label("Agent using app");
                    stop = ui.button("Stop").on_hover_text("Take the app back from the agent").clicked();
                });
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
            for n in p.ws.notes.drain(..) {
                self.notify(n);
            }
            if let Ok(mut n) = self.notices.lock()
                && let Some(text) = n.pop()
            {
                n.clear();
                self.notice = Some((text, now));
            }
            if let Some((text, at)) = &self.notice {
                guilab_kit::toast(ui, egui::Id::new("notice-toast"), egui::Id::new(VIEW_TOASTS), view, |ui| {
                    ui.label(text.as_str());
                });
                if now - at > secs {
                    self.notice = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(secs - (now - at) + 0.05));
                }
            }
        });
        p.ws.paint_motion(ui.ctx(), store, &t);
        self.host_keys(ui.ctx(), &mut p, store);
        // "Use the app" closes the editor (also when a drawing's screenshot arrived after it
        // was picked).
        if p.ws.editor.tool == Tool::Interact && p.ws.image_open() {
            p.ws.close(p.zoom);
        }
        for r in p.ws.requests.drain(..) {
            match r {
                Request::Capture { ticket } => out.push(Action::Capture { session: s.name.to_string(), ticket }),
            }
        }
        self.sessions.insert(s.name.to_string(), p);
        out
    }

    /// Undo/redo and Esc (close the open image), when the keys are the viewer's and no
    /// text field has them.
    fn host_keys(&mut self, ctx: &egui::Context, p: &mut PerSession, store: &Store) {
        if ctx.egui_wants_keyboard_input() || !(p.ws.wants_keys || p.ws.image_open()) {
            return;
        }
        let (undo, redo, esc) = ctx.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
            let undo = i.consume_key(Modifiers::COMMAND, Key::Z);
            let esc = !p.ws.editor.busy() && i.consume_key(Modifiers::NONE, Key::Escape);
            (undo, redo, esc)
        });
        if undo {
            p.ws.undo(store, p.zoom);
        }
        if redo {
            p.ws.redo(p.zoom);
        }
        if esc && p.ws.image_open() {
            p.ws.close(p.zoom);
            p.ws.editor.tool = Tool::Interact;
        }
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

    /// "Lock to window": asks for the output size whose logical size fills the view,
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
    fn menu_bar(&mut self, ui: &mut egui::Ui, sessions: &[SessionView], store: &Store, out: &mut Vec<Action>) {
        let cur = self.current.clone();
        let mut chat_place = self.chat_place;
        let sv = cur.as_deref().and_then(|c| sessions.iter().find(|s| s.name == c));
        let last_view = self.last_view;
        let menu_open = MenuBar::is_open(ui.ctx(), MENU_BAR);
        let mut theme = self.theme;
        let t = Theme::get(ui.ctx());
        let fonts = self.fonts.clone();
        let p = sv.map(|s| self.per(s.name, s.key));
        let free = egui::Panel::top("menu").show_separator_line(false).frame(egui::Frame::new().fill(t.colors.panel)).show(ui, |ui| {
            MenuBar::new(MENU_BAR).show(ui, |bar| {
                let Some(p) = p else {
                    for m in ["File", "Edit", "Session", "View"] {
                        bar.menu_enabled(m, false, |_| {});
                    }
                    return;
                };
                let name = cur.clone().unwrap_or_default();
                bar.menu("File", |m| {
                    if m.item("Insert image…", None, true) {
                        out.push(Action::Insert { session: name.clone() });
                    }
                    m.gap();
                    let label = if p.ws.image_open() { "Save image…" } else { "Save screenshot…" };
                    if m.item(label, None, true) {
                        let png = match (&p.ws.open, p.ws.doc().filter(|_| p.ws.image_open())) {
                            (Open::Past { image, .. }, Some(d)) => render_png(d, &fonts, Some(*image)),
                            (_, Some(d)) => render_png(d, &fonts, None),
                            _ => Ok(None),
                        };
                        match png {
                            Ok(png) => out.push(Action::Save { session: name.clone(), png }),
                            Err(e) => p.ws.notes.push(format!("Couldn't render the image: {e}")),
                        }
                    }
                });
                bar.menu("Edit", |m| {
                    if m.item("Undo", Some("Ctrl+Z"), p.ws.can_undo()) {
                        p.ws.undo(store, p.zoom);
                    }
                    if m.item("Redo", Some("Ctrl+Shift+Z"), p.ws.can_redo()) {
                        p.ws.redo(p.zoom);
                    }
                });
                if let Some(sv) = sv {
                    bar.menu("Session", |m| session_menu(m, sv, &mut p.sizing, &mut p.zoom, last_view, menu_open, &name, out));
                }
                bar.menu("View", |m| {
                    let doc = p.ws.image_open();
                    let fixed = p.sizing.follow && !doc;
                    let z = if doc { &mut p.ws.doc_zoom } else { &mut p.zoom };
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
                                let mut pct = z.zoom * 100.0;
                                let r = ui.add_sized(vec2(step.x * 2.0, step.y), egui::DragValue::new(&mut pct).range(MIN_ZOOM * 100.0..=MAX_ZOOM * 100.0).max_decimals(0).suffix("%").speed(1.0).update_while_editing(false));
                                if r.changed() {
                                    z.zoom = pct / 100.0;
                                    z.fit = false;
                                }
                                if ui.add(egui::Button::new("−").min_size(step)).on_hover_text("Zoom out").clicked() {
                                    z.zoom = (z.zoom / 1.25).max(MIN_ZOOM);
                                    z.fit = false;
                                }
                            });
                        },
                    );
                    if m.item("Zoom 1:1", None, !fixed && (z.fit || z.zoom != 1.0)) {
                        *z = Zoom { fit: false, zoom: 1.0, pan: Vec2::ZERO };
                    }
                    if m.item("Fit to window", None, !z.fit && !fixed) {
                        *z = Zoom::default();
                    }
                    m.gap();
                    m.heading("Chat");
                    for (place, label) in [(ChatPlace::Off, "Off"), (ChatPlace::Bottom, "Bottom"), (ChatPlace::Right, "Right side")] {
                        if m.radio(chat_place == place, label) {
                            chat_place = place;
                        }
                    }
                    m.gap();
                    m.heading("Theme");
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

}

/// The open image as a PNG (marked as a rendering of `id`, or of a draft).
fn render_png(doc: &Doc, fonts: &Fonts, id: Option<ImageId>) -> Result<Option<Vec<u8>>, String> {
    let r = guilab_image::render(doc.body(), doc, fonts, &Default::default()).map_err(|e| e.to_string())?;
    Ok(Some(r.png(id)))
}

/// The menu bar's id.
const MENU_BAR: &str = "menubar";
/// The toast stack over the session view.
const VIEW_TOASTS: &str = "view-toasts";

/// The window edge `p` is on, if within `border` of it.
pub fn window_edge(screen: Rect, border: f32, p: Pos2) -> Option<(i8, i8)> {
    let side = |v: f32, lo: f32, hi: f32| if v < lo + border { -1 } else if v > hi - border { 1 } else { 0 };
    let e = (side(p.x, screen.min.x, screen.max.x), side(p.y, screen.min.y, screen.max.y));
    (e != (0, 0)).then_some(e)
}

/// Scale steps in the Session menu's − / + buttons.
const SCALE_STEP: f64 = 0.25;
const SCALE_RANGE: std::ops::RangeInclusive<f64> = 0.25..=8.0;

/// The Session menu: scale, size, resize to / lock to the window.
#[allow(clippy::too_many_arguments)]
fn session_menu(m: &mut guilab_kit::menu::MenuUi, s: &SessionView, sz: &mut Sizing, z: &mut Zoom, view: Option<Rect>, menu_open: bool, name: &str, out: &mut Vec<Action>) {
    // Opening (or not editable): show the session's current values. Locked to the window, the
    // size field keeps the size from before.
    if !menu_open || s.locked {
        sz.edit_scale = s.scale;
        sz.edit_size = sz.before_follow.filter(|_| sz.follow).unwrap_or(s.size);
    }
    let free = !s.locked;
    let resize = |out: &mut Vec<Action>, size: Option<(u32, u32)>, scale: Option<f64>| out.push(Action::Resize { session: name.to_string(), size, scale });
    let t = Theme::get(m.ui.ctx());
    let step = vec2(t.size.menu_item_h, t.size.menu_item_h - t.space.sm);
    m.custom(
        |ui| {
            ui.add_enabled(free, egui::Label::new("Scale"));
        },
        |ui| {
            ui.add_enabled_ui(free, |ui| {
                let cur = sz.edit_scale;
                let mut pick = None;
                // Right to left: + value −.
                if ui.add(egui::Button::new("+").min_size(step)).on_hover_text("Larger").clicked() {
                    pick = Some(((cur / SCALE_STEP).floor() + 1.0) * SCALE_STEP);
                }
                let r = ui.add_sized(vec2(step.x * 2.0, step.y), egui::DragValue::new(&mut sz.edit_scale).range(SCALE_RANGE).speed(0.01).max_decimals(2).update_while_editing(false));
                if (r.changed() && !r.dragged()) || r.drag_stopped() {
                    pick = Some(sz.edit_scale);
                }
                if ui.add(egui::Button::new("−").min_size(step)).on_hover_text("Smaller").clicked() {
                    pick = Some(((cur / SCALE_STEP).ceil() - 1.0) * SCALE_STEP);
                }
                if let Some(sc) = pick.map(|sc| sc.clamp(*SCALE_RANGE.start(), *SCALE_RANGE.end())).filter(|sc| *sc != s.scale) {
                    sz.edit_scale = sc;
                    // Following the window keeps the logical size; otherwise the pixel size.
                    let size = sz.follow.then_some(view).flatten().map(|v| sizing::fit_pixels(v, sc));
                    resize(out, size, Some(sc));
                }
            });
        },
    );
    let edit_size = free && !sz.follow;
    m.custom(
        |ui| {
            ui.add_enabled(edit_size, egui::Label::new("Size"));
        },
        |ui| {
            ui.add_enabled_ui(edit_size, |ui| {
                // Right to left: W × H. Applied on Enter, leaving the field or ending a drag;
                // Esc restores the old value.
                let range = sizing::MIN_PX..=sizing::MAX_PX;
                let field = |ui: &mut egui::Ui, v: &mut u32| {
                    let r = ui.add(egui::DragValue::new(v).range(range.clone()).speed(4).update_while_editing(false)).on_hover_text("Output size in pixels");
                    (r.changed() && !r.dragged()) || r.drag_stopped()
                };
                let h = field(ui, &mut sz.edit_size.1);
                ui.label("×");
                let w = field(ui, &mut sz.edit_size.0);
                if (w || h) && sz.edit_size != s.size {
                    resize(out, Some(sz.edit_size), None);
                }
            });
        },
    );
    m.gap();
    if m.item("Resize to window", None, free && !sz.follow && view.is_some()) {
        let size = view.map(|v| sizing::fit_pixels(v, s.scale));
        if let Some(size) = size {
            sz.edit_size = size;
        }
        resize(out, size, None);
        *z = Zoom::default();
    }
    if m.check(sz.follow, "Lock to window", true) {
        sz.follow = !sz.follow;
        if sz.follow {
            sz.before_follow = Some(s.size);
        } else if let Some(before) = sz.before_follow.take() {
            sz.edit_size = before;
            if free && before != s.size {
                resize(out, Some(before), None);
            }
        }
        *z = Zoom::default();
    }
    if s.locked {
        m.note("The agent is using the app; its size is frozen");
    }
}

#[cfg(test)]
mod tests;
