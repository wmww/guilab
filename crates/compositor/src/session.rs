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
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use smithay::{
    desktop::{PopupManager, Space, Window},
    input::{Seat, SeatState, keyboard::{XkbConfig, xkb}},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{EventLoop, Interest, LoopHandle, Mode as CMode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle, ListeningSocket,
            protocol::wl_surface::WlSurface,
            backend::{ClientData, ClientId, DisconnectReason},
        },
    },
    utils::{Clock, Logical, Monotonic, Point, Transform},
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
        xwayland_shell::XWaylandShellState,
    },
};

use guilab_proto::log::EventLog;

use crate::{
    keys::Keymap,
    procs::Procs,
    render::{Frame, Gpu},
    xwayland::Xw,
};

pub type SharedGpu = Rc<RefCell<Gpu>>;

pub struct SessionConfig {
    pub name: String,
    /// Per-session runtime dir (becomes the apps' `XDG_RUNTIME_DIR`). Must exist.
    pub dir: PathBuf,
    /// Output size in pixels.
    pub size: (i32, i32),
    pub scale: f64,
    /// Start Xwayland (see xwayland.rs).
    pub xwayland: bool,
}

#[derive(Default)]
pub(crate) struct ClientState {
    pub compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

pub const SOCKET_NAME: &str = "wayland-0";

/// `color_under_pointer` cache: ((viz seq, frame seq), when, color).
pub(crate) type UnderPointer = ((u64, u64), Instant, Option<[u8; 3]>);

pub struct Session {
    pub name: String,
    /// Unique per session instance; apps carry it as `GUILAB_TAG` (see procs.rs).
    pub tag: String,
    pub dir: PathBuf,
    pub(crate) handle: LoopHandle<'static, Session>,
    pub(crate) dh: DisplayHandle,
    pub(crate) gpu: SharedGpu,
    pub(crate) clock: Clock<Monotonic>,

    // `_` fields: states kept alive with the session, never read.
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    _xdg_decoration_state: XdgDecorationState,
    pub(crate) shm_state: ShmState,
    pub(crate) dmabuf_state: DmabufState,
    _dmabuf_global: Option<DmabufGlobal>,
    _output_manager_state: OutputManagerState,
    pub(crate) seat_state: SeatState<Session>,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) primary_selection_state: PrimarySelectionState,
    pub(crate) wlr_data_control_state: WlrDataControlState,
    pub(crate) ext_data_control_state: ExtDataControlState,
    pub(crate) xdg_activation_state: XdgActivationState,
    pub(crate) xdg_foreign_state: XdgForeignState,
    pub(crate) keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    _presentation_state: PresentationState,
    pub(crate) xwayland_shell_state: XWaylandShellState,

    pub(crate) seat: Seat<Session>,
    pub(crate) cursor: crate::cursor::CursorState,
    pub(crate) output: Output,
    pub(crate) space: Space<Window>,
    pub(crate) popups: PopupManager,
    /// Toplevels created but not yet placed (placed at their initial configure / first buffer).
    pub(crate) unmapped: Vec<Window>,

    pub frame: Frame,
    pub frame_seq: u64,

    pub(crate) procs: Procs,
    pub(crate) next_surface_id: u32,
    pub(crate) settle: crate::settle::Times,
    /// Something may have changed what's on screen since the last render (client
    /// requests, commands, input). Idle sessions skip rendering.
    pub(crate) dirty: bool,

    pub(crate) keymap: Keymap,
    pub(crate) input: crate::input::InputState,
    /// Input that reached the apps, for the viewer to show.
    pub(crate) viz: guilab_proto::viz::VizLog,
    /// Cache for `color_under_pointer`: (viz end, frame seq), when sampled, the color.
    pub(crate) under_pointer: Option<UnderPointer>,
    pub clip: crate::clipboard::Clipboard,
    /// Icon surface of an active drag.
    pub(crate) dnd_icon: Option<WlSurface>,
    pub(crate) xw: Xw,
    /// The session's event log (the server adds commands, messages, lock changes).
    pub log: EventLog,
    /// Toplevels as of the last frame (`track_windows`): id, title, client pid.
    pub(crate) seen_windows: Vec<(u32, Option<String>, Option<u32>)>,
    /// The session's private D-Bus daemon and its address (see bus.rs); None without
    /// dbus-daemon.
    pub(crate) bus: Option<(std::process::Child, String)>,
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

        // Globals. See notes/smithay.md for the chosen set and what is skipped.
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
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);

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
        // Repeat starts off (rate 0), so held keys in agent input chains don't repeat; human keys
        // turn it on (see `input::set_repeat`).
        seat.add_keyboard(XkbConfig { layout: "us", ..Default::default() }, 600, 0)
            .context("keyboard (is xkeyboard-config installed?)")?;
        // Same keymap, as text we keep (keys.rs extends it to type arbitrary characters).
        let keymap_text = {
            let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
            xkb::Keymap::new_from_names(&ctx, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
                .context("keymap (is xkeyboard-config installed?)")?
                .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
        };
        let keymap = Keymap::new(keymap_text)?;
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
            _xdg_decoration_state: xdg_decoration_state,
            shm_state,
            dmabuf_state,
            _dmabuf_global: dmabuf_global,
            _output_manager_state: output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            wlr_data_control_state,
            ext_data_control_state,
            xdg_activation_state,
            xdg_foreign_state,
            keyboard_shortcuts_inhibit_state,
            _presentation_state: presentation_state,
            xwayland_shell_state,
            seat,
            cursor: Default::default(),
            output,
            space,
            popups: PopupManager::default(),
            unmapped: Vec::new(),
            frame,
            frame_seq: 0,
            procs: Procs::default(),
            next_surface_id: 1,
            settle: crate::settle::Times::new(),
            dirty: true,
            keymap,
            input: Default::default(),
            viz: Default::default(),
            under_pointer: None,
            clip: Default::default(),
            dnd_icon: None,
            xw: Xw::default(),
            log: EventLog::default(),
            seen_windows: Vec::new(),
            bus: None,
        };
        let mut session = session;
        let kb = session.seat.get_keyboard().unwrap();
        let text = session.keymap.text.clone();
        kb.set_keymap_from_string(&mut session, text).context("setting keymap")?;
        match crate::bus::start(&session.dir, crate::procs::TAG_VAR, &session.tag) {
            Ok(b) => session.bus = Some(b),
            Err(e) => tracing::warn!("{}: no D-Bus session bus for apps: {e:#}", session.name),
        }
        if cfg.xwayland {
            session.start_xwayland()?;
        }
        Ok((session, event_loop))
    }

    pub fn socket_path(&self) -> PathBuf {
        self.dir.join(SOCKET_NAME)
    }

    /// Call after dispatching the session loop.
    pub fn flush(&mut self) {
        if let Err(e) = self.dh.flush_clients() {
            tracing::debug!("flush_clients: {e}");
        }
    }

    /// Mapped windows (not counting X menus and tooltips).
    pub fn windows(&self) -> usize {
        self.space.elements().filter(|w| !crate::xwayland::is_or(w)).count()
    }

    /// Something outside may have changed what's on screen (a command, client requests):
    /// render at the next tick.
    pub fn touch(&mut self) {
        self.dirty = true;
    }

    /// Input that reached the apps, for the viewer's overlay.
    pub fn viz(&self) -> &guilab_proto::viz::VizLog {
        &self.viz
    }

    /// A note for the viewer's input overlay (e.g. an agent's output change).
    pub fn note(&mut self, text: String) {
        self.viz.push(true, guilab_proto::viz::VizKind::Note(text));
    }

    /// The output scale (output pixels per logical pixel).
    pub fn scale(&self) -> f64 {
        self.output.current_scale().fractional_scale()
    }

    /// Output pixels (what agents and screenshots use) of a global logical point, rounded.
    pub fn to_px(&self, p: Point<f64, Logical>) -> (i32, i32) {
        let s = self.scale();
        ((p.x * s).round() as i32, (p.y * s).round() as i32)
    }

    /// The global logical point at output pixel (x, y).
    pub fn from_px(&self, x: f64, y: f64) -> Point<f64, Logical> {
        let s = self.scale();
        (x / s, y / s).into()
    }

    /// Frame interval of the output.
    pub fn refresh_interval(&self) -> Duration {
        let mhz = self.output.current_mode().map(|m| m.refresh).unwrap_or(60_000).max(1);
        Duration::from_secs_f64(1000.0 / mhz as f64)
    }
}
