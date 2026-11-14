//! Per-session Xwayland with smithay's X11Wm (glue based on anvil's `shell/x11.rs`, MIT).
//!
//! Started at session creation when asked for (`new -x`). Display numbers come from our
//! own range (`DISPLAYS`), not smithay's :0-:32, so many sessions can run at once. Lock
//! file and sockets go away when the `XWayland` source drops with the session loop;
//! Xwayland exits when its Wayland connection closes, and X clients carry the session tag.
//!
//! Window policy matches xdg-shell (`shell.rs`): the first normal window tiles, dialogs,
//! transients and other window types float centered. Override-redirect windows (menus,
//! tooltips) sit where X puts them, above everything, kept in X stacking order in `or`.

use std::{os::fd::OwnedFd, process::Stdio};

use anyhow::{Context, bail};
use smithay::{
    desktop::Window,
    utils::{Logical, Rectangle},
    wayland::{
        selection::{
            SelectionTarget,
            data_device::{clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection, set_data_device_selection},
            primary_selection::{clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection, set_primary_selection},
        },
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge, WmWindowType, X11Window, XwmId},
    },
};

use crate::{
    focus::KbFocus,
    handlers::SelData,
    procs::TAG_VAR,
    session::Session,
    shell::{WindowState, is_tiled},
};

/// X display numbers we try, in order (smithay's own search stops at :32).
const DISPLAYS: std::ops::Range<u32> = 100..1000;

#[derive(Debug, Clone, PartialEq)]
pub enum XState {
    Off,
    Starting,
    /// Display number.
    Ready(u32),
    Failed(String),
}

pub struct Xw {
    pub state: XState,
    pub wm: Option<X11Wm>,
    /// Mapped override-redirect windows, bottom to top.
    pub or: Vec<Window>,
}

impl Default for Xw {
    fn default() -> Self {
        Xw { state: XState::Off, wm: None, or: Vec::new() }
    }
}

pub fn is_or(w: &Window) -> bool {
    w.x11_surface().is_some_and(|x| x.is_override_redirect())
}

impl Session {
    /// Spawns Xwayland; `xw.state` becomes Ready or Failed once it has started.
    pub(crate) fn start_xwayland(&mut self) -> anyhow::Result<()> {
        let log = std::fs::File::create(self.dir.join("xwayland.log")).context("creating xwayland.log")?;
        let envs = [("XDG_RUNTIME_DIR".to_string(), self.dir.to_string_lossy().into_owned()), (TAG_VAR.to_string(), self.tag())];
        let mut spawned = None;
        for n in DISPLAYS {
            match XWayland::spawn(&self.dh, n, envs.clone(), std::iter::empty::<&str>(), true, Stdio::null(), log.try_clone()?, |_| ()) {
                Ok(r) => {
                    spawned = Some(r);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("Xwayland isn't installed (no Xwayland on PATH)"),
                Err(e) => bail!("starting Xwayland: {e}"),
            }
        }
        let Some((xwayland, client)) = spawned else { bail!("no free X display number in :{}-:{}", DISPLAYS.start, DISPLAYS.end - 1) };
        self.handle
            .insert_source(xwayland, move |ev, _, s: &mut Session| match ev {
                XWaylandEvent::Ready { x11_socket, display_number } => {
                    match X11Wm::start_wm(s.handle.clone(), &s.dh, x11_socket, client.clone()) {
                        Ok(wm) => {
                            tracing::info!("{}: Xwayland ready on :{display_number}", s.name);
                            s.xw.wm = Some(wm);
                            s.xw.state = XState::Ready(display_number);
                        }
                        Err(e) => s.xw.state = XState::Failed(format!("X11 window manager: {e}")),
                    }
                }
                XWaylandEvent::Error => s.xw.state = XState::Failed("Xwayland exited during startup (see xwayland.log in the session dir)".into()),
            })
            .map_err(|e| e.error)?;
        self.xw.state = XState::Starting;
        Ok(())
    }

    /// `:N` once Xwayland is ready.
    pub fn x_display(&self) -> Option<String> {
        match self.xw.state {
            XState::Ready(n) => Some(format!(":{n}")),
            _ => None,
        }
    }

    pub fn x_state(&self) -> &XState {
        &self.xw.state
    }

    /// The space element of an X window.
    pub(crate) fn x_window(&self, x: &X11Surface) -> Option<Window> {
        self.space.elements().find(|w| w.x11_surface() == Some(x)).cloned()
    }

    /// The window an X window is transient for.
    pub(crate) fn x_parent(&self, x: &X11Surface) -> Option<Window> {
        let id = x.is_transient_for()?;
        self.space.elements().find(|w| w.x11_surface().is_some_and(|p| p.window_id() == id)).cloned()
    }

    /// Where a managed X window goes (X window rect, i.e. including any client-side
    /// frame): tiled windows fill the output, others keep their size, centered.
    fn x_rect(&self, w: &Window, x: &X11Surface) -> Rectangle<i32, Logical> {
        let area = self.output_rect();
        if is_tiled(w) {
            return area;
        }
        // Center the visible part (geometry), not a CSD shadow.
        let geo = w.geometry();
        let full = x.last_configure().size;
        let size = (full.w.clamp(1, area.size.w.max(1)), full.h.clamp(1, area.size.h.max(1)));
        let vis = (geo.size.w.min(area.size.w), geo.size.h.min(area.size.h));
        let x0 = area.loc.x + ((area.size.w - vis.0) / 2).max(0) - geo.loc.x;
        let y0 = area.loc.y + ((area.size.h - vis.1) / 2).max(0) - geo.loc.y;
        Rectangle::new((x0, y0).into(), size.into())
    }

    /// Configures a managed X window per the policy and puts it in the space there.
    pub(crate) fn x_place(&mut self, w: &Window) {
        let Some(x) = w.x11_surface() else { return };
        let rect = self.x_rect(w, x);
        if let Err(e) = x.configure(rect) {
            tracing::debug!("x11 configure: {e}");
        }
        self.move_window(w, rect.loc + w.geometry().loc);
    }

    /// Keeps override-redirect windows above everything, in X stacking order.
    pub(crate) fn raise_or(&mut self) {
        for w in self.xw.or.clone() {
            self.space.raise_element(&w, false);
        }
    }

    /// Tells X the stacking order and activation state of managed windows after a focus change.
    pub(crate) fn x_sync_stacking(&mut self, focused: &Window) {
        let Some(wm) = self.xw.wm.as_mut() else { return };
        let xs: Vec<X11Surface> = self.space.elements().filter_map(|w| w.x11_surface()).filter(|x| !x.is_override_redirect()).cloned().collect();
        for x in &xs {
            let _ = x.set_activated(focused.x11_surface() == Some(x));
        }
        if let Err(e) = wm.update_stacking_order_upwards(xs.iter()) {
            tracing::debug!("x11 stacking: {e}");
        }
    }

    fn x_forget(&mut self, x: &X11Surface) {
        self.xw.or.retain(|w| w.x11_surface() != Some(x));
        if let Some(w) = self.x_window(x) {
            self.space.unmap_elem(&w);
            let focused = self.seat.get_keyboard().and_then(|k| k.current_focus());
            if focused.is_none_or(|f| f == KbFocus::X(x.clone())) {
                self.focus_top();
            }
        }
    }
}

impl XWaylandShellHandler for Session {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for Session {
    fn xwm_state(&mut self, _: XwmId) -> &mut X11Wm {
        self.xw.wm.as_mut().expect("X11Wm event without a running X11Wm")
    }

    fn new_window(&mut self, _: XwmId, _: X11Surface) {}
    fn new_override_redirect_window(&mut self, _: XwmId, _: X11Surface) {}

    fn map_window_request(&mut self, _: XwmId, x: X11Surface) {
        if let Err(e) = x.set_mapped(true) {
            tracing::warn!("x11 map: {e}");
            return;
        }
        let w = Window::new_x11_window(x.clone());
        let floats = x.is_transient_for().is_some() || x.is_modal() || x.window_type().is_some_and(|t| t != WmWindowType::Normal);
        let tiled = !floats && !self.space.elements().chain(self.unmapped.iter()).any(is_tiled);
        tracing::debug!(xid = x.window_id(), class = x.class(), tiled, "placing X window");
        w.user_data().insert_if_missing(WindowState::default);
        w.user_data().get::<WindowState>().unwrap().tiled.set(tiled);
        if tiled {
            let _ = x.set_maximized(true);
        }
        self.x_place(&w);
        self.focus_window(&w);
    }

    fn mapped_override_redirect_window(&mut self, _: XwmId, x: X11Surface) {
        let w = Window::new_x11_window(x.clone());
        self.space.map_element(w.clone(), x.last_configure().loc, false);
        self.xw.or.retain(|o| o.x11_surface() != Some(&x));
        self.xw.or.push(w);
        self.raise_or();
    }

    fn unmapped_window(&mut self, _: XwmId, x: X11Surface) {
        self.x_forget(&x);
        if !x.is_override_redirect() {
            let _ = x.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _: XwmId, x: X11Surface) {
        self.x_forget(&x);
    }

    fn configure_request(&mut self, _: XwmId, x: X11Surface, nx: Option<i32>, ny: Option<i32>, nw: Option<u32>, nh: Option<u32>, _: Option<Reorder>) {
        let mut geo = x.last_configure();
        if let Some(w) = nw {
            geo.size.w = w as i32;
        }
        if let Some(h) = nh {
            geo.size.h = h as i32;
        }
        match self.x_window(&x) {
            // Mapped: size requests of floating windows are granted (and re-centered),
            // tiled windows keep filling the output. Moves are ignored.
            Some(w) => {
                if !is_tiled(&w) {
                    let _ = x.configure(Rectangle::new(geo.loc, geo.size));
                }
                self.x_place(&w);
            }
            None => {
                if let Some(v) = nx {
                    geo.loc.x = v;
                }
                if let Some(v) = ny {
                    geo.loc.y = v;
                }
                let _ = x.configure(geo);
            }
        }
    }

    fn configure_notify(&mut self, _: XwmId, x: X11Surface, geometry: Rectangle<i32, Logical>, above: Option<X11Window>) {
        let Some(w) = self.x_window(&x) else { return };
        if !x.is_override_redirect() {
            // Normally where we put it; keep the space in sync with what X thinks.
            self.move_window(&w, geometry.loc + w.geometry().loc);
            return;
        }
        self.move_window(&w, geometry.loc);
        // Restack among override-redirect windows: right above `above` if that's one of
        // them, else at the bottom of the group (still above all managed windows).
        self.xw.or.retain(|o| o != &w);
        let idx = above
            .and_then(|id| self.xw.or.iter().position(|o| o.x11_surface().is_some_and(|s| s.window_id() == id)))
            .map_or(0, |i| i + 1);
        self.xw.or.insert(idx, w);
        self.raise_or();
    }

    fn maximize_request(&mut self, _: XwmId, x: X11Surface) {
        // Tiled windows are already maximized; floating ones stay as they are.
        if self.x_window(&x).is_some_and(|w| is_tiled(&w)) {
            let _ = x.set_maximized(true);
        }
    }
    fn unmaximize_request(&mut self, _: XwmId, _: X11Surface) {}
    fn fullscreen_request(&mut self, _: XwmId, x: X11Surface) {
        if let Some(w) = self.x_window(&x)
            && is_tiled(&w)
        {
            let _ = x.set_fullscreen(true);
            self.x_place(&w);
        }
    }
    fn unfullscreen_request(&mut self, _: XwmId, x: X11Surface) {
        let _ = x.set_fullscreen(false);
    }

    fn resize_request(&mut self, _: XwmId, _: X11Surface, _: u32, _: ResizeEdge) {}
    fn move_request(&mut self, _: XwmId, _: X11Surface, _: u32) {}

    fn active_window_request(&mut self, _: XwmId, x: X11Surface, _: u32, _: Option<X11Surface>) {
        if let Some(w) = self.x_window(&x) {
            self.focus_window(&w);
        }
    }

    // Clipboard and primary selection bridging (from anvil). X clients may always read
    // the Wayland selection: sessions are disposable, and scripts (`xsel`) have no focus.
    fn allow_selection_access(&mut self, _: XwmId, _: SelectionTarget) -> bool {
        true
    }

    fn send_selection(&mut self, _: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        match selection {
            SelectionTarget::Clipboard => {
                let own = current_data_device_selection_userdata(&self.seat).and_then(|d| match &*d {
                    SelData::Text(t) => Some(t.clone()),
                    SelData::X11 => None,
                });
                if let Some(text) = own {
                    crate::clipboard::send_text(text, fd);
                } else if let Err(e) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    tracing::debug!("clipboard for X: {e}");
                }
            }
            SelectionTarget::Primary => {
                if let Err(e) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    tracing::debug!("primary for X: {e}");
                }
            }
        }
    }

    fn new_selection(&mut self, _: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => {
                self.clip_mimes = Some(mime_types.clone());
                set_data_device_selection(&self.dh, &self.seat, mime_types, SelData::X11);
            }
            SelectionTarget::Primary => set_primary_selection(&self.dh, &self.seat, mime_types, SelData::X11),
        }
    }

    fn cleared_selection(&mut self, _: XwmId, selection: SelectionTarget) {
        // Only clear selections X owns.
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some_and(|d| matches!(*d, SelData::X11)) {
                    clear_data_device_selection(&self.dh, &self.seat);
                    self.clip_mimes = None;
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some_and(|d| matches!(*d, SelData::X11)) {
                    clear_primary_selection(&self.dh, &self.seat);
                }
            }
        }
    }

    fn disconnected(&mut self, _: XwmId) {
        tracing::warn!("{}: Xwayland went away", self.name);
        self.xw.wm = None;
        self.xw.state = XState::Failed("Xwayland exited (see xwayland.log in the session dir)".into());
        let gone: Vec<Window> = self.space.elements().filter(|w| w.x11_surface().is_some()).cloned().collect();
        for w in gone {
            self.space.unmap_elem(&w);
        }
        self.xw.or.clear();
        self.focus_top();
    }
}

/// Text to X: when our own (`clip set`) selection changes, X needs to hear about it.
pub(crate) fn announce_selection(s: &mut Session, ty: SelectionTarget, mimes: Option<Vec<String>>) {
    if let Some(wm) = s.xw.wm.as_mut()
        && let Err(e) = wm.new_selection(ty, mimes)
    {
        tracing::debug!("x11 selection: {e}");
    }
}

/// Reads an X-owned selection into `fd`.
pub(crate) fn read_x_selection(s: &mut Session, mime: String, fd: OwnedFd) -> Result<(), String> {
    let wm = s.xw.wm.as_mut().ok_or("Xwayland isn't running")?;
    wm.send_selection(SelectionTarget::Clipboard, mime, fd).map_err(|e| format!("reading the X clipboard: {e}"))
}
