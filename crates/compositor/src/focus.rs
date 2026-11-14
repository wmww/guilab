//! Keyboard focus target: a Wayland surface or an X11 window.
//!
//! X windows need their own target because focusing one must also do the X side
//! (SetInputFocus / WM_TAKE_FOCUS), which `X11Surface`'s `KeyboardTarget` impl does.
//! Pointer and touch focus stay `WlSurface`: smithay's X11Surface targets just forward
//! to the wl_surface, and X-side pointer grabs (menus) happen inside Xwayland.

use std::borrow::Cow;

use smithay::{
    backend::input::KeyState,
    desktop::{PopupKind, Window, WindowSurface},
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    },
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface},
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
    xwayland::X11Surface,
};

use crate::session::Session;

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // X11Surface is big; cloned rarely
pub enum KbFocus {
    Wl(WlSurface),
    X(X11Surface),
}

impl KbFocus {
    /// The focus target for a window (its toplevel surface or X window).
    pub fn of(w: &Window) -> Option<KbFocus> {
        match w.underlying_surface() {
            WindowSurface::Wayland(t) => Some(KbFocus::Wl(t.wl_surface().clone())),
            WindowSurface::X11(x) => Some(KbFocus::X(x.clone())),
        }
    }

    pub fn is_x11(&self) -> bool {
        matches!(self, KbFocus::X(_))
    }

    fn inner(&self) -> &dyn KeyboardTarget<Session> {
        match self {
            KbFocus::Wl(s) => s,
            KbFocus::X(x) => x,
        }
    }
}

impl IsAlive for KbFocus {
    fn alive(&self) -> bool {
        match self {
            KbFocus::Wl(s) => s.alive(),
            KbFocus::X(x) => x.alive(),
        }
    }
}

impl WaylandFocus for KbFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            KbFocus::Wl(s) => Some(Cow::Borrowed(s)),
            KbFocus::X(x) => x.wl_surface().map(Cow::Owned),
        }
    }
    fn same_client_as(&self, id: &ObjectId) -> bool {
        match self {
            KbFocus::Wl(s) => s.same_client_as(id),
            KbFocus::X(x) => x.same_client_as(id),
        }
    }
}

impl From<WlSurface> for KbFocus {
    fn from(s: WlSurface) -> Self {
        KbFocus::Wl(s)
    }
}

impl From<PopupKind> for KbFocus {
    fn from(p: PopupKind) -> Self {
        KbFocus::Wl(p.wl_surface().clone())
    }
}

/// Popup grabs need `PointerFocus: From<KeyboardFocus>`. Their roots are always Wayland
/// toplevels, so the X arm doesn't run in practice.
impl From<KbFocus> for WlSurface {
    fn from(f: KbFocus) -> Self {
        match f {
            KbFocus::Wl(s) => s,
            KbFocus::X(x) => x.wl_surface().expect("X11 window without a wl_surface as a popup grab root"),
        }
    }
}

impl KeyboardTarget<Session> for KbFocus {
    fn enter(&self, seat: &Seat<Session>, data: &mut Session, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        if self.is_x11() {
            data.x_focus_at = Some(std::time::Instant::now());
        }
        self.inner().enter(seat, data, keys, serial)
    }
    fn leave(&self, seat: &Seat<Session>, data: &mut Session, serial: Serial) {
        self.inner().leave(seat, data, serial)
    }
    fn key(&self, seat: &Seat<Session>, data: &mut Session, key: KeysymHandle<'_>, state: KeyState, serial: Serial, time: smithay::backend::input::InputTime) {
        self.inner().key(seat, data, key, state, serial, time)
    }
    fn modifiers(&self, seat: &Seat<Session>, data: &mut Session, modifiers: ModifiersState, serial: Serial) {
        self.inner().modifiers(seat, data, modifiers, serial)
    }
}
