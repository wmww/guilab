//! xdg-shell: window policy, popups (constraints + grabs from anvil), decorations.
//!
//! Policy: a parentless toplevel fills the output (tiled, maximized, activated) if no other
//! tiled toplevel exists. Dialogs (with a parent) and later toplevels float, centered on the
//! output. Placement is decided at the initial configure, since `set_parent` arrives after
//! `get_toplevel`. Windows enter the space on their first buffer.

use std::{cell::Cell, time::Instant};

use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window, find_popup_root_surface,
        get_popup_toplevel_coords,
    },
    input::{Seat, pointer::Focus},
    reexports::{
        wayland_protocols::xdg::{decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecoMode, shell::server::xdg_toplevel},
        wayland_server::protocol::{wl_output, wl_seat, wl_surface::WlSurface},
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial},
    wayland::{
        compositor::with_states,
        seat::WaylandFocus,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelCachedState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
            decoration::XdgDecorationHandler,
        },
    },
    backend::renderer::utils::with_renderer_surface_state,
};

use crate::session::Session;

/// Per-window policy state, in `Window::user_data()`.
#[derive(Default)]
pub struct WindowState {
    pub tiled: Cell<bool>,
}

pub fn is_tiled(w: &Window) -> bool {
    w.user_data().get::<WindowState>().is_some_and(|s| s.tiled.get())
}

fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false)
}

const TILED_STATES: [xdg_toplevel::State; 5] = [
    xdg_toplevel::State::Maximized,
    xdg_toplevel::State::TiledLeft,
    xdg_toplevel::State::TiledRight,
    xdg_toplevel::State::TiledTop,
    xdg_toplevel::State::TiledBottom,
];

impl Session {
    /// Output area in logical coordinates.
    pub fn output_rect(&self) -> Rectangle<i32, Logical> {
        self.space.output_geometry(&self.output).unwrap_or_else(|| Rectangle::from_size((1, 1).into()))
    }

    /// Mapped or not-yet-mapped window whose toplevel surface is `surface`.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .chain(self.unmapped.iter())
            .find(|w| w.wl_surface().as_deref() == Some(surface))
            .cloned()
    }

    fn apply_policy(&self, w: &Window) {
        let tl = w.toplevel().unwrap();
        let tiled = tl.parent().is_none()
            && !self.space.elements().chain(self.unmapped.iter()).any(|o| o != w && is_tiled(o));
        tracing::debug!(surface = ?w.wl_surface().as_deref().and_then(crate::handlers::surface_id), tiled, "placing toplevel");
        w.user_data().insert_if_missing(WindowState::default);
        w.user_data().get::<WindowState>().unwrap().tiled.set(tiled);
        let area = self.output_rect().size;
        tl.with_pending_state(|s| {
            s.bounds = Some(area);
            s.decoration_mode = Some(DecoMode::ServerSide);
            if tiled {
                s.size = Some(area);
                for st in TILED_STATES {
                    s.states.set(st);
                }
            }
        });
    }

    /// Where a floating window goes: centered on the output, top-left kept on screen.
    fn centered(&self, w: &Window) -> Point<i32, Logical> {
        let area = self.output_rect();
        let size = w.geometry().size;
        let x = area.loc.x + ((area.size.w - size.w) / 2).max(0);
        let y = area.loc.y + ((area.size.h - size.h) / 2).max(0);
        (x, y).into()
    }

    /// xdg-shell part of the commit handler.
    pub(crate) fn shell_commit(&mut self, surface: &WlSurface) {
        if let Some(idx) = self.unmapped.iter().position(|w| w.wl_surface().as_deref() == Some(surface)) {
            let w = self.unmapped[idx].clone();
            let tl = w.toplevel().unwrap().clone();
            if !tl.is_initial_configure_sent() {
                self.apply_policy(&w);
                tl.send_configure();
                self.last_configure = Instant::now();
            } else if has_buffer(surface) {
                self.unmapped.remove(idx);
                let loc = if is_tiled(&w) { self.output_rect().loc } else { self.centered(&w) };
                self.space.map_element(w.clone(), loc, false);
                self.focus_window(&w);
            }
            return;
        }

        let mapped = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned();
        if let Some(w) = mapped {
            if !has_buffer(surface) {
                // Client unmapped (null buffer): back to the initial state.
                self.space.unmap_elem(&w);
                self.unmapped.push(w);
                self.focus_top();
            } else if !is_tiled(&w) {
                let loc = self.centered(&w);
                if self.space.element_location(&w) != Some(loc) {
                    self.space.map_element(w, loc, false);
                }
            }
            return;
        }

        if let Some(PopupKind::Xdg(p)) = self.popups.find_popup(surface)
            && !p.is_initial_configure_sent()
        {
            let _ = p.send_configure();
        }
    }

    /// Raise, activate and give keyboard focus to `w`.
    pub fn focus_window(&mut self, w: &Window) {
        self.space.raise_element(w, true);
        self.last_configure = Instant::now();
        for e in self.space.elements() {
            if let Some(tl) = e.toplevel()
                && tl.is_initial_configure_sent()
            {
                tl.send_pending_configure();
            }
        }
        let surface = w.wl_surface().map(|s| s.into_owned());
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    /// Focus the topmost window, if any.
    pub fn focus_top(&mut self) {
        if let Some(w) = self.space.elements().last().cloned() {
            self.focus_window(&w);
        } else if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, None, SERIAL_COUNTER.next_serial());
        }
    }

    /// Re-applies the policy after an output change: tiled windows get the new size,
    /// floating ones the new bounds and are re-centered.
    pub(crate) fn relayout(&mut self) {
        let area = self.output_rect();
        self.last_configure = Instant::now();
        let windows: Vec<Window> = self.space.elements().chain(self.unmapped.iter()).cloned().collect();
        for w in windows {
            let Some(tl) = w.toplevel() else { continue };
            tl.with_pending_state(|s| {
                s.bounds = Some(area.size);
                if is_tiled(&w) {
                    s.size = Some(area.size);
                }
            });
            if tl.is_initial_configure_sent() {
                tl.send_pending_configure();
            }
            if self.space.element_location(&w).is_some() {
                let loc = if is_tiled(&w) { area.loc } else { self.centered(&w) };
                self.space.map_element(w, loc, false);
            }
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else { return };
        let Some(window) = self.window_for_surface(&root) else { return };
        // Unmapped parents (rare) are treated as sitting at the output origin.
        let window_loc = self.space.element_geometry(&window).map(|g| g.loc).unwrap_or_default();
        let mut target = self.output_rect();
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= window_loc;
        popup.with_pending_state(|s| s.geometry = s.positioner.get_unconstrained_geometry(target));
    }
}

impl Session {
    fn reply_configure(&mut self, surface: &ToplevelSurface) {
        // The protocol wants a configure in reply even when we change nothing.
        if surface.is_initial_configure_sent() {
            surface.send_configure();
            self.last_configure = Instant::now();
        }
    }
}

impl XdgShellHandler for Session {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // The initial configure is sent from the commit handler (see shell_commit).
        self.unmapped.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.unmapped.retain(|w| w.toplevel() != Some(&surface));
        let mapped = self.space.elements().find(|w| w.toplevel() == Some(&surface)).cloned();
        if let Some(w) = mapped {
            self.space.unmap_elem(&w);
            self.focus_top();
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(e) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!("track popup: {e}");
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|s| {
            s.geometry = positioner.get_geometry();
            s.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        // From anvil: grab the popup chain; clicking outside dismisses it.
        let Some(seat) = Seat::<Session>::from_resource(&seat) else { return };
        let kind = PopupKind::Xdg(surface);
        let Some(root) = find_popup_root_surface(&kind).ok().and_then(|r| self.window_for_surface(&r)) else {
            return;
        };
        let Some(root) = root.wl_surface().map(|s| s.into_owned()) else { return };
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else { return };
        if let Some(kb) = seat.get_keyboard() {
            if kb.is_grabbed() && !(kb.has_grab(serial) || kb.has_grab(grab.previous_serial().unwrap_or(serial))) {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            kb.set_focus(self, grab.current_grab(), serial);
            kb.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(ptr) = seat.get_pointer() {
            if ptr.is_grabbed()
                && !(ptr.has_grab(serial) || ptr.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            ptr.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        // Tiled windows are already maximized; floating ones stay as they are.
        self.reply_configure(&surface);
    }
    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.reply_configure(&surface);
    }
    fn fullscreen_request(&mut self, surface: ToplevelSurface, _: Option<wl_output::WlOutput>) {
        self.reply_configure(&surface);
    }
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.reply_configure(&surface);
    }
}

impl XdgDecorationHandler for Session {
    // Always server-side, and we draw none: Qt, kitty and GTK3 drop their CSD.
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecoMode::ServerSide));
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _: DecoMode) {
        self.new_decoration(toplevel.clone());
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
            self.last_configure = Instant::now();
        }
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.request_mode(toplevel, DecoMode::ServerSide);
    }
    fn decoration_destroyed(&mut self, _: ToplevelSurface) {}
}

/// app_id and title of a toplevel surface.
pub fn toplevel_ids(surface: &WlSurface) -> (Option<String>, Option<String>) {
    with_states(surface, |s| {
        s.data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|d| {
                let d = d.lock().unwrap();
                (d.app_id.clone(), d.title.clone())
            })
            .unwrap_or_default()
    })
}

/// Whether the toplevel has configures the client hasn't acked, or acked but not committed yet.
pub fn has_pending_configure(surface: &WlSurface) -> bool {
    with_states(surface, |s| {
        let Some(d) = s.data_map.get::<XdgToplevelSurfaceData>() else { return false };
        let d = d.lock().unwrap();
        let acked = d.last_acked.as_ref().map(|c| c.serial);
        let committed = s.cached_state.get::<ToplevelCachedState>().current().last_acked.as_ref().map(|c| c.serial);
        !d.pending_configures().is_empty() || acked != committed
    })
}
