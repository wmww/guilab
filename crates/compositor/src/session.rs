//! One session = one Wayland display with its own globals, seat, output, space and apps.
//!
//! Each session runs its own calloop `EventLoop<Session>` (sources get `&mut Session`),
//! which the server nests into its loop and dispatches with the session. This is what
//! smithay's per-display machinery (X11Wm, xwayland-shell, `*_state()` getters) expects.
//!
//! Caveat of nesting: calloop timers are handled by the loop's poll timeout, not by an
//! fd, so a `Timer` in a session loop never wakes the server. Anything timed is driven
//! from the server's loop instead (see `Session::tick`).

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use smithay::{
    backend::renderer::element::RenderElementStates,
    desktop::{PopupManager, Space, Window},
    input::{Seat, SeatState, keyboard::XkbConfig, pointer::CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{EventLoop, Interest, LoopHandle, Mode as CMode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle, ListeningSocket,
            backend::{ClientData, ClientId, DisconnectReason},
        },
    },
    utils::{Clock, Monotonic, Transform},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
        fixes::FixesState,
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        pointer_gestures::PointerGesturesState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        selection::{
            data_device::DataDeviceState, ext_data_control::DataControlState as ExtDataControlState,
            primary_selection::PrimarySelectionState, wlr_data_control::DataControlState as WlrDataControlState,
        },
        shell::xdg::{XdgShellState, decoration::XdgDecorationState},
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
        xdg_foreign::XdgForeignState,
    },
};

use crate::{
    procs::Procs,
    render::{Frame, Gpu},
};

pub type SharedGpu = Rc<RefCell<Gpu>>;

pub struct SessionConfig {
    pub name: String,
    /// Per-session runtime dir (becomes the apps' `XDG_RUNTIME_DIR`). Must exist.
    pub dir: PathBuf,
    /// Output size in pixels.
    pub size: (i32, i32),
    pub scale: f64,
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

pub const SOCKET_NAME: &str = "wayland-0";

pub struct Session {
    pub name: String,
    /// Unique per session instance; apps carry it as `GUILAB_TAG` (see procs.rs).
    pub tag: String,
    pub dir: PathBuf,
    pub handle: LoopHandle<'static, Session>,
    pub dh: DisplayHandle,
    pub gpu: SharedGpu,
    pub clock: Clock<Monotonic>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Session>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub wlr_data_control_state: WlrDataControlState,
    pub ext_data_control_state: ExtDataControlState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_foreign_state: XdgForeignState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub presentation_state: PresentationState,

    pub seat: Seat<Session>,
    pub cursor_status: CursorImageStatus,
    pub output: Output,
    pub space: Space<Window>,
    pub popups: PopupManager,
    /// Toplevels created but not yet placed (placed at their initial configure / first buffer).
    pub unmapped: Vec<Window>,

    pub frame: Frame,
    /// Element states of the last render, for primary-scanout tracking.
    pub last_states: Option<RenderElementStates>,
    pub frame_seq: u64,

    pub procs: Procs,
    pub next_surface_id: u32,
    /// Settle tracking.
    pub last_commit: Instant,
    pub last_render: Instant,
    /// When we last sent a toplevel configure.
    pub last_configure: Instant,
}

impl Session {
    /// Creates the session's display, globals, output and socket. The returned loop must be
    /// nested into the server's loop and dispatched with this session.
    pub fn new(cfg: SessionConfig, gpu: SharedGpu) -> anyhow::Result<(Session, EventLoop<'static, Session>)> {
        let event_loop: EventLoop<'static, Session> = EventLoop::try_new()?;
        let handle = event_loop.handle();
        let display: Display<Session> = Display::new()?;
        let dh = display.handle();
        let clock = Clock::<Monotonic>::new();

        // Socket: `<dir>/wayland-0`, so apps get XDG_RUNTIME_DIR=<dir>, WAYLAND_DISPLAY=wayland-0.
        let sock_path = cfg.dir.join(SOCKET_NAME);
        let _ = std::fs::remove_file(&sock_path);
        let listener = ListeningSocket::bind_absolute(sock_path.clone())
            .with_context(|| format!("binding {}", sock_path.display()))?;
        handle
            .insert_source(Generic::new(listener, Interest::READ, CMode::Level), |_, l, s: &mut Session| {
                while let Some(stream) = l.accept()? {
                    if let Err(e) = s.dh.insert_client(stream, Arc::new(ClientState::default())) {
                        tracing::warn!("insert client: {e}");
                    }
                }
                Ok(PostAction::Continue)
            })
            .map_err(|e| e.error)?;
        handle
            .insert_source(Generic::new(display, Interest::READ, CMode::Level), |_, d, s: &mut Session| {
                // Safety: the display is never dropped while the source lives.
                if let Err(e) = unsafe { d.get_mut().dispatch_clients(s) } {
                    tracing::warn!("dispatch_clients: {e}");
                }
                Ok(PostAction::Continue)
            })
            .map_err(|e| e.error)?;

        // Globals. See notes/code-map.md for why each is (or isn't) here.
        let compositor_state = CompositorState::new_v6::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let wlr_data_control_state = WlrDataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |_| true);
        let ext_data_control_state = ExtDataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |_| true);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&dh);
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(&dh);
        let presentation_state = PresentationState::new::<Self>(&dh, clock.id() as u32);
        ViewporterState::new::<Self>(&dh);
        FractionalScaleManagerState::new::<Self>(&dh);
        SinglePixelBufferState::new::<Self>(&dh);
        CursorShapeManagerState::new::<Self>(&dh);
        PointerGesturesState::new::<Self>(&dh);
        RelativePointerManagerState::new::<Self>(&dh);
        PointerConstraintsState::new::<Self>(&dh);
        TextInputManagerState::new::<Self>(&dh);
        IdleInhibitManagerState::new::<Self>(&dh);
        FixesState::new::<Self>(&dh);

        let mut dmabuf_state = DmabufState::new();
        let mut dmabuf_global = None;
        {
            let g = gpu.borrow();
            if let (crate::render::Renderer::Gles(r), Some(node)) = (&g.renderer, g.node) {
                use smithay::backend::renderer::ImportDma;
                match DmabufFeedbackBuilder::new(node.dev_id(), r.dmabuf_formats()).build() {
                    Ok(fb) => dmabuf_global = Some(dmabuf_state.create_global_with_default_feedback::<Self>(&dh, &fb)),
                    Err(e) => tracing::warn!("dmabuf feedback: {e}"),
                }
            }
        }

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
        // Repeat rate 0: no client-side key repeat, so held keys in agent input chains don't repeat.
        seat.add_keyboard(XkbConfig { layout: "us", ..Default::default() }, 600, 0)
            .context("keyboard (is xkeyboard-config installed?)")?;
        seat.add_pointer();
        seat.add_touch();

        let output = Output::new(
            "HEADLESS-1".into(),
            PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "guilab".into(), model: "headless".into(), serial_number: cfg.name.clone() },
        );
        let mode = Mode { size: cfg.size.into(), refresh: 60_000 };
        output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Fractional(cfg.scale)), Some((0, 0).into()));
        output.set_preferred(mode);
        output.create_global::<Self>(&dh);
        let mut space = Space::default();
        space.map_output(&output, (0, 0));
        let frame = Frame::new(&output);

        let now = Instant::now();
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let session = Session {
            tag: format!("{}-{}-{nonce:x}", std::process::id(), cfg.name),
            name: cfg.name,
            dir: cfg.dir,
            handle,
            dh,
            gpu,
            clock,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            dmabuf_state,
            dmabuf_global,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            wlr_data_control_state,
            ext_data_control_state,
            xdg_activation_state,
            xdg_foreign_state,
            keyboard_shortcuts_inhibit_state,
            presentation_state,
            seat,
            cursor_status: CursorImageStatus::default_named(),
            output,
            space,
            popups: PopupManager::default(),
            unmapped: Vec::new(),
            frame,
            last_states: None,
            frame_seq: 0,
            procs: Procs::default(),
            next_surface_id: 1,
            last_commit: now,
            last_render: now,
            last_configure: now,
        };
        Ok((session, event_loop))
    }

    pub fn socket_path(&self) -> PathBuf {
        self.dir.join(SOCKET_NAME)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Call after dispatching the session loop.
    pub fn flush(&mut self) {
        if let Err(e) = self.dh.flush_clients() {
            tracing::debug!("flush_clients: {e}");
        }
    }

    pub fn windows(&self) -> usize {
        self.space.elements().count()
    }

    /// Frame interval of the output.
    pub fn refresh_interval(&self) -> Duration {
        let mhz = self.output.current_mode().map(|m| m.refresh).unwrap_or(60_000).max(1);
        Duration::from_secs_f64(1000.0 / mhz as f64)
    }
}
