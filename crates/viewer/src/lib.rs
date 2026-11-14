//! guilab's viewer window.
//!
//! - [`source`], [`host`], [`gfx`]: generic. egui in a window on a host Wayland display,
//!   drawn by a smithay `GlesRenderer` that lives on a render node (not on the window), so
//!   the window can come and go while the renderer stays. Nothing guilab-specific.
//! - [`ui`] and [`Viewer`]: guilab's UI on top (tabs, session view, lock controls, and the
//!   drawing toolbar and chat panel from `guilab-markup`).
//!
//! The server owns sessions; each draw it passes [`SessionFrame`]s in and applies the
//! [`ui::Action`]s that come back.

pub mod gfx;
pub mod host;
pub mod source;
pub mod ui;

use std::{
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

use smithay::backend::{
    allocator::gbm::GbmDevice,
    drm::{DrmDeviceFd, DrmNode},
    renderer::gles::GlesRenderer,
};

pub use egui;
pub use guilab_markup::{self as markup, Annotated, FeedItem, Who};
pub use host::{BufferKind, Connected, HostInput, HostState, connect};
pub use source::{HostEvent, HostSource};
pub use ui::{Action, Input};

/// A session for one draw.
pub struct SessionFrame<'a> {
    pub name: &'a str,
    /// Unique per session instance (texture cache key).
    pub key: &'a str,
    /// GL texture name and pixel size of its latest frame.
    pub texture: Option<(u32, (i32, i32))>,
    /// Changes whenever the frame's content does.
    pub seq: u64,
    pub size: (u32, u32),
    pub scale: f64,
    pub locked: bool,
    pub shared: bool,
    pub cursor: egui::CursorIcon,
}

pub struct Viewer {
    pub host: HostState,
    gfx: gfx::Gfx,
    ui: ui::ViewerUi,
    start: Instant,
    next_repaint: Option<Instant>,
    /// Signature of what the last frame showed (sessions, locks, current frame seq).
    shown: u64,
    /// Texture updates egui produced that haven't been painted yet.
    pending: egui::TexturesDelta,
    /// Frames presented.
    pub frames: u64,
}

impl Viewer {
    /// Creates the window. `gbm`/`node` belong to `renderer` (for dmabuf presentation).
    pub fn new(
        c: &Connected,
        renderer: &mut GlesRenderer,
        gbm: Option<GbmDevice<DrmDeviceFd>>,
        node: Option<DrmNode>,
        force_shm: bool,
    ) -> anyhow::Result<Viewer> {
        let host = HostState::new(c, "guilab", "guilab", gbm, node, force_shm)?;
        let gfx = gfx::Gfx::new(renderer)?;
        Ok(Viewer {
            host,
            gfx,
            ui: Default::default(),
            start: Instant::now(),
            next_repaint: None,
            shown: 0,
            pending: Default::default(),
            frames: 0,
        })
    }

    pub fn destroy(self, renderer: &mut GlesRenderer) {
        self.gfx.destroy(renderer);
    }

    fn signature(&self, sessions: &[SessionFrame]) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let cur = self.ui.current();
        for s in sessions {
            (s.name, s.locked, s.shared, s.size, s.scale.to_bits(), s.cursor as u8).hash(&mut h);
            if Some(s.name) == cur || cur.is_none() {
                s.seq.hash(&mut h);
            }
        }
        h.finish()
    }

    /// Whether to run the UI now: the host is ready for a frame and something changed, or
    /// there's new input (handled right away even if the frame has to wait).
    pub fn wants_draw(&mut self, now: Instant, sessions: &[SessionFrame]) -> bool {
        self.host.tick_repeat(now);
        let changed = self.host.dirty || self.next_repaint.is_some_and(|t| now >= t) || self.signature(sessions) != self.shown;
        (self.host.can_draw() && changed) || self.host.has_input()
    }

    /// The next event seq the session's feed wants (0 = nothing shown yet).
    pub fn feed_next(&mut self, name: &str, key: &str) -> u64 {
        self.ui.feed_next(name, key)
    }

    /// Adds feed items (events before `next`) for a session.
    pub fn push_feed(&mut self, name: &str, key: &str, items: Vec<FeedItem>, next: u64) {
        let any = !items.is_empty();
        self.ui.push_feed(name, key, items, next);
        self.host.dirty |= any;
    }

    /// Answers [`Action::Freeze`].
    pub fn set_screenshot(&mut self, name: &str, image: egui::ColorImage) {
        self.ui.set_screenshot(name, image);
        self.host.dirty = true;
    }

    /// Releases the human's held buttons and keys (on hide).
    pub fn release_all(&mut self) -> Vec<Action> {
        self.ui.release_all()
    }

    /// Runs the UI and presents a frame if the host is ready. Returns the human's actions.
    pub fn draw(&mut self, renderer: &mut GlesRenderer, sessions: &[SessionFrame], now: Instant) -> anyhow::Result<Vec<Action>> {
        let inputs = self.host.take_input();
        let keyboard_left = std::mem::take(&mut self.host.keyboard_left);
        // Keys go to egui only while it wants them (a focused text field); otherwise to the
        // session, and egui mustn't see them (Tab/Space would act on its buttons).
        let to_session = !self.gfx.ctx.egui_wants_keyboard_input() && !self.ui.keys_to_egui();
        let events: Vec<egui::Event> = inputs
            .iter()
            .filter_map(|i| match i {
                HostInput::Egui(egui::Event::Key { .. } | egui::Event::Text(_)) if to_session => None,
                HostInput::Egui(e) => Some(e.clone()),
                HostInput::Key(_) => None,
            })
            .collect();
        // The UI gets everything in order (pointer and keys interleaved), keys only if
        // they're the session's.
        let mut inputs: Vec<HostInput> = inputs.into_iter().filter(|i| to_session || !matches!(i, HostInput::Key(_))).collect();

        let cur = self.ui.current().or(sessions.first().map(|s| s.name)).map(str::to_string);
        let nearest = self.ui.nearest();
        let mut tex = None;
        if let Some(s) = sessions.iter().find(|s| Some(s.name) == cur.as_deref())
            && let Some((id, size)) = s.texture
        {
            let gfx = &mut self.gfx;
            tex = renderer.with_context(|_| gfx.mirror(s.key, id, size, s.seq, nearest)).ok();
        }
        self.gfx.retain_mirrors(|k| sessions.iter().any(|s| s.key == k && Some(s.name) == cur.as_deref()));
        let views: Vec<ui::SessionView> = sessions
            .iter()
            .map(|s| ui::SessionView {
                name: s.name,
                key: s.key,
                texture: if Some(s.name) == cur.as_deref() { tex } else { None },
                size: s.size,
                scale: s.scale,
                locked: s.locked,
                shared: s.shared,
                cursor: s.cursor,
            })
            .collect();

        let (w, h) = self.host.size.unwrap_or((1, 1));
        let ppp = self.host.scale as f32;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32))),
            time: Some(now.duration_since(self.start).as_secs_f64()),
            modifiers: self.host.modifiers,
            focused: self.host.focused,
            events,
            max_texture_side: Some(self.gfx.max_texture_side()),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let mut actions = Vec::new();
        let ui_state = &mut self.ui;
        let out = self.gfx.ctx.run_ui(raw, |ui| {
            // Input is consumed in the first pass only (egui may run several).
            actions.extend(ui_state.show(ui, &views, std::mem::take(&mut inputs), keyboard_left));
        });
        if self.ui.current().map(str::to_string) != cur || self.ui.nearest() != nearest {
            self.host.dirty = true; // the mirror is stale (new tab, or its filter changed)
        }
        self.host.set_cursor(out.platform_output.cursor_icon);
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        self.next_repaint = (delay < Duration::from_secs(3600)).then(|| now + delay);
        self.pending.append(out.textures_delta);
        let ppp = self.gfx.ctx.pixels_per_point();
        let prims = self.gfx.ctx.tessellate(out.shapes, ppp);
        let gfx = &mut self.gfx;
        let pending = &self.pending;
        let mut painted = false;
        let drawn = self.host.present(renderer, |size| {
            gfx.paint(size, ppp, &prims, pending);
            painted = true;
        })?;
        if painted {
            self.pending = Default::default();
        }
        if drawn {
            self.frames += 1;
            self.shown = self.signature(sessions);
        } else {
            // The UI ran on input without presenting; its new state still has to be shown.
            self.host.dirty = true;
        }
        Ok(actions)
    }
}
