//! The viewer window (`show`/`hide`/`server --show`): connecting to the host display off
//! the main thread, drawing from the tick, applying the human's actions to sessions.
//!
//! Showing and hiding never touch sessions beyond reading their frames and injecting the
//! human's input. Host failures (disconnect, protocol error, closed window) hide the viewer.

use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant},
};

use calloop::{RegistrationToken, channel};
use guilab_compositor::{AppCursor, InputEvent, Session, render::Renderer};
use guilab_image::ImageId;
use guilab_proto::{EventData, Response, ViewerInfo, text::Fmt};
use guilab_viewer::{
    Action, BufferKind, Connected, CursorImage, FeedItem, HostCursor, HostEvent, HostSource, Input, SessionFrame, Status, Viewer,
    egui::{ColorImage, CursorIcon as EguiCursor},
};
use smithay::{
    backend::renderer::gles::GlesRenderer,
    backend::input::AxisSource,
    input::{keyboard::Keycode, pointer::CursorIcon},
};

use crate::{Server, human::MsgImage, images::Images};

/// The agent's status line notes it's been quiet after this many seconds.
const QUIET_NOTE: u64 = 120;

/// Feed items loaded when the viewer first shows a session.
const FEED_BACKLOG: usize = 300;

/// How long `show` waits for the window's first frame.
const SHOW_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) type ConnectResult = (u64, Result<Connected, String>);

#[derive(Default)]
pub(crate) enum ViewerState {
    #[default]
    Hidden,
    /// Connecting on a helper thread; `attempt` tells its result from stale ones.
    Connecting { attempt: u64, display: PathBuf, waiters: Vec<UnixStream>, deadline: Instant },
    Shown(Box<Shown>),
}

pub(crate) struct Shown {
    viewer: Viewer,
    /// The viewer's own software GLES, when sessions render with pixman.
    soft: Option<Box<GlesRenderer>>,
    /// pixman sessions' latest frames as images, by tag: (frame_seq, image).
    cpu_frames: std::collections::HashMap<String, (u64, std::sync::Arc<ColorImage>)>,
    /// None once the source removed itself (host lost).
    token: Option<RegistrationToken>,
    display: PathBuf,
    /// `show` requests waiting for the first frame.
    waiters: Vec<UnixStream>,
    deadline: Instant,
    clip: ClipBridge,
    /// Human presses, keys, scrolls, touches and gestures dropped because the session was
    /// locked (not motion, leaves or updates); the UI drops
    /// most itself (`Viewer::blocked_tries`), these got past it before it knew.
    dropped: u64,
    /// Per session (by tag): the app cursor as of `Session::cursor_seq`.
    cursors: std::collections::HashMap<String, (u64, HostCursor)>,
    next_cursor_id: u64,
}

/// Text clipboard bridging between the host and the shown session: host changes go to
/// the session; the session's changes go to the host while the viewer has keyboard focus
/// (so an agent's `clip set` or copy doesn't overwrite the human's desktop clipboard
/// behind their back).
#[derive(Default)]
struct ClipBridge {
    /// Per session (tag): the `clip_seq` already handled.
    seen: std::collections::HashMap<String, u64>,
    /// The text last moved either way (so it isn't bounced back).
    last: Option<String>,
    /// A session clipboard read in progress (its pipe is read on a thread).
    reading: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl ClipBridge {
    fn bridge(&mut self, viewer: &mut Viewer, session: &mut Session) {
        if let Some(text) = viewer.host.take_clipboard()
            && self.last.as_ref() != Some(&text)
        {
            session.clip_set(text.clone());
            self.seen.insert(session.tag.clone(), session.clip.seq());
            self.last = Some(text);
        }
        let read = self.reading.lock().ok().and_then(|mut r| r.take());
        if let Some(text) = read
            && self.last.as_ref() != Some(&text)
        {
            viewer.host.set_clipboard(text.clone());
            self.last = Some(text);
        }
        let seq = session.clip.seq();
        let old = self.seen.insert(session.tag.clone(), seq);
        // First sight of a session, no change, or the human isn't in the viewer.
        if old.is_none_or(|o| o == seq) || !viewer.host.focused {
            return;
        }
        match session.clip_get() {
            Ok(guilab_compositor::ClipRead::Text(t)) => {
                if self.last.as_ref() != Some(&t) {
                    viewer.host.set_clipboard(t.clone());
                    self.last = Some(t);
                }
            }
            Ok(guilab_compositor::ClipRead::Pipe(fd)) => {
                let out = self.reading.clone();
                let _ = std::thread::Builder::new().name("session-clipboard-read".into()).spawn(move || {
                    use std::io::Read;
                    let mut buf = Vec::new();
                    if std::fs::File::from(fd).take(4 << 20).read_to_end(&mut buf).is_ok()
                        && let Ok(t) = String::from_utf8(buf)
                        && let Ok(mut o) = out.lock()
                    {
                        *o = Some(t);
                    }
                });
            }
            Err(_) => {}
        }
    }
}

impl Shown {
    /// The session's app cursor for the viewer (converted once per change).
    fn cursor(&mut self, session: &mut Session) -> HostCursor {
        let seq = session.cursor_seq();
        if let Some((s, c)) = self.cursors.get(&session.tag)
            && *s == seq
        {
            return c.clone();
        }
        let c = match session.app_cursor() {
            AppCursor::Hidden => HostCursor::Icon(EguiCursor::None),
            AppCursor::Named(i) => HostCursor::Icon(egui_cursor(i)),
            AppCursor::Image(img) => {
                self.next_cursor_id += 1;
                HostCursor::Image(std::sync::Arc::new(CursorImage {
                    id: self.next_cursor_id,
                    width: img.width,
                    height: img.height,
                    rgba: img.rgba.clone(),
                    hotspot: img.hotspot,
                    scale: img.scale,
                }))
            }
        };
        self.cursors.insert(session.tag.clone(), (seq, c.clone()));
        c
    }
}

fn egui_cursor(icon: CursorIcon) -> EguiCursor {
    match icon {
        CursorIcon::ContextMenu => EguiCursor::ContextMenu,
        CursorIcon::Help => EguiCursor::Help,
        CursorIcon::Pointer => EguiCursor::PointingHand,
        CursorIcon::Progress => EguiCursor::Progress,
        CursorIcon::Wait => EguiCursor::Wait,
        CursorIcon::Cell => EguiCursor::Cell,
        CursorIcon::Crosshair => EguiCursor::Crosshair,
        CursorIcon::Text => EguiCursor::Text,
        CursorIcon::VerticalText => EguiCursor::VerticalText,
        CursorIcon::Alias => EguiCursor::Alias,
        CursorIcon::Copy => EguiCursor::Copy,
        CursorIcon::Move => EguiCursor::Move,
        CursorIcon::NoDrop => EguiCursor::NoDrop,
        CursorIcon::NotAllowed => EguiCursor::NotAllowed,
        CursorIcon::Grab => EguiCursor::Grab,
        CursorIcon::Grabbing => EguiCursor::Grabbing,
        CursorIcon::EResize => EguiCursor::ResizeEast,
        CursorIcon::NResize => EguiCursor::ResizeNorth,
        CursorIcon::NeResize => EguiCursor::ResizeNorthEast,
        CursorIcon::NwResize => EguiCursor::ResizeNorthWest,
        CursorIcon::SResize => EguiCursor::ResizeSouth,
        CursorIcon::SeResize => EguiCursor::ResizeSouthEast,
        CursorIcon::SwResize => EguiCursor::ResizeSouthWest,
        CursorIcon::WResize => EguiCursor::ResizeWest,
        CursorIcon::EwResize => EguiCursor::ResizeHorizontal,
        CursorIcon::NsResize => EguiCursor::ResizeVertical,
        CursorIcon::NeswResize => EguiCursor::ResizeNeSw,
        CursorIcon::NwseResize => EguiCursor::ResizeNwSe,
        CursorIcon::ColResize => EguiCursor::ResizeColumn,
        CursorIcon::RowResize => EguiCursor::ResizeRow,
        CursorIcon::AllScroll => EguiCursor::AllScroll,
        CursorIcon::ZoomIn => EguiCursor::ZoomIn,
        CursorIcon::ZoomOut => EguiCursor::ZoomOut,
        _ => EguiCursor::Default,
    }
}

/// The session event for the human's input (output pixels -> logical).
fn to_event(s: &Session, input: Input) -> InputEvent {
    let scale = s.scale();
    match input {
        Input::Motion { x, y } => InputEvent::PointerMotion { pos: s.from_px(x, y) },
        Input::Leave => InputEvent::PointerLeave,
        Input::Button { button, pressed } => InputEvent::PointerButton { button, pressed },
        Input::Scroll { dx, dy, steps, stop } => InputEvent::PointerAxis {
            source: if steps.is_some() { AxisSource::Wheel } else { AxisSource::Finger },
            delta: (dx / scale, dy / scale),
            steps,
            stop,
        },
        Input::Key { code, pressed } => InputEvent::Key { code: Keycode::new(code + 8), pressed },
        Input::PinchBegin { fingers } => InputEvent::PinchBegin { fingers },
        Input::PinchUpdate { dx, dy, scale: s, rotation } => InputEvent::PinchUpdate { delta: (dx / scale, dy / scale).into(), scale: s, rotation },
        Input::PinchEnd { cancelled } => InputEvent::PinchEnd { cancelled },
        Input::SwipeBegin { fingers } => InputEvent::SwipeBegin { fingers },
        Input::SwipeUpdate { dx, dy } => InputEvent::SwipeUpdate { delta: (dx / scale, dy / scale).into() },
        Input::SwipeEnd { cancelled } => InputEvent::SwipeEnd { cancelled },
        Input::TouchDown { id, x, y } => InputEvent::TouchDown { slot: id as u32, pos: s.from_px(x, y) },
        Input::TouchMotion { id, x, y } => InputEvent::TouchMotion { slot: id as u32, pos: s.from_px(x, y) },
        Input::TouchUp { id } => InputEvent::TouchUp { slot: id as u32 },
    }
}

impl Server {
    pub(crate) fn viewer_info(&self) -> Option<ViewerInfo> {
        match &self.viewer {
            ViewerState::Shown(s) if s.viewer.frames > 0 => Some(ViewerInfo {
                display: s.display.clone(),
                buffers: match s.viewer.host.buffer_kind {
                    Some(BufferKind::Dmabuf) => "dmabuf",
                    _ => "shm",
                }
                .into(),
                frames: s.viewer.frames,
                window: s.viewer.host.size.map_or([0, 0], |(w, h)| [w, h]),
                dropped: s.dropped + self.viewer_ui.blocked_tries,
            }),
            _ => None,
        }
    }

    /// The session tab the viewer shows, if it's up.
    pub(crate) fn viewer_current(&self) -> Option<String> {
        match &self.viewer {
            ViewerState::Shown(_) => self.viewer_ui.current().map(str::to_string).or_else(|| self.sessions.keys().next().cloned()),
            _ => None,
        }
    }

    /// `show`: opens the viewer on `display`. Replies (to `stream`) once the first frame is
    /// up; `None` = answered later.
    pub(crate) fn show(&mut self, display: PathBuf, stream: Option<UnixStream>) -> Option<Response> {
        let canon = std::fs::canonicalize(&display).unwrap_or_else(|_| display.clone());
        if !canon.exists() {
            return Some(Response::err(format!("no Wayland display at {}", display.display())));
        }
        if let Some((name, _)) = self.sessions.iter().find(|(_, s)| std::fs::canonicalize(s.session.socket_path()).ok().as_ref() == Some(&canon)) {
            return Some(Response::err(format!(
                "{} is guilab session '{name}' of this server; the viewer can't show inside its own sessions (use another server as host)",
                display.display()
            )));
        }
        match &mut self.viewer {
            ViewerState::Shown(s) if s.display == display => {
                if s.viewer.frames > 0 {
                    return Some(Response::ok(self.viewer_info()));
                }
                s.waiters.extend(stream);
                return None;
            }
            ViewerState::Connecting { display: d, waiters, .. } if *d == display => {
                waiters.extend(stream);
                return None;
            }
            ViewerState::Hidden => {}
            _ => self.hide_viewer(Some("the viewer moved to another display")),
        }
        self.viewer_gen += 1;
        let attempt = self.viewer_gen;
        let tx = self.viewer_tx.clone();
        let path = display.clone();
        // The roundtrip blocks; a stuck host must not freeze sessions.
        let spawned = std::thread::Builder::new().name("viewer-connect".into()).spawn(move || {
            let r = guilab_viewer::connect(&path).map_err(|e| format!("{e:#}"));
            let _ = tx.send((attempt, r));
        });
        if let Err(e) = spawned {
            return Some(Response::err(format!("spawning connect thread: {e}")));
        }
        self.viewer = ViewerState::Connecting { attempt, display, waiters: stream.into_iter().collect(), deadline: Instant::now() + SHOW_TIMEOUT };
        None
    }

    /// A connect attempt finished (on the helper thread).
    pub(crate) fn viewer_connected(&mut self, (attempt, res): ConnectResult) {
        let ViewerState::Connecting { attempt: want, .. } = &self.viewer else { return };
        if *want != attempt {
            return;
        }
        let ViewerState::Connecting { display, waiters, deadline, .. } = std::mem::take(&mut self.viewer) else { unreachable!() };
        let fail = |out: &mut crate::out::Outbox, waiters: Vec<UnixStream>, e: String| {
            tracing::warn!("viewer: {e}");
            for w in waiters {
                out.send(&w, &Response::err(format!("showing the viewer: {e}")));
            }
        };
        let c = match res {
            Ok(c) => c,
            Err(e) => return fail(&mut self.out, waiters, e),
        };
        let force_shm = std::env::var_os("GUILAB_VIEWER_SHM").is_some_and(|v| v == "1");
        // Sessions on pixman: the viewer draws with its own software GLES (llvmpipe).
        let mut soft = None;
        let viewer = {
            let mut gpu = self.gpu.borrow_mut();
            let (gbm, node) = (gpu.gbm.clone(), gpu.node);
            match &mut gpu.renderer {
                Renderer::Gles(r) => Viewer::new(&c, r, gbm, node, force_shm),
                Renderer::Pixman(_) => match guilab_compositor::render::software_gles() {
                    Ok(mut r) => {
                        let v = Viewer::new(&c, &mut r, None, None, true);
                        soft = Some(Box::new(r));
                        v
                    }
                    Err(e) => Err(anyhow::anyhow!("sessions render with pixman and the viewer's software GLES failed: {e:#}")),
                },
            }
        };
        let viewer = match viewer {
            Ok(v) => v,
            Err(e) => return fail(&mut self.out, waiters, format!("{e:#}")),
        };
        let source = HostSource::new(c.conn, c.queue);
        let token = self.handle.insert_source(source, |ev, queue, srv: &mut Server| match ev {
            HostEvent::Dispatch => match &mut srv.viewer {
                ViewerState::Shown(s) => queue.dispatch_pending(&mut s.viewer.host),
                _ => Ok(0),
            },
            HostEvent::Lost(why) => {
                if let ViewerState::Shown(s) = &mut srv.viewer {
                    s.token = None; // the source removes itself
                }
                srv.hide_viewer(Some(&format!("lost the host display ({why})")));
                Ok(0)
            }
        });
        let token = match token {
            Ok(t) => t,
            Err(e) => {
                with_renderer(&self.gpu, soft.as_deref_mut(), |r| viewer.destroy(r));
                return fail(&mut self.out, waiters, e.error.to_string());
            }
        };
        let shown_on = display.display().to_string();
        tracing::info!("viewer shown on {shown_on}");
        self.viewer = ViewerState::Shown(Box::new(Shown { viewer, soft, token: Some(token), display, waiters, deadline, clip: ClipBridge::default(), dropped: 0, cursors: Default::default(), next_cursor_id: 0, cpu_frames: Default::default() }));
    }

    /// Closes the viewer (if any). `why`: an error for `show` requests still waiting.
    pub(crate) fn hide_viewer(&mut self, why: Option<&str>) {
        match std::mem::take(&mut self.viewer) {
            ViewerState::Hidden => {}
            ViewerState::Connecting { waiters, .. } => {
                for w in waiters {
                    self.out.send(&w, &Response::err(why.unwrap_or("hidden before it was shown")));
                }
            }
            ViewerState::Shown(mut s) => {
                if let Some(why) = why {
                    tracing::info!("viewer closed: {why}");
                }
                let releases = self.viewer_ui.release_all();
                self.apply_viewer_actions(releases);
                self.viewer_ui.detach();
                if let Some(t) = s.token.take() {
                    self.handle.remove(t);
                }
                for w in s.waiters.drain(..) {
                    self.out.send(&w, &Response::err(why.unwrap_or("hidden before it was shown")));
                }
                let Shown { viewer, mut soft, .. } = *s;
                with_renderer(&self.gpu, soft.as_deref_mut(), |r| viewer.destroy(r));
            }
        }
    }

    /// From the server tick: timeouts, closing, drawing when something changed.
    pub(crate) fn viewer_tick(&mut self, now: Instant) {
        match &mut self.viewer {
            ViewerState::Hidden => {}
            ViewerState::Connecting { deadline, .. } => {
                if now >= *deadline {
                    self.hide_viewer(Some("timed out connecting to the host display"));
                }
            }
            ViewerState::Shown(s) => {
                if s.viewer.host.closed {
                    return self.hide_viewer(Some("window closed"));
                }
                if s.viewer.frames == 0 && now >= s.deadline {
                    return self.hide_viewer(Some("timed out waiting for the host to show the window"));
                }
                if let Some(tab) = self.viewer_tab.take() {
                    self.viewer_ui.select(&tab);
                    s.viewer.host.dirty = true;
                }
                if let Some(slot) = self.viewer_ui.current().and_then(|c| self.sessions.get_mut(c)) {
                    s.clip.bridge(&mut s.viewer, &mut slot.session);
                }
                for (name, slot) in &self.sessions {
                    let log = &slot.session.log;
                    let next = self.viewer_ui.feed_next(name, &slot.session.tag);
                    if next == log.end() {
                        continue;
                    }
                    let items = feed_items(log, next, &self.images);
                    s.viewer.host.dirty |= !items.is_empty();
                    self.viewer_ui.push_feed(name, &slot.session.tag, items, log.end());
                }
                // Pictures the human picked to insert.
                let picks = std::mem::take(&mut *self.viewer_picks.lock().unwrap_or_else(|e| e.into_inner()));
                for (name, path) in picks {
                    let Some(slot) = self.sessions.get(&name) else { continue };
                    let pic = self.images.picture(&path, slot.group);
                    self.viewer_ui.inserted(&name, pic);
                    s.viewer.host.dirty = true;
                }
                // What's under the pointer decides the overlay's light/dark colors.
                let under: Vec<Option<[u8; 3]>> = self.sessions.values_mut().map(|slot| slot.session.color_under_pointer()).collect();
                let cursors: Vec<HostCursor> = self.sessions.values_mut().map(|slot| s.cursor(&mut slot.session)).collect();
                s.cursors.retain(|tag, _| self.sessions.values().any(|slot| &slot.session.tag == tag));
                // pixman frames reach the viewer as CPU images (the current session's only).
                let mut cpu_image = None;
                if s.soft.is_some() {
                    let cur = self.viewer_ui.current().map(str::to_string).or_else(|| self.sessions.keys().next().cloned());
                    if let Some(slot) = cur.as_deref().and_then(|c| self.sessions.get_mut(c)) {
                        let (tag, seq) = (slot.session.tag.clone(), slot.session.frame_seq);
                        let cached = s.cpu_frames.get(&tag).filter(|(q, _)| *q == seq).map(|(_, i)| i.clone());
                        let img = cached.or_else(|| {
                            let (w, h, mut px) = slot.session.read_rgba().ok()?;
                            px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
                            let img = std::sync::Arc::new(ColorImage::from_rgba_premultiplied([w as usize, h as usize], &px));
                            s.cpu_frames.insert(tag.clone(), (seq, img.clone()));
                            Some(img)
                        });
                        cpu_image = img.map(|i| (tag, i));
                    }
                    s.cpu_frames.retain(|k, _| cpu_image.as_ref().is_some_and(|(t, _)| t == k));
                }
                let frames: Vec<SessionFrame> = self
                    .sessions
                    .iter()
                    .map(|(name, slot)| {
                        let o = slot.session.output_info();
                        SessionFrame {
                            name,
                            key: &slot.session.tag,
                            texture: slot.session.frame.gles_texture().map(|(t, size)| (t.tex_id(), size)),
                            image: cpu_image.as_ref().filter(|(t, _)| *t == slot.session.tag).map(|(_, i)| i.clone()),
                            seq: slot.session.frame_seq,
                            size: (o.width, o.height),
                            scale: o.scale,
                            locked: slot.lock.is_locked(),
                            cursor: HostCursor::Icon(EguiCursor::Default),
                            viz: slot.session.viz(),
                            under_pointer: None,
                            agent: slot.working.as_ref().map(|w| {
                                // Long compiles are normal: quiet only adds a dim note.
                                let quiet = slot.last_agent.elapsed().as_secs();
                                Status { text: w.status.clone(), elapsed: w.since.elapsed().as_secs(), note: (quiet >= QUIET_NOTE).then(|| format!("no guilab activity for {}m", quiet / 60)) }
                            }),
                        }
                    })
                    .zip(under.into_iter().zip(cursors))
                    .map(|(mut f, (u, c))| {
                        f.under_pointer = u;
                        f.cursor = c;
                        f
                    })
                    .collect();
                if !s.viewer.wants_draw(&self.viewer_ui, now, &frames) {
                    return;
                }
                let res = {
                    let (viewer, ui, store) = (&mut s.viewer, &mut self.viewer_ui, &self.images.store);
                    with_renderer(&self.gpu, s.soft.as_deref_mut(), |r| viewer.draw(r, ui, &frames, store, now))
                };
                drop(frames);
                let actions = match res {
                    Ok(a) => a,
                    Err(e) => return self.hide_viewer(Some(&format!("drawing failed: {e:#}"))),
                };
                let waiters = if s.viewer.frames > 0 { std::mem::take(&mut s.waiters) } else { Vec::new() };
                if !waiters.is_empty() {
                    let r = Response::ok(self.viewer_info());
                    for w in waiters {
                        self.out.send(&w, &r);
                    }
                }
                self.apply_viewer_actions(actions);
            }
        }
    }

    /// Applies the human's viewer actions through the human API (`human.rs`).
    pub(crate) fn apply_viewer_actions(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Input { session, input } => {
                    let Some(slot) = self.sessions.get(&session) else { continue };
                    let release = input.is_release();
                    let counts = matches!(input, Input::Button { .. } | Input::Key { .. } | Input::Scroll { .. } | Input::PinchBegin { .. } | Input::SwipeBegin { .. } | Input::TouchDown { .. });
                    let ev = to_event(&slot.session, input);
                    if self.human_input(&session, ev, release).is_err()
                        && counts
                        && let ViewerState::Shown(s) = &mut self.viewer
                    {
                        s.dropped += 1;
                    }
                }
                Action::Revoke { session } => {
                    // Not locked any more (the agent let go meanwhile): nothing to stop.
                    let _ = self.human_revoke(&session);
                }
                Action::Resize { session, size, scale } => {
                    if let Err(e) = self.human_resize(&session, size, scale) {
                        tracing::debug!("viewer: resizing {session}: {e}");
                    }
                }
                Action::Capture { session, ticket } => {
                    let img = self.human_capture(&session);
                    self.viewer_ui.captured(&session, ticket, img, &self.images.store);
                    if let ViewerState::Shown(s) = &mut self.viewer {
                        s.viewer.host.dirty = true;
                    }
                }
                Action::Message { session, text, drafts, viewing } => {
                    let imgs = drafts.into_iter().map(|(parent, doc)| MsgImage::Draft { parent, doc }).collect();
                    if let Err(e) = self.human_message(&session, text, imgs, viewing) {
                        tracing::warn!("viewer: sending message: {e}");
                        self.viewer_ui.notices().lock().map(|mut n| n.push(format!("Couldn't send: {e}"))).ok();
                    }
                }
                Action::Save { session, png } => {
                    let png = match png {
                        Some(p) => p,
                        None => match self.screenshot_png(&session) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!("viewer: screenshot to save: {e}");
                                continue;
                            }
                        },
                    };
                    save_dialog(session, png, self.viewer_ui.notices());
                }
                Action::Insert { session } => open_dialog(session, self.viewer_picks.clone()),
            }
        }
    }

    /// A screenshot of the session as a PNG (into the store, so the file is marked as a
    /// rendering of its image).
    fn screenshot_png(&mut self, session: &str) -> Result<Vec<u8>, String> {
        let slot = self.sessions.get_mut(session).ok_or("no such session")?;
        let id = self.images.capture(&mut slot.session, slot.group, false, false, None)?;
        let img = self.images.get(id.as_str())?;
        let r = guilab_image::render(&img.body, &self.images.store, &self.images.fonts, &Default::default()).map_err(|e| e.to_string())?;
        Ok(r.png(Some(img.id)))
    }
}

/// Runs `f` with the viewer's renderer: its own software one if any, else the server's
/// GLES. (Shown only exists with one of them.)
fn with_renderer<T>(gpu: &guilab_compositor::SharedGpu, soft: Option<&mut GlesRenderer>, f: impl FnOnce(&mut GlesRenderer) -> T) -> T {
    match soft {
        Some(r) => f(r),
        None => {
            let mut gpu = gpu.borrow_mut();
            let Renderer::Gles(r) = &mut gpu.renderer else { unreachable!("the viewer has a software renderer on pixman servers") };
            f(r)
        }
    }
}

/// Inserts the channel that carries connect results into the server loop.
pub(crate) fn insert_channel(handle: &calloop::LoopHandle<'static, Server>) -> anyhow::Result<channel::Sender<ConnectResult>> {
    let (tx, rx) = channel::channel::<ConnectResult>();
    handle
        .insert_source(rx, |ev, _, srv: &mut Server| {
            if let channel::Event::Msg(m) = ev {
                srv.viewer_connected(m);
            }
        })
        .map_err(|e| anyhow::anyhow!("{}", e.error))?;
    Ok(tx)
}

/// Feed items for the log's messages (both ways), agent commands, screenshots, made images
/// and status changes from seq `next` on (the newest [`FEED_BACKLOG`] when the feed is new).
/// Images the agent sends are shown normally; its working screenshots and `image make`
/// previews dimmed and smaller. A human message's `screen` (context for the agent) isn't.
fn feed_items(log: &guilab_proto::log::EventLog, next: u64, _images: &Images) -> Vec<FeedItem<ImageId>> {
    let fmt = Fmt::local();
    let wanted = |e: &&guilab_proto::Event| match &e.data {
        // Its Screenshot event shows it, with the image.
        EventData::Command { cmd } => !cmd.starts_with("screenshot"),
        EventData::Message { .. } | EventData::AgentMessage { .. } | EventData::ImageMade { .. } | EventData::Screenshot { .. } | EventData::Work { .. } | EventData::Done => true,
        _ => false,
    };
    let mut events: Vec<&guilab_proto::Event> = if next == 0 {
        log.from_seq(0).rev().filter(wanted).take(FEED_BACKLOG).collect()
    } else {
        log.from_seq(next).filter(wanted).collect()
    };
    if next == 0 {
        events.reverse();
    }
    let ids = |refs: &[guilab_proto::ImageRef]| refs.iter().filter_map(|r| ImageId::parse(&r.id).ok()).collect::<Vec<_>>();
    let id = |s: &str| ImageId::parse(s).ok();
    events
        .into_iter()
        .map(|e| {
            let time = fmt.clock(e.t);
            match &e.data {
                EventData::Message { text, images: refs, .. } => FeedItem::me(time, text.clone()).with(ids(refs)),
                EventData::AgentMessage { text, images: refs } => FeedItem::them(AGENT, time, text.clone()).with(ids(refs)),
                EventData::ImageMade { image } => FeedItem::activity(time, format!("image make → {image}")).with(id(image)).faint(),
                EventData::Screenshot { image, .. } => FeedItem::activity(time, format!("screenshot → {image}")).with(id(image)).faint(),
                EventData::Command { cmd } => FeedItem::activity(time, cmd.clone()),
                EventData::Work { status } => FeedItem::activity(time, format!("working: {status}")),
                EventData::Done => FeedItem::activity(time, "done"),
                _ => unreachable!("filtered"),
            }
        })
        .collect()
}

/// The agent's name in the feed.
const AGENT: &str = "Agent";

/// Asks for a picture to insert (system file dialog, on its own thread); the pick goes to
/// `picks` for the tick to load.
fn open_dialog(session: String, picks: std::sync::Arc<std::sync::Mutex<Vec<(String, PathBuf)>>>) {
    let spawned = std::thread::Builder::new().name("open-dialog".into()).spawn(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Insert image").add_filter("Images", &["png", "jpg", "jpeg", "svg"]);
        if let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Pictures")).filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file()
            && let Ok(mut p) = picks.lock()
        {
            p.push((session, path));
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("open dialog thread: {e}");
    }
}

/// Asks where to save (system file dialog, on its own thread so the server keeps running)
/// and writes the PNG. Tells the human how it went via `notices`.
fn save_dialog(session: String, png: Vec<u8>, notices: std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let spawned = std::thread::Builder::new().name("save-dialog".into()).spawn(move || {
        let stamp = Fmt::local().file_stamp(guilab_proto::events::now_ms());
        let mut dialog = rfd::FileDialog::new().set_title("Save image").set_file_name(format!("{session}-{stamp}.png")).add_filter("PNG image", &["png"]);
        if let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Pictures")).filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else { return };
        if path.extension().is_none() {
            path.set_extension("png");
        }
        let tmp = path.with_extension("png.tmp");
        let msg = match std::fs::write(&tmp, &png).and_then(|()| std::fs::rename(&tmp, &path)) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Couldn't save: {e}"),
        };
        if let Ok(mut n) = notices.lock() {
            n.push(msg);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("save dialog thread: {e}");
    }
}
