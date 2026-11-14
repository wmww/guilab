//! smithay handler traits for `Session` (except xdg-shell, see `shell.rs`).

use std::{os::fd::OwnedFd, sync::Arc};

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::on_commit_buffer_handler},
    delegate_dispatch2,
    input::{
        Seat, SeatHandler, SeatState,
        dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source},
        pointer::{CursorImageStatus, Focus, PointerHandle},
    },
    reexports::{
        calloop::Interest,
        wayland_server::{
            Client, Resource,
            protocol::{wl_buffer, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier, get_dmabuf},
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        idle_inhibit::IdleInhibitHandler,
        keyboard_shortcuts_inhibit::{KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor},
        output::OutputHandler,
        pointer_constraints::{PointerConstraintsHandler, with_pointer_constraint},
        seat::WaylandFocus,
        selection::{
            SelectionHandler, SelectionSource, SelectionTarget,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus},
            ext_data_control,
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState, set_primary_focus},
            wlr_data_control,
        },
        shm::{ShmHandler, ShmState},
        text_input::TextInputActivation,
        xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData},
        xdg_foreign::{XdgForeignHandler, XdgForeignState},
    },
};

use crate::{
    focus::KbFocus,
    session::{ClientState, Session},
};

/// Stable per-session surface id, stored in each surface's data map (shown by `tree`).
pub struct SurfaceId(pub u32);

pub fn surface_id(surface: &WlSurface) -> Option<u32> {
    with_states(surface, |s| s.data_map.get::<SurfaceId>().map(|i| i.0))
}

impl CompositorHandler for Session {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(x) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &x.compositor_state;
        }
        &client.get_data::<ClientState>().expect("unknown client data").compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        let id = self.next_surface_id;
        self.next_surface_id += 1;
        with_states(surface, |s| {
            s.data_map.insert_if_missing(|| SurfaceId(id));
        });
        // Don't sample dmabufs the client is still rendering into (from anvil).
        add_pre_commit_hook::<Self, _>(surface, |state, _dh, surface| {
            let dmabuf = with_states(surface, |s| {
                match s.cached_state.get::<SurfaceAttributes>().pending().buffer.as_ref() {
                    Some(BufferAssignment::NewBuffer(b)) => get_dmabuf(b).cloned().ok(),
                    _ => None,
                }
            });
            if let Some(dmabuf) = dmabuf
                && let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ)
                && let Some(client) = surface.client()
            {
                let res = state.handle.insert_source(source, move |_, _, s: &mut Session| {
                    let dh = s.dh.clone();
                    s.client_compositor_state(&client).blocker_cleared(s, &dh);
                    Ok(())
                });
                if res.is_ok() {
                    add_blocker(surface, blocker);
                }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        // Cursor commits (animated cursors) don't count as the app drawing.
        if self.cursor_commit(surface) {
            return;
        }
        self.settle.note_commit();
        self.dirty = true;
        tracing::trace!(surface = surface_id(surface), "commit");
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(p) = get_parent(&root) {
                root = p;
            }
            if let Some(w) = self.window_for_surface(&root) {
                w.on_commit();
            }
        }
        self.popups.commit(surface);
        self.shell_commit(surface);
    }
}

impl BufferHandler for Session {
    fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Session {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for Session {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }
    fn dmabuf_imported(&mut self, _: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.gpu.borrow_mut().import_dmabuf(&dmabuf) {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}

impl OutputHandler for Session {}

impl SeatHandler for Session {
    type KeyboardFocus = KbFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, target: Option<&KbFocus>) {
        let client = target.and_then(|t| t.wl_surface()).and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh, seat, client.clone());
        set_primary_focus(&self.dh, seat, client);
    }

    fn cursor_image(&mut self, _: &Seat<Self>, image: CursorImageStatus) {
        self.set_cursor_status(image);
    }
}

// Needed by cursor-shape (which covers tablet tools too).
impl smithay::input::tablet::TabletSeatHandler for Session {
    type ToolFocus = WlSurface;
}

/// Owner of a compositor-side selection.
#[derive(Clone)]
pub enum SelData {
    /// Text set by `clip set`.
    Text(Arc<str>),
    /// An X client (bridged by the X11Wm).
    X11,
}

impl SelectionHandler for Session {
    type SelectionUserData = SelData;

    // A Wayland client set a selection: X clients get it through the X11Wm.
    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _seat: Seat<Self>) {
        let mimes = source.map(|s| s.mime_types());
        if ty == SelectionTarget::Clipboard {
            self.clip.set_owner(mimes.clone());
        }
        crate::xwayland::announce_selection(self, ty, mimes);
    }

    fn send_selection(&mut self, ty: SelectionTarget, mime: String, fd: OwnedFd, _: Seat<Self>, data: &SelData) {
        match data {
            SelData::Text(t) => crate::clipboard::send_text(t.clone(), &mime, fd),
            SelData::X11 => {
                if let Some(wm) = self.xw.wm.as_mut()
                    && let Err(e) = wm.send_selection(ty, mime, fd)
                {
                    tracing::debug!("X selection to Wayland: {e}");
                }
            }
        }
    }
}
impl DataDeviceHandler for Session {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}
impl WaylandDndGrabHandler for Session {
    // From anvil: a client started a drag (after a press); the grab moves the offer around.
    fn dnd_requested<S: Source>(&mut self, source: S, icon: Option<WlSurface>, seat: Seat<Self>, serial: Serial, ty: GrabType) {
        self.dnd_icon = icon;
        match ty {
            GrabType::Pointer => {
                let ptr = seat.get_pointer().unwrap();
                let Some(start) = ptr.grab_start_data() else { return source.cancel() };
                ptr.set_grab(self, DnDGrab::new_pointer(&self.dh, start, source, seat), serial, Focus::Keep);
            }
            GrabType::Touch => {
                let touch = seat.get_touch().unwrap();
                let Some(start) = touch.grab_start_data() else { return source.cancel() };
                touch.set_grab(self, DnDGrab::new_touch(&self.dh, start, source, seat), serial);
            }
        }
    }
}
impl DndGrabHandler for Session {
    fn dropped(&mut self, _: Option<DndTarget<'_, Self>>, _: bool, _: Seat<Self>, _: Point<f64, Logical>) {
        self.dnd_icon = None;
    }
    fn cancelled(&mut self, _: Seat<Self>, _: Point<f64, Logical>) {
        self.dnd_icon = None;
    }
}
impl PrimarySelectionHandler for Session {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}
impl wlr_data_control::DataControlHandler for Session {
    fn data_control_state(&mut self) -> &mut wlr_data_control::DataControlState {
        &mut self.wlr_data_control_state
    }
}
impl ext_data_control::DataControlHandler for Session {
    fn data_control_state(&mut self) -> &mut ext_data_control::DataControlState {
        &mut self.ext_data_control_state
    }
}

impl PointerConstraintsHandler for Session {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        if pointer.current_focus().as_ref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |c| {
                if let Some(c) = c {
                    c.activate()
                }
            });
        }
    }
    fn cursor_position_hint(&mut self, _: &WlSurface, _: &PointerHandle<Self>, _: Point<f64, Logical>) {}
}

impl TextInputActivation for Session {}

impl FractionalScaleHandler for Session {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        // One output per session, so its scale is always the right one.
        let scale = self.scale();
        with_states(&surface, |s| with_fractional_scale(s, |f| f.set_preferred_scale(scale)));
    }
}

impl XdgActivationHandler for Session {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }
    fn request_activation(&mut self, _: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        let w = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(&surface)).cloned();
        if data.timestamp.elapsed().as_secs() < 10
            && let Some(w) = w
        {
            self.focus_window(&w);
        }
    }
}

impl XdgForeignHandler for Session {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.xdg_foreign_state
    }
}

impl KeyboardShortcutsInhibitHandler for Session {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }
    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        // We have no compositor shortcuts; grant it.
        inhibitor.activate();
    }
}

impl IdleInhibitHandler for Session {
    fn inhibit(&mut self, _: WlSurface) {}
    fn uninhibit(&mut self, _: WlSurface) {}
}

delegate_dispatch2!(Session);
