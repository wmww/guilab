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
use guilab_compositor::{AppCursor, InputEvent, InputSource, Session, render::Renderer};
use guilab_proto::{Annotation, EventData, Response, ViewerInfo, text::Fmt};
use guilab_viewer::{
    Action, BufferKind, Connected, FeedItem, HostEvent, HostSource, Input, SessionFrame, Status, Viewer, Who,
    CursorImage, HostCursor,
    egui::{ColorImage, CursorIcon as EguiCursor},
    markup,
};
use smithay::{
    backend::renderer::gles::GlesRenderer,
    backend::input::AxisSource,
    input::{keyboard::Keycode, pointer::CursorIcon},
};

use crate::{
    Server,
    requests::{MsgImage, human_message},
    send,
};

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
            self.seen.insert(session.tag.clone(), session.clip_seq);
            self.last = Some(text);
        }
        let read = self.reading.lock().ok().and_then(|mut r| r.take());
        if let Some(text) = read
            && self.last.as_ref() != Some(&text)
        {
            viewer.host.set_clipboard(text.clone());
            self.last = Some(text);
        }
        let seq = session.clip_seq;
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
    let scale = s.output.current_scale().fractional_scale();
    match input {
        Input::Motion { x, y } => InputEvent::PointerMotion { pos: (x / scale, y / scale).into() },
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
        Input::TouchDown { id, x, y } => InputEvent::TouchDown { slot: id as u32, pos: (x / scale, y / scale).into() },
        Input::TouchMotion { id, x, y } => InputEvent::TouchMotion { slot: id as u32, pos: (x / scale, y / scale).into() },
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
            }),
            _ => None,
        }
    }

    /// The session tab the viewer shows, if it's up.
    pub(crate) fn viewer_current(&self) -> Option<String> {
        match &self.viewer {
            ViewerState::Shown(s) => s.viewer.current().map(str::to_string).or_else(|| self.sessions.keys().next().cloned()),
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
        let fail = |waiters: Vec<UnixStream>, e: String| {
            tracing::warn!("viewer: {e}");
            for w in waiters {
                send(&w, &Response::err(format!("showing the viewer: {e}")));
            }
        };
        let c = match res {
            Ok(c) => c,
            Err(e) => return fail(waiters, e),
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
            Err(e) => return fail(waiters, format!("{e:#}")),
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
                return fail(waiters, e.error.to_string());
            }
        };
        let shown_on = display.display().to_string();
        tracing::info!("viewer shown on {shown_on}");
        self.viewer = ViewerState::Shown(Box::new(Shown { viewer, soft, token: Some(token), display, waiters, deadline, clip: ClipBridge::default(), cursors: Default::default(), next_cursor_id: 0, cpu_frames: Default::default() }));
    }

    /// Closes the viewer (if any). `why`: an error for `show` requests still waiting.
    pub(crate) fn hide_viewer(&mut self, why: Option<&str>) {
        match std::mem::take(&mut self.viewer) {
            ViewerState::Hidden => {}
            ViewerState::Connecting { waiters, .. } => {
                for w in waiters {
                    send(&w, &Response::err(why.unwrap_or("hidden before it was shown")));
                }
            }
            ViewerState::Shown(mut s) => {
                if let Some(why) = why {
                    tracing::info!("viewer closed: {why}");
                }
                let releases = s.viewer.release_all();
                self.apply_viewer_actions(releases);
                if let Some(t) = s.token.take() {
                    self.handle.remove(t);
                }
                for w in s.waiters.drain(..) {
                    send(&w, &Response::err(why.unwrap_or("hidden before it was shown")));
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
                    s.viewer.select(&tab);
                }
                if let Some(slot) = s.viewer.current().and_then(|c| self.sessions.get_mut(c)) {
                    s.clip.bridge(&mut s.viewer, &mut slot.session);
                }
                for (name, slot) in &self.sessions {
                    let log = &slot.session.log;
                    let next = s.viewer.feed_next(name, &slot.session.tag);
                    if next == log.end() {
                        continue;
                    }
                    let items = feed_items(log, next);
                    s.viewer.push_feed(name, &slot.session.tag, items, log.end());
                }
                // What's under the pointer decides the overlay's light/dark colors.
                let under: Vec<Option<[u8; 3]>> = self.sessions.values_mut().map(|slot| slot.session.color_under_pointer()).collect();
                let cursors: Vec<HostCursor> = self.sessions.values_mut().map(|slot| s.cursor(&mut slot.session)).collect();
                s.cursors.retain(|tag, _| self.sessions.values().any(|slot| &slot.session.tag == tag));
                // pixman frames reach the viewer as CPU images (the current session's only).
                let mut cpu_image = None;
                if s.soft.is_some() {
                    let cur = s.viewer.current().map(str::to_string).or_else(|| self.sessions.keys().next().cloned());
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
                            locked: slot.lock.current.is_some(),
                            shared: slot.lock.current.as_ref().is_some_and(|l| l.shared),
                            cursor: HostCursor::Icon(EguiCursor::Default),
                            viz: &slot.session.viz,
                            under_pointer: None,
                            agent: slot.working.as_ref().map(|w| Status {
                                text: w.status.clone(),
                                elapsed: w.since.elapsed().as_secs(),
                                quiet: slot.last_agent.elapsed().as_secs(),
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
                if !s.viewer.wants_draw(now, &frames) {
                    return;
                }
                let res = {
                    let viewer = &mut s.viewer;
                    with_renderer(&self.gpu, s.soft.as_deref_mut(), |r| viewer.draw(r, &frames, now))
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
                        send(&w, &r);
                    }
                }
                self.apply_viewer_actions(actions);
            }
        }
    }

    /// Applies the human's viewer actions. Input is dropped while the session is locked
    /// and not shared, except releases (so nothing stays stuck).
    pub(crate) fn apply_viewer_actions(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Input { session, input } => {
                    let Some(slot) = self.sessions.get_mut(&session) else { continue };
                    if !input.is_release() && !slot.lock.human_input_allowed() {
                        continue;
                    }
                    // The human using a session keeps it alive.
                    slot.last_active = Instant::now();
                    let touch = matches!(input, Input::TouchDown { .. } | Input::TouchMotion { .. } | Input::TouchUp { .. });
                    let ev = to_event(&slot.session, input);
                    slot.session.inject(ev, InputSource::Human);
                    if touch {
                        slot.session.inject(InputEvent::TouchFrame, InputSource::Human);
                    }
                }
                Action::Revoke { session } => self.revoke_lock(&session),
                Action::Resize { session, size, scale } => {
                    let Some(slot) = self.sessions.get_mut(&session) else { continue };
                    // The agent's lock freezes the output (for the human).
                    if slot.lock.current.is_some() {
                        continue;
                    }
                    slot.last_active = Instant::now();
                    let size = size.map(|(w, h)| (w as i32, h as i32));
                    if let Err(e) = crate::requests::set_output(&mut slot.session, size, scale, None) {
                        tracing::warn!("viewer: resizing {session}: {e:#}");
                    }
                }
                Action::Share { session, on } => self.share(&session, on),
                Action::SaveScreenshot { session, image } => {
                    let (w, h, mut rgba) = match image {
                        Some(i) => (i.size[0] as u32, i.size[1] as u32, i.pixels.iter().flat_map(|c| c.to_array()).collect::<Vec<u8>>()),
                        None => {
                            let Some(slot) = self.sessions.get_mut(&session) else { continue };
                            match slot.session.read_rgba() {
                                Ok(shot) => shot,
                                Err(e) => {
                                    tracing::warn!("viewer: screenshot to save: {e:#}");
                                    continue;
                                }
                            }
                        }
                    };
                    rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
                    let ViewerState::Shown(s) = &self.viewer else { continue };
                    save_dialog(session, w, h, rgba, s.viewer.notices());
                }
                Action::Freeze { session } => {
                    let Some(slot) = self.sessions.get_mut(&session) else { continue };
                    let shot = match slot.session.read_rgba() {
                        Ok((w, h, mut px)) => {
                            // Like screenshots: opaque (clients may leave alpha 0, e.g. GL areas).
                            px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
                            ColorImage::from_rgba_premultiplied([w as usize, h as usize], &px)
                        }
                        Err(e) => {
                            tracing::warn!("viewer: screenshot for drawing: {e:#}");
                            continue;
                        }
                    };
                    if let ViewerState::Shown(s) = &mut self.viewer {
                        s.viewer.set_screenshot(&session, shot);
                    }
                }
                Action::Message { session, text, images } => {
                    let Some(slot) = self.sessions.get_mut(&session) else { continue };
                    slot.last_active = Instant::now();
                    let mut annotations = Vec::new();
                    let mut files = Vec::new();
                    for (i, a) in images.into_iter().enumerate() {
                        let [w, h] = a.image.size;
                        let px: Vec<u8> = a.image.pixels.iter().flat_map(|c| c.to_array()).collect();
                        files.push(MsgImage::Rgba(w as u32, h as u32, px));
                        annotations.extend(a.notes.into_iter().map(|n| Annotation {
                            text: n.text,
                            image: Some(i),
                            x: Some(n.pos.x.round() as i32),
                            y: Some(n.pos.y.round() as i32),
                            color: markup::mark::color_name(n.color).map(|c| c.to_lowercase()),
                        }));
                    }
                    if let Err(e) = human_message(&mut slot.session, text, files, annotations) {
                        tracing::warn!("viewer: sending message: {e}");
                    }
                }
            }
        }
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

/// Feed items for the log's messages (both ways), agent commands and status changes from
/// seq `next` on (the newest [`FEED_BACKLOG`] when the feed is new).
fn feed_items(log: &guilab_proto::log::EventLog, next: u64) -> Vec<FeedItem> {
    let fmt = Fmt { tz: crate::tz_offset() };
    let wanted = |e: &&guilab_proto::Event| {
        matches!(e.data, EventData::Message { .. } | EventData::AgentMessage { .. } | EventData::Command { .. } | EventData::Work { .. } | EventData::Done)
    };
    let mut events: Vec<&guilab_proto::Event> = if next == 0 {
        log.from_seq(0).rev().filter(wanted).take(FEED_BACKLOG).collect()
    } else {
        log.from_seq(next).filter(wanted).collect()
    };
    if next == 0 {
        events.reverse();
    }
    events
        .into_iter()
        .map(|e| {
            let time = fmt.clock(e.t);
            match &e.data {
                EventData::Message { text, images, .. } => FeedItem { who: Who::Human, time, text: text.clone(), images: load_images(images) },
                EventData::AgentMessage { text, images } => FeedItem { who: Who::Agent, time, text: text.clone(), images: load_images(images) },
                EventData::Command { cmd } => FeedItem { who: Who::Log, time, text: cmd.clone(), images: Vec::new() },
                EventData::Work { status } => FeedItem { who: Who::Log, time, text: format!("working: {status}"), images: Vec::new() },
                EventData::Done => FeedItem { who: Who::Log, time, text: "done".into(), images: Vec::new() },
                _ => unreachable!("filtered"),
            }
        })
        .collect()
}

fn load_images(paths: &[PathBuf]) -> Vec<std::sync::Arc<ColorImage>> {
    paths
        .iter()
        .filter_map(|p| match decode_png(p) {
            Ok(i) => Some(std::sync::Arc::new(i)),
            Err(e) => {
                tracing::warn!("viewer: image {}: {e:#}", p.display());
                None
            }
        })
        .collect()
}

fn decode_png(path: &std::path::Path) -> anyhow::Result<ColorImage> {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().ok_or_else(|| anyhow::anyhow!("image too large"))?];
    let info = r.next_frame(&mut buf)?;
    let buf = &buf[..info.buffer_size()];
    let size = [info.width as usize, info.height as usize];
    Ok(match info.color_type {
        png::ColorType::Rgb => ColorImage::from_rgb(size, buf),
        png::ColorType::Rgba => ColorImage::from_rgba_unmultiplied(size, buf),
        png::ColorType::Grayscale => ColorImage::from_rgb(size, &buf.iter().flat_map(|g| [*g; 3]).collect::<Vec<_>>()),
        png::ColorType::GrayscaleAlpha => {
            ColorImage::from_rgba_unmultiplied(size, &buf.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect::<Vec<_>>())
        }
        t => anyhow::bail!("unsupported PNG color type {t:?}"),
    })
}

/// Asks where to save (system file dialog, on its own thread so the server keeps running)
/// and writes the PNG. Tells the human how it went via `notices`.
fn save_dialog(session: String, w: u32, h: u32, rgba: Vec<u8>, notices: std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let spawned = std::thread::Builder::new().name("save-dialog".into()).spawn(move || {
        let stamp = chrono_like_stamp();
        let mut dialog = rfd::FileDialog::new().set_title("Save screenshot").set_file_name(format!("{session}-{stamp}.png")).add_filter("PNG image", &["png"]);
        if let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Pictures")).filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else { return };
        if path.extension().is_none() {
            path.set_extension("png");
        }
        let msg = match guilab_compositor::write_rgba_png(&path, w, h, &rgba) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Couldn't save: {e:#}"),
        };
        if let Ok(mut n) = notices.lock() {
            n.push(msg);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("save dialog thread: {e}");
    }
}

/// Local time as YYYYMMDD-HHMMSS, for default file names.
fn chrono_like_stamp() -> String {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Realtime).tv_sec as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes only to `tm`.
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}
