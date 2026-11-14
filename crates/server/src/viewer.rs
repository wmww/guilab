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
use guilab_compositor::{InputEvent, InputSource, Session, render::Renderer};
use guilab_proto::{Response, ViewerInfo};
use guilab_viewer::{Action, BufferKind, Connected, HostEvent, HostSource, Input, SessionFrame, Viewer, egui::CursorIcon as EguiCursor};
use smithay::{
    backend::input::AxisSource,
    input::{keyboard::Keycode, pointer::{CursorIcon, CursorImageStatus}},
};

use crate::{Server, send};

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
    /// None once the source removed itself (host lost).
    token: Option<RegistrationToken>,
    display: PathBuf,
    /// `show` requests waiting for the first frame.
    waiters: Vec<UnixStream>,
    deadline: Instant,
}

fn egui_cursor(status: &CursorImageStatus) -> EguiCursor {
    let CursorImageStatus::Named(icon) = status else {
        // A client-drawn cursor surface: we can't forward its image, show an arrow.
        return if matches!(status, CursorImageStatus::Hidden) { EguiCursor::None } else { EguiCursor::Default };
    };
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
        Input::Button { button, pressed } => InputEvent::PointerButton { button, pressed },
        Input::Scroll { dx, dy, steps, stop } => InputEvent::PointerAxis {
            source: if steps.is_some() { AxisSource::Wheel } else { AxisSource::Finger },
            delta: (dx / scale, dy / scale),
            steps,
            stop,
        },
        Input::Key { code, pressed } => InputEvent::Key { code: Keycode::new(code + 8), pressed },
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

    /// `show`: opens the viewer on `display`. Replies (to `stream`) once the first frame is
    /// up; `None` = answered later.
    pub(crate) fn show(&mut self, display: PathBuf, stream: Option<UnixStream>) -> Option<Response> {
        if !matches!(self.gpu.borrow().renderer, Renderer::Gles(_)) {
            return Some(Response::err("the viewer needs GPU rendering (GLES), but this server renders with pixman"));
        }
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
        let viewer = {
            let mut gpu = self.gpu.borrow_mut();
            let (gbm, node) = (gpu.gbm.clone(), gpu.node);
            let Renderer::Gles(r) = &mut gpu.renderer else { unreachable!("checked in show") };
            Viewer::new(&c, r, gbm, node, force_shm)
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
                let mut gpu = self.gpu.borrow_mut();
                if let Renderer::Gles(r) = &mut gpu.renderer {
                    viewer.destroy(r);
                }
                return fail(waiters, e.error.to_string());
            }
        };
        let shown_on = display.display().to_string();
        tracing::info!("viewer shown on {shown_on}");
        self.viewer = ViewerState::Shown(Box::new(Shown { viewer, token: Some(token), display, waiters, deadline }));
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
                let mut gpu = self.gpu.borrow_mut();
                if let Renderer::Gles(r) = &mut gpu.renderer {
                    s.viewer.destroy(r);
                }
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
                let frames: Vec<SessionFrame> = self
                    .sessions
                    .iter()
                    .map(|(name, slot)| {
                        let o = slot.session.output_info();
                        SessionFrame {
                            name,
                            key: &slot.session.tag,
                            texture: slot.session.frame.gles_texture().map(|(t, size)| (t.tex_id(), size)),
                            seq: slot.session.frame_seq,
                            size: (o.width, o.height),
                            scale: o.scale,
                            locked: slot.lock.current.is_some(),
                            shared: slot.lock.current.as_ref().is_some_and(|l| l.shared),
                            cursor: egui_cursor(&slot.session.cursor_status),
                        }
                    })
                    .collect();
                if !s.viewer.wants_draw(now, &frames) {
                    return;
                }
                let res = {
                    let mut gpu = self.gpu.borrow_mut();
                    let Renderer::Gles(r) = &mut gpu.renderer else { return };
                    s.viewer.draw(r, &frames, now)
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
                    let ev = to_event(&slot.session, input);
                    slot.session.inject(ev, InputSource::Human);
                }
                Action::Revoke { session } => self.revoke_lock(&session),
                Action::Share { session, on } => self.share(&session, on),
            }
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
