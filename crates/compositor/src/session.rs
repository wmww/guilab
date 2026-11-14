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
    input::{Seat, SeatState, keyboard::{Keycode, Keysym, XkbConfig, xkb}, pointer::CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{EventLoop, Interest, LoopHandle, Mode as CMode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle, ListeningSocket,
            protocol::wl_surface::WlSurface,
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
        xwayland_shell::XWaylandShellState,
    },
};

use guilab_proto::log::EventLog;

use crate::{
    input::Held,
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
pub struct ClientState {
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
    pub xwayland_shell_state: XWaylandShellState,

    pub seat: Seat<Session>,
    /// What the apps want the cursor to be; change it with `set_cursor_status`.
    pub cursor_status: CursorImageStatus,
    pub(crate) cursor_seq: u64,
    pub(crate) cursor_image: Option<std::sync::Arc<crate::cursor::CursorImage>>,
    /// A named cursor loaded from the theme, by (icon, scale bits).
    pub(crate) named_cursor: Option<crate::cursor::NamedCursor>,
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
    /// When input was last delivered (settle waits for the app to react).
    pub last_input: Instant,
    /// Start of the current run of commits without a `SETTLE_QUIET` gap (animation check).
    pub busy_since: Instant,
    /// The last `run`: when, and the app's id (settle waits for its first window).
    pub last_spawn: Option<(Instant, u32)>,
    /// When a toplevel last appeared.
    pub last_map: Instant,
    /// Something may have changed what's on screen since the last render (client
    /// requests, commands, input). Idle sessions skip rendering.
    pub dirty: bool,

    pub keymap: Keymap,
    /// Extra keysyms on spare keycodes of the current seat keymap (typing, see actions.rs).
    pub extra_keys: Vec<(Keysym, Keycode)>,
    pub held: Held,
    /// Clients have key repeat on (see `input::set_repeat`).
    pub key_repeat: bool,
    /// Input that reached the apps, for the viewer to show.
    pub viz: guilab_proto::viz::VizLog,
    /// Cache for `color_under_pointer`: (viz end, frame seq), when sampled, the color.
    pub(crate) under_pointer: Option<UnderPointer>,
    /// A button was pressed while a popup grab was active (see `popup_may_close`).
    pub click_in_grab: bool,
    /// Mime types of a client-owned clipboard selection (None: empty or ours).
    pub clip_mimes: Option<Vec<String>>,
    /// Icon surface of an active drag.
    pub dnd_icon: Option<WlSurface>,
    pub xw: Xw,
    /// The session's event log (the server adds commands, messages, lock changes).
    pub log: EventLog,
    /// Toplevels as last seen by `track_windows`: (surface id, title).
    pub(crate) seen_windows: Vec<(u32, Option<String>)>,
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
            xwayland_shell_state,
            seat,
            cursor_status: CursorImageStatus::default_named(),
            cursor_seq: 0,
            cursor_image: None,
            named_cursor: None,
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
            last_input: now,
            busy_since: now,
            last_spawn: None,
            last_map: now,
            dirty: true,
            keymap,
            extra_keys: Vec::new(),
            held: Held::default(),
            key_repeat: false,
            viz: Default::default(),
            under_pointer: None,
            click_in_grab: false,
            clip_mimes: None,
            dnd_icon: None,
            xw: Xw::default(),
            log: EventLog::default(),
            seen_windows: Vec::new(),
        };
        let mut session = session;
        let kb = session.seat.get_keyboard().unwrap();
        let text = session.keymap.text.clone();
        kb.set_keymap_from_string(&mut session, text).context("setting keymap")?;
        if cfg.xwayland {
            session.start_xwayland()?;
        }
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

    /// Mapped windows (not counting X menus and tooltips).
    pub fn windows(&self) -> usize {
        self.space.elements().filter(|w| !crate::xwayland::is_or(w)).count()
    }

    /// Frame interval of the output.
    pub fn refresh_interval(&self) -> Duration {
        let mhz = self.output.current_mode().map(|m| m.refresh).unwrap_or(60_000).max(1);
        Duration::from_secs_f64(1000.0 / mhz as f64)
    }
}
