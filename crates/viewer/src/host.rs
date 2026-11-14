//! The host window: an sctk client on its own Wayland connection, presenting frames that
//! the shared `GlesRenderer` draws (dmabuf, or shm as a CPU-copy fallback), and turning host
//! input into egui events plus raw key codes.
//!
//! Nothing here knows about guilab sessions; it's an egui host for any smithay-based
//! program. All handlers only record state; the owner reads it after dispatching.

use std::{
    os::{fd::AsFd, unix::net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow};
use sctk::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_dmabuf, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    dmabuf::{DmabufFeedback, DmabufHandler, DmabufState},
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers, RepeatInfo},
        pointer::{PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager},
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler, slot::{Buffer as ShmBuffer, SlotPool}},
};
use smithay::{
    backend::{
        allocator::{
            Fourcc, Modifier, Swapchain,
            dmabuf::{AsDmabuf, Dmabuf},
            gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice},
            Buffer as _,
        },
        drm::{DrmDeviceFd, DrmNode},
        renderer::{Bind, ExportMem, Frame as _, Offscreen, Renderer as _, gles::{GlesRenderer, GlesTexture}},
    },
    utils::{Rectangle, Transform},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_buffer::WlBuffer, wl_keyboard::WlKeyboard, wl_output, wl_pointer::WlPointer, wl_seat::WlSeat, wl_shm, wl_surface::WlSurface},
};
use wayland_protocols::wp::{
    cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    fractional_scale::v1::client::{wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, wp_fractional_scale_v1::{self, WpFractionalScaleV1}},
    linux_dmabuf::zv1::client::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1},
    pointer_gestures::zv1::client::{
        zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
        zwp_pointer_gesture_swipe_v1::{self, ZwpPointerGestureSwipeV1},
        zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

/// A connected host display, made on a helper thread (the roundtrip blocks).
pub struct Connected {
    pub conn: Connection,
    pub globals: GlobalList,
    pub queue: EventQueue<HostState>,
}

/// Connects to the Wayland socket at `path` and reads its globals. Blocks.
pub fn connect(path: &Path) -> anyhow::Result<Connected> {
    let stream = UnixStream::connect(path).with_context(|| format!("connecting to {}", path.display()))?;
    let conn = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<HostState>(&conn).context("reading host globals")?;
    Ok(Connected { conn, globals, queue })
}

/// Host input in arrival order: egui events, and raw keys (for forwarding to a session).
#[derive(Debug, Clone)]
pub enum HostInput {
    Egui(egui::Event),
    Key(RawKey),
    Gesture(Gesture),
}

/// A touchpad gesture on the host (zwp_pointer_gestures). Deltas in surface points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    PinchBegin { fingers: u32 },
    /// `scale` is absolute (vs. begin), `rotation` relative degrees.
    PinchUpdate { dx: f64, dy: f64, scale: f64, rotation: f64 },
    PinchEnd { cancelled: bool },
    SwipeBegin { fingers: u32 },
    SwipeUpdate { dx: f64, dy: f64 },
    SwipeEnd { cancelled: bool },
}

/// A key on the host keyboard, as a raw evdev code.
#[derive(Debug, Clone, Copy)]
pub struct RawKey {
    pub code: u32,
    pub pressed: bool,
}

/// A cursor image for the host pointer.
pub struct CursorImage {
    /// Unique per image content (images are re-sent only when it changes).
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8.
    pub rgba: Vec<u8>,
    /// Hotspot in image pixels.
    pub hotspot: (i32, i32),
    /// Image pixels per logical pixel: the cursor shows at size / scale host points.
    pub scale: f64,
}

/// What the host pointer shows over the window.
#[derive(Clone)]
pub enum HostCursor {
    Icon(egui::CursorIcon),
    Image(std::sync::Arc<CursorImage>),
}

impl HostCursor {
    fn same(&self, o: &HostCursor) -> bool {
        match (self, o) {
            (HostCursor::Icon(a), HostCursor::Icon(b)) => a == b,
            (HostCursor::Image(a), HostCursor::Image(b)) => a.id == b.id,
            _ => false,
        }
    }
}

/// The surface and buffers for image cursors.
struct CursorSurface {
    surface: WlSurface,
    viewport: Option<WpViewport>,
    pool: SlotPool,
    /// The attached buffer (kept alive while the host may read it).
    buffer: Option<ShmBuffer>,
}

/// Whether the host takes our dmabufs (see `HostState::start_probe`).
enum Probe {
    Untried,
    /// Sent; the buffer stays alive until the answer.
    Pending { since: Instant, _buf: GbmBuffer },
    Ok,
    Failed,
}

/// How frames reach the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Dmabuf,
    Shm,
}

/// A host wl_buffer we own; destroyed with its swapchain slot.
struct OwnedBuffer(WlBuffer);
impl Drop for OwnedBuffer {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

enum Present {
    Dmabuf {
        swapchain: Swapchain<GbmAllocator<DrmDeviceFd>>,
        /// Slots the host still reads from, until `wl_buffer.release`.
        busy: Vec<smithay::backend::allocator::Slot<GbmBuffer>>,
    },
    Shm {
        pool: SlotPool,
        buffers: Vec<ShmBuffer>,
        target: Option<GlesTexture>,
    },
}

pub struct HostState {
    registry: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    viewporter: Option<WpViewporter>,
    cursor_surface: Option<CursorSurface>,
    _xdg: XdgShell,
    shm: Shm,
    dmabuf: DmabufState,
    window: Window,
    qh: QueueHandle<HostState>,
    viewport: Option<WpViewport>,
    fractional: Option<WpFractionalScaleV1>,
    gestures: Option<ZwpPointerGesturesV1>,
    pinch: Option<ZwpPointerGesturePinchV1>,
    swipe: Option<ZwpPointerGestureSwipeV1>,
    cursor_shapes: Option<CursorShapeManager>,
    cursor_device: Option<WpCursorShapeDeviceV1>,
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    /// Serial of the last pointer enter (`set_cursor` needs it).
    pointer_serial: u32,
    /// What the pointer should show (re-applied on enter).
    cursor: Option<HostCursor>,
    present: Option<Present>,
    feedback: Option<DmabufFeedback>,
    /// From the host's `zwp_linux_dmabuf_feedback_v1`, once known: whether it can take our dmabufs.
    gbm: Option<GbmDevice<DrmDeviceFd>>,
    node: Option<DrmNode>,
    force_shm: bool,
    probe: Probe,

    /// Window size in logical (surface) pixels, once configured.
    pub size: Option<(u32, u32)>,
    /// Host scale (fractional if the host supports it).
    pub scale: f64,
    pub closed: bool,
    frame_pending: bool,
    /// When the first frame was wanted while dmabuf feedback hadn't arrived (we wait 500 ms).
    first_frame_wanted: Option<Instant>,
    /// Something changed that needs a redraw (input, resize, scale).
    pub dirty: bool,
    /// Recorded input, consumed by `take_input`.
    events: Vec<HostInput>,
    pub modifiers: egui::Modifiers,
    pub focused: bool,
    /// The keyboard left since the last `take_input`.
    pub keyboard_left: bool,
    pointer_pos: egui::Pos2,
    pub buffer_kind: Option<BufferKind>,
    /// Key repeat from the host compositor: (keys per second, delay ms); None = off.
    repeat_info: Option<(u32, u32)>,
    /// The key repeating for egui (session apps repeat keys themselves).
    repeating: Option<Repeat>,
}

struct Repeat {
    code: u32,
    ev: KeyEvent,
    next: Instant,
}

/// Same physical GPU? Render and card nodes of one device share `/sys/dev/char/M:m/device`.
fn same_gpu(a: libc::dev_t, b: libc::dev_t) -> bool {
    if a == b {
        return true;
    }
    let dev = |d: libc::dev_t| {
        std::fs::canonicalize(format!("/sys/dev/char/{}:{}/device", libc::major(d), libc::minor(d))).ok()
    };
    matches!((dev(a), dev(b)), (Some(x), Some(y)) if x == y)
}

impl HostState {
    /// Creates the window. `gbm`/`node`: the renderer's device, for dmabuf presentation.
    pub fn new(
        c: &Connected,
        title: &str,
        app_id: &str,
        gbm: Option<GbmDevice<DrmDeviceFd>>,
        node: Option<DrmNode>,
        force_shm: bool,
    ) -> anyhow::Result<HostState> {
        let qh = c.queue.handle();
        let g = &c.globals;
        let compositor = CompositorState::bind(g, &qh).map_err(|e| anyhow!("host has no wl_compositor: {e}"))?;
        let xdg = XdgShell::bind(g, &qh).map_err(|e| anyhow!("host has no xdg_wm_base: {e}"))?;
        let shm = Shm::bind(g, &qh).map_err(|e| anyhow!("host has no wl_shm: {e}"))?;
        let dmabuf = DmabufState::new(g, &qh);
        if dmabuf.version().is_some_and(|v| v >= 4) && !force_shm {
            dmabuf.get_default_feedback(&qh).ok();
        }
        let surface = compositor.create_surface(&qh);
        // egui draws GL-style (row 0 at the bottom); the host flips it back.
        surface.set_buffer_transform(wl_output::Transform::Flipped180);
        let viewporter = g.bind::<WpViewporter, _, _>(&qh, 1..=1, ()).ok();
        let viewport = viewporter.as_ref().map(|v| v.get_viewport(&surface, &qh, ()));
        let fractional = match (&viewport, g.bind::<WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ())) {
            (Some(_), Ok(m)) => Some(m.get_fractional_scale(&surface, &qh, ())),
            _ => None,
        };
        let window = xdg.create_window(surface, WindowDecorations::RequestServer, &qh);
        window.set_title(title);
        window.set_app_id(app_id);
        window.set_min_size(Some((320, 240)));
        window.commit();
        Ok(HostState {
            registry: RegistryState::new(g),
            seat_state: SeatState::new(g, &qh),
            output_state: OutputState::new(g, &qh),
            gestures: g.bind::<ZwpPointerGesturesV1, _, _>(&qh, 1..=3, ()).ok(),
            cursor_shapes: CursorShapeManager::bind(g, &qh).ok(),
            compositor,
            viewporter,
            cursor_surface: None,
            _xdg: xdg,
            shm,
            dmabuf,
            window,
            qh,
            viewport,
            fractional,
            pinch: None,
            swipe: None,
            cursor_device: None,
            keyboard: None,
            pointer: None,
            pointer_serial: 0,
            cursor: None,
            present: None,
            feedback: None,
            gbm,
            node,
            force_shm,
            probe: Probe::Untried,
            size: None,
            scale: 1.0,
            closed: false,
            frame_pending: false,
            first_frame_wanted: None,
            dirty: true,
            events: Vec::new(),
            modifiers: Default::default(),
            focused: false,
            keyboard_left: false,
            pointer_pos: egui::Pos2::ZERO,
            buffer_kind: None,
            repeat_info: Some((25, 600)),
            repeating: None,
        })
    }

    /// Configured and not waiting for a frame callback.
    pub fn can_draw(&self) -> bool {
        self.size.is_some() && !self.frame_pending
    }

    /// Whether the host sends touchpad gestures (pinch, swipe).
    pub fn has_gestures(&self) -> bool {
        self.pinch.is_some()
    }

    pub fn has_input(&self) -> bool {
        !self.events.is_empty() || self.keyboard_left
    }

    /// Takes the recorded input, up to where egui must run a pass before seeing the rest
    /// (the rest stays for the next pass):
    /// - after a button press or release: a click moves focus, and the keys typed right
    ///   after it must be routed by the new focus; egui also detects one click per pass;
    /// - before a press that follows motion: egui needs a pass with the pointer there
    ///   (hover) first, or the click is lost when it comes with the pointer's first move.
    pub fn take_input(&mut self) -> Vec<HostInput> {
        let mut moved = false;
        let mut end = self.events.len();
        for (i, e) in self.events.iter().enumerate() {
            match e {
                HostInput::Egui(egui::Event::PointerButton { pressed, .. }) => {
                    end = if *pressed && moved { i } else { i + 1 };
                    break;
                }
                HostInput::Egui(egui::Event::PointerMoved(_)) => moved = true,
                _ => {}
            }
        }
        self.events.drain(..end).collect()
    }

    /// Buffer size in physical pixels.
    pub fn buffer_size(&self) -> (u32, u32) {
        let (w, h) = self.size.unwrap_or((1, 1));
        ((w as f64 * self.scale).round().max(1.0) as u32, (h as f64 * self.scale).round().max(1.0) as u32)
    }

    pub fn set_cursor(&mut self, cursor: HostCursor) {
        if self.cursor.as_ref().is_some_and(|c| c.same(&cursor)) {
            return;
        }
        self.cursor = Some(cursor);
        self.apply_cursor();
    }

    /// Sends `self.cursor` to the host (needs a pointer serial).
    fn apply_cursor(&mut self) {
        let (Some(ptr), Some(cursor)) = (&self.pointer, &self.cursor) else { return };
        let serial = self.pointer_serial;
        match cursor {
            HostCursor::Icon(icon) => match (cursor_shape(*icon), &self.cursor_device) {
                (Some(s), Some(d)) => d.set_shape(serial, s),
                (None, _) => ptr.set_cursor(serial, None, 0, 0),
                // No cursor-shape protocol: leave the host's cursor alone.
                _ => {}
            },
            HostCursor::Image(img) => {
                let img = img.clone();
                let ptr = ptr.clone();
                match self.cursor_image(&img) {
                    Ok((surface, hx, hy)) => ptr.set_cursor(serial, Some(&surface), hx, hy),
                    Err(e) => tracing::warn!("viewer cursor image: {e:#}"),
                }
            }
        }
    }

    /// Puts `img` on the cursor surface; returns it with the hotspot in surface points.
    fn cursor_image(&mut self, img: &CursorImage) -> anyhow::Result<(WlSurface, i32, i32)> {
        if self.cursor_surface.is_none() {
            let surface = self.compositor.create_surface(&self.qh);
            let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(&surface, &self.qh, ()));
            let pool = SlotPool::new(64 * 64 * 4, &self.shm).context("cursor shm pool")?;
            self.cursor_surface = Some(CursorSurface { surface, viewport, pool, buffer: None });
        }
        let c = self.cursor_surface.as_mut().unwrap();
        let (w, h) = (img.width.max(1), img.height.max(1));
        let (buf, canvas) = c.pool.create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)?;
        // Premultiplied RGBA -> little-endian ARGB (BGRA bytes).
        for (d, s) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(img.rgba.as_chunks::<4>().0) {
            *d = [s[2], s[1], s[0], s[3]];
        }
        buf.attach_to(&c.surface)?;
        // The cursor's logical size is its pixel size over its scale, whatever the host's
        // scale (real cursors aren't zoomed).
        let scale = img.scale.max(0.1);
        let (lw, lh) = (((w as f64 / scale).round() as i32).max(1), ((h as f64 / scale).round() as i32).max(1));
        let k = if let Some(vp) = &c.viewport {
            vp.set_destination(lw, lh);
            scale
        } else {
            // Integer buffer scale only if it divides the image.
            let s = scale.round() as i32;
            let s = if (scale - s as f64).abs() < 0.01 && s >= 1 && w as i32 % s == 0 && h as i32 % s == 0 { s } else { 1 };
            c.surface.set_buffer_scale(s);
            s as f64
        };
        c.surface.damage_buffer(0, 0, w as i32, h as i32);
        c.surface.commit();
        c.buffer = Some(buf);
        Ok((c.surface.clone(), (img.hotspot.0 as f64 / k).round() as i32, (img.hotspot.1 as f64 / k).round() as i32))
    }

    /// Picks dmabuf or shm once the host's dmabuf feedback is in (or can't come) and, for
    /// dmabuf, the host accepted a probe buffer. Ok(false): not decided yet.
    fn ensure_present(&mut self, renderer: &GlesRenderer) -> anyhow::Result<bool> {
        if self.present.is_some() {
            return Ok(true);
        }
        let (w, h) = self.buffer_size();
        let mods = self.dmabuf_modifiers(renderer);
        // The host may reject a format/modifier it advertised; `create_immed` would then be
        // a protocol error that closes the connection. So first create one buffer the
        // asynchronous way and see.
        let mods = match (&mods, &self.probe) {
            (Some(m), Probe::Untried) => {
                self.probe = match self.start_probe(m) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::info!("viewer dmabuf probe: {e:#}; using shm");
                        Probe::Failed
                    }
                };
                if matches!(self.probe, Probe::Pending { .. }) {
                    return Ok(false);
                }
                None
            }
            (Some(_), Probe::Pending { since, .. }) if since.elapsed() < Duration::from_secs(1) => return Ok(false),
            (Some(_), Probe::Pending { .. }) => {
                tracing::info!("host didn't answer the dmabuf probe; using shm");
                self.probe = Probe::Failed;
                None
            }
            (Some(m), Probe::Ok) => Some(m.clone()),
            _ => None,
        };
        if let Some(mods) = mods {
            let gbm = self.gbm.clone().unwrap();
            let alloc = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING);
            self.present = Some(Present::Dmabuf { swapchain: Swapchain::new(alloc, w, h, Fourcc::Argb8888, mods), busy: Vec::new() });
            self.buffer_kind = Some(BufferKind::Dmabuf);
        } else {
            let pool = SlotPool::new((w * h * 4) as usize, &self.shm).context("shm pool")?;
            self.present = Some(Present::Shm { pool, buffers: Vec::new(), target: None });
            self.buffer_kind = Some(BufferKind::Shm);
        }
        tracing::info!("viewer presents via {:?}", self.buffer_kind.unwrap());
        Ok(true)
    }

    /// Sends a small dmabuf to the host with `zwp_linux_buffer_params_v1.create`; the
    /// answer (`created`/`failed`) settles `self.probe`.
    fn start_probe(&mut self, mods: &[Modifier]) -> anyhow::Result<Probe> {
        let gbm = self.gbm.clone().context("no gbm device")?;
        let mut alloc = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING);
        let buf = smithay::backend::allocator::Allocator::create_buffer(&mut alloc, 64, 64, Fourcc::Argb8888, mods).map_err(|e| anyhow!("allocating: {e}"))?;
        let dmabuf = buf.export().map_err(|e| anyhow!("exporting: {e}"))?;
        let params = self.dmabuf.create_params(&self.qh).map_err(|e| anyhow!("params: {e}"))?;
        let modifier = u64::from(dmabuf.format().modifier);
        for (i, (fd, (offset, stride))) in dmabuf.handles().zip(dmabuf.offsets().zip(dmabuf.strides())).enumerate() {
            params.add(fd.as_fd(), i as u32, offset, stride, modifier);
        }
        params.create(64, 64, Fourcc::Argb8888 as u32, zwp_linux_buffer_params_v1::Flags::empty());
        Ok(Probe::Pending { since: Instant::now(), _buf: buf })
    }

    /// Modifiers for ARGB8888 dmabufs the host can read and we can render to, if the host
    /// is on our GPU. None: use shm.
    fn dmabuf_modifiers(&self, renderer: &GlesRenderer) -> Option<Vec<Modifier>> {
        let fb = self.feedback.as_ref()?;
        let node = self.node?;
        self.gbm.as_ref()?;
        if !same_gpu(fb.main_device(), node.dev_id()) {
            tracing::info!("host compositor is on another GPU; using shm");
            return None;
        }
        let table = fb.format_table();
        let render = renderer.egl_context().dmabuf_render_formats();
        let mut mods: Vec<Modifier> = fb
            .tranches()
            .iter()
            .flat_map(|t| t.formats.iter())
            .filter_map(|i| table.get(*i as usize))
            .filter(|f| f.format == Fourcc::Argb8888 as u32)
            .map(|f| Modifier::from(f.modifier))
            .filter(|m| render.iter().any(|r| r.code == Fourcc::Argb8888 && r.modifier == *m))
            .collect();
        mods.sort_by_key(|m| u64::from(*m));
        mods.dedup();
        (!mods.is_empty()).then_some(mods)
    }

    /// Whether we still wait for dmabuf feedback before the first frame.
    fn awaiting_feedback(&self) -> bool {
        self.present.is_none()
            && self.feedback.is_none()
            && !self.force_shm
            && self.gbm.is_some()
            && self.dmabuf.version().is_some_and(|v| v >= 4)
            && self.first_frame_wanted.is_none_or(|t| t.elapsed().as_millis() < 500)
    }

    /// Renders one frame with `paint` (called with the GL context current and the target
    /// bound, after a clear) and commits it. Returns false if nothing could be drawn now.
    pub fn present(&mut self, renderer: &mut GlesRenderer, paint: impl FnOnce((u32, u32))) -> anyhow::Result<bool> {
        if !self.can_draw() {
            return Ok(false);
        }
        if self.awaiting_feedback() {
            self.first_frame_wanted.get_or_insert_with(Instant::now);
            return Ok(false);
        }
        if !self.ensure_present(renderer)? {
            return Ok(false);
        }
        let (w, h) = self.buffer_size();
        let size = (w as i32, h as i32);
        let surface = self.window.wl_surface().clone();
        let qh = self.qh.clone();
        match self.present.as_mut().unwrap() {
            Present::Dmabuf { swapchain, busy } => {
                swapchain.resize(w, h);
                let Some(slot) = swapchain.acquire().map_err(|e| anyhow!("allocating host buffer: {e}"))? else {
                    return Ok(false); // all buffers still with the host
                };
                let mut dmabuf: Dmabuf = slot.export().map_err(|e| anyhow!("exporting dmabuf: {e}"))?;
                render_into(renderer, &mut dmabuf, size, paint)?;
                if slot.userdata().get::<OwnedBuffer>().is_none() {
                    let params = self.dmabuf.create_params(&qh).map_err(|e| anyhow!("dmabuf params: {e}"))?;
                    let modifier = u64::from(dmabuf.format().modifier);
                    for (i, (fd, (offset, stride))) in dmabuf.handles().zip(dmabuf.offsets().zip(dmabuf.strides())).enumerate() {
                        params.add(fd.as_fd(), i as u32, offset, stride, modifier);
                    }
                    let (buf, params) = params.create_immed(w as i32, h as i32, Fourcc::Argb8888 as u32, zwp_linux_buffer_params_v1::Flags::empty(), &qh);
                    params.destroy();
                    slot.userdata().insert_if_missing(|| OwnedBuffer(buf));
                }
                let buf = &slot.userdata().get::<OwnedBuffer>().unwrap().0;
                surface.attach(Some(buf), 0, 0);
                swapchain.submitted(&slot);
                busy.push(slot);
            }
            Present::Shm { pool, buffers, target } => {
                if target.as_ref().is_none_or(|t| smithay::backend::renderer::Texture::size(t) != (size.0, size.1).into()) {
                    *target = Some(renderer.create_buffer(Fourcc::Abgr8888, (size.0, size.1).into())?);
                    buffers.clear();
                }
                let tex = target.as_mut().unwrap();
                let pixels = {
                    let mut fb = renderer.bind(tex)?;
                    paint_target(renderer, &mut fb, size, paint)?;
                    let map = renderer.copy_framebuffer(&fb, Rectangle::from_size((size.0, size.1).into()), Fourcc::Abgr8888)?;
                    drop(fb);
                    renderer.map_texture(&map)?.to_vec()
                };
                let stride = w as i32 * 4;
                let idx = match buffers.iter().position(|b| b.canvas(pool).is_some()) {
                    Some(i) => i,
                    None if buffers.len() < 3 => {
                        buffers.push(pool.create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)?.0);
                        buffers.len() - 1
                    }
                    None => return Ok(false),
                };
                let canvas = buffers[idx].canvas(pool).unwrap();
                // RGBA -> little-endian ARGB (BGRA bytes).
                for (d, s) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(pixels.as_chunks::<4>().0) {
                    d.copy_from_slice(&[s[2], s[1], s[0], 255]);
                }
                buffers[idx].attach_to(&surface)?;
            }
        }
        if let Some(vp) = &self.viewport {
            let (lw, lh) = self.size.unwrap();
            vp.set_destination(lw as i32, lh as i32);
        }
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.frame(&qh, surface.clone());
        surface.commit();
        self.frame_pending = true;
        self.dirty = false;
        Ok(true)
    }

    /// Drops the presentation buffers (they're re-made on the next frame).
    pub fn reset_buffers(&mut self) {
        self.present = None;
    }
}

/// Binds `target`, clears it and paints into it.
fn render_into(renderer: &mut GlesRenderer, target: &mut Dmabuf, size: (i32, i32), paint: impl FnOnce((u32, u32))) -> anyhow::Result<()> {
    let mut fb = renderer.bind(target)?;
    paint_target(renderer, &mut fb, size, paint)
}

/// Clears the bound target and runs `paint` inside a smithay frame (GL context current,
/// target bound), then waits for the GPU.
fn paint_target(
    renderer: &mut GlesRenderer,
    fb: &mut smithay::backend::renderer::gles::GlesTarget<'_>,
    size: (i32, i32),
    paint: impl FnOnce((u32, u32)),
) -> anyhow::Result<()> {
    let mut frame = renderer.render(fb, (size.0, size.1).into(), Transform::Normal)?;
    frame.clear([0.1, 0.1, 0.1, 1.0].into(), &[Rectangle::from_size((size.0, size.1).into())])?;
    frame.with_context(|_| paint((size.0 as u32, size.1 as u32)))?;
    frame.finish()?.wait().ok();
    Ok(())
}

fn cursor_shape(icon: egui::CursorIcon) -> Option<Shape> {
    use egui::CursorIcon as C;
    Some(match icon {
        C::None => return None,
        C::Default => Shape::Default,
        C::ContextMenu => Shape::ContextMenu,
        C::Help => Shape::Help,
        C::PointingHand => Shape::Pointer,
        C::Progress => Shape::Progress,
        C::Wait => Shape::Wait,
        C::Cell => Shape::Cell,
        C::Crosshair => Shape::Crosshair,
        C::Text => Shape::Text,
        C::VerticalText => Shape::VerticalText,
        C::Alias => Shape::Alias,
        C::Copy => Shape::Copy,
        C::Move => Shape::Move,
        C::NoDrop => Shape::NoDrop,
        C::NotAllowed => Shape::NotAllowed,
        C::Grab => Shape::Grab,
        C::Grabbing => Shape::Grabbing,
        C::AllScroll => Shape::AllScroll,
        C::ResizeHorizontal => Shape::EwResize,
        C::ResizeNeSw => Shape::NeswResize,
        C::ResizeNwSe => Shape::NwseResize,
        C::ResizeVertical => Shape::NsResize,
        C::ResizeEast => Shape::EResize,
        C::ResizeSouthEast => Shape::SeResize,
        C::ResizeSouth => Shape::SResize,
        C::ResizeSouthWest => Shape::SwResize,
        C::ResizeWest => Shape::WResize,
        C::ResizeNorthWest => Shape::NwResize,
        C::ResizeNorth => Shape::NResize,
        C::ResizeNorthEast => Shape::NeResize,
        C::ResizeColumn => Shape::ColResize,
        C::ResizeRow => Shape::RowResize,
        C::ZoomIn => Shape::ZoomIn,
        C::ZoomOut => Shape::ZoomOut,
    })
}

fn egui_button(b: u32) -> Option<egui::PointerButton> {
    Some(match b {
        0x110 => egui::PointerButton::Primary,
        0x111 => egui::PointerButton::Secondary,
        0x112 => egui::PointerButton::Middle,
        0x113 => egui::PointerButton::Extra1,
        0x114 => egui::PointerButton::Extra2,
        _ => return None,
    })
}

fn egui_key(k: Keysym) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match k {
        Keysym::Escape => K::Escape,
        Keysym::Tab | Keysym::ISO_Left_Tab => K::Tab,
        Keysym::BackSpace => K::Backspace,
        Keysym::Return | Keysym::KP_Enter => K::Enter,
        Keysym::space => K::Space,
        Keysym::Insert => K::Insert,
        Keysym::Delete => K::Delete,
        Keysym::Home => K::Home,
        Keysym::End => K::End,
        Keysym::Page_Up => K::PageUp,
        Keysym::Page_Down => K::PageDown,
        Keysym::Left => K::ArrowLeft,
        Keysym::Right => K::ArrowRight,
        Keysym::Up => K::ArrowUp,
        Keysym::Down => K::ArrowDown,
        _ => {
            let c = k.key_char()?.to_ascii_lowercase();
            return K::from_name(&c.to_string());
        }
    })
}

impl CompositorHandler for HostState {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, factor: i32) {
        if self.fractional.is_none() {
            surface.set_buffer_scale(factor);
            self.scale = factor as f64;
            self.dirty = true;
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {
        self.frame_pending = false;
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for HostState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for HostState {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.closed = true;
    }
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window, cfg: WindowConfigure, _: u32) {
        let (w, h) = self.size.unwrap_or((1280, 800));
        let size = (cfg.new_size.0.map_or(w, |v| v.get()), cfg.new_size.1.map_or(h, |v| v.get()));
        self.size = Some(size);
        self.dirty = true;
        // A configure wants a new buffer even if we're waiting for a frame callback.
        self.frame_pending = false;
    }
}

impl SeatHandler for HostState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: WlSeat, cap: Capability) {
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if cap == Capability::Pointer && self.pointer.is_none() {
            let Ok(ptr) = self.seat_state.get_pointer(qh, &seat) else { return };
            self.cursor_device = self.cursor_shapes.as_ref().map(|m| m.get_shape_device(&ptr, qh));
            self.pinch = self.gestures.as_ref().map(|g| g.get_pinch_gesture(&ptr, qh, ()));
            self.swipe = self.gestures.as_ref().map(|g| g.get_swipe_gesture(&ptr, qh, ()));
            self.pointer = Some(ptr);
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, cap: Capability) {
        if cap == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
            self.keyboard_left = true;
        }
        if cap == Capability::Pointer
            && let Some(p) = self.pointer.take()
        {
            p.release();
            self.cursor_device = None;
            self.pinch = None;
            self.swipe = None;
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl KeyboardHandler for HostState {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: &WlSurface, _: u32, _: &[u32], _: &[Keysym]) {
        self.focused = true;
        self.events.push(HostInput::Egui(egui::Event::WindowFocused(true)));
        self.dirty = true;
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: &WlSurface, _: u32) {
        self.focused = false;
        self.keyboard_left = true;
        self.repeating = None;
        self.modifiers = Default::default();
        self.events.push(HostInput::Egui(egui::Event::WindowFocused(false)));
        self.dirty = true;
    }
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, ev: KeyEvent) {
        self.key(ev, true);
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_repeat_info(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, info: RepeatInfo) {
        self.repeat_info = match info {
            RepeatInfo::Repeat { rate, delay } => Some((rate.get(), delay)),
            RepeatInfo::Disable => None,
        };
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, ev: KeyEvent) {
        self.key(ev, false);
    }
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, m: Modifiers, _: RawModifiers, _: u32) {
        self.modifiers = egui::Modifiers { alt: m.alt, ctrl: m.ctrl, shift: m.shift, mac_cmd: false, command: m.ctrl };
    }
}

impl HostState {
    fn key(&mut self, ev: KeyEvent, pressed: bool) {
        self.dirty = true;
        self.events.push(HostInput::Key(RawKey { code: ev.raw_code, pressed }));
        if pressed && !ev.keysym.is_modifier_key() {
            self.repeating = self.repeat_info.map(|(_, delay)| Repeat { code: ev.raw_code, ev: ev.clone(), next: Instant::now() + Duration::from_millis(delay as u64) });
        } else if !pressed && self.repeating.as_ref().is_some_and(|r| r.code == ev.raw_code) {
            self.repeating = None;
        }
        self.egui_key(&ev, pressed, false);
    }

    /// Repeats the held key for egui (the host compositor leaves repeat to clients). Called
    /// every server tick.
    pub fn tick_repeat(&mut self, now: Instant) {
        let (Some(r), Some((rate, _))) = (&mut self.repeating, self.repeat_info) else { return };
        if !self.focused || now < r.next {
            return;
        }
        let ev = r.ev.clone();
        let step = Duration::from_secs(1) / rate.max(1);
        // At most a few per tick, so a stall doesn't dump a burst.
        let mut n = 0;
        while r.next <= now && n < 3 {
            r.next += step;
            n += 1;
        }
        if r.next <= now {
            r.next = now + step;
        }
        for _ in 0..n {
            self.egui_key(&ev, true, true);
        }
        self.dirty = true;
    }

    fn egui_key(&mut self, ev: &KeyEvent, pressed: bool, repeat: bool) {
        if let Some(key) = egui_key(ev.keysym) {
            self.events.push(HostInput::Egui(egui::Event::Key { key, physical_key: None, pressed, repeat, modifiers: self.modifiers }));
        }
        if pressed
            && !self.modifiers.ctrl
            && !self.modifiers.alt
            && let Some(t) = ev.utf8.clone().filter(|t| !t.chars().any(char::is_control))
        {
            self.events.push(HostInput::Egui(egui::Event::Text(t)));
        }
    }
}

impl PointerHandler for HostState {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlPointer, events: &[PointerEvent]) {
        for ev in events {
            self.dirty = true;
            let pos = egui::pos2(ev.position.0 as f32, ev.position.1 as f32);
            match ev.kind {
                PointerEventKind::Enter { serial } => {
                    self.pointer_serial = serial;
                    // The host forgets our cursor on enter; set it again.
                    self.apply_cursor();
                    self.pointer_pos = pos;
                    self.events.push(HostInput::Egui(egui::Event::PointerMoved(pos)));
                }
                PointerEventKind::Leave { .. } => self.events.push(HostInput::Egui(egui::Event::PointerGone)),
                PointerEventKind::Motion { .. } => {
                    self.pointer_pos = pos;
                    self.events.push(HostInput::Egui(egui::Event::PointerMoved(pos)));
                }
                PointerEventKind::Press { button, .. } | PointerEventKind::Release { button, .. } => {
                    let pressed = matches!(ev.kind, PointerEventKind::Press { .. });
                    if let Some(button) = egui_button(button) {
                        self.events.push(HostInput::Egui(egui::Event::PointerButton { pos, button, pressed, modifiers: self.modifiers }));
                    }
                }
                PointerEventKind::Axis { horizontal: h, vertical: v, source, .. } => {
                    let wheel = matches!(source, Some(wayland_client::protocol::wl_pointer::AxisSource::Wheel))
                        || h.value120 != 0
                        || v.value120 != 0
                        || h.discrete != 0
                        || v.discrete != 0;
                    let notches = |a: &sctk::seat::pointer::AxisScroll| {
                        if a.value120 != 0 { a.value120 as f32 / 120.0 } else if a.discrete != 0 { a.discrete as f32 } else { a.absolute as f32 / 15.0 }
                    };
                    // egui: positive = scroll up/left; Wayland: positive = down/right.
                    let (unit, delta) = if wheel {
                        (egui::MouseWheelUnit::Line, egui::vec2(-notches(&h), -notches(&v)))
                    } else {
                        (egui::MouseWheelUnit::Point, egui::vec2(-h.absolute as f32, -v.absolute as f32))
                    };
                    let stop = h.stop || v.stop;
                    let phase = if stop { egui::TouchPhase::End } else { egui::TouchPhase::Move };
                    if delta != egui::Vec2::ZERO || stop {
                        self.events.push(HostInput::Egui(egui::Event::MouseWheel { unit, delta, phase, modifiers: self.modifiers }));
                    }
                }
            }
        }
    }
}

impl ShmHandler for HostState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl DmabufHandler for HostState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }
    fn dmabuf_feedback(&mut self, _: &Connection, _: &QueueHandle<Self>, proxy: &ZwpLinuxDmabufFeedbackV1, fb: DmabufFeedback) {
        self.feedback = Some(fb);
        proxy.destroy();
        self.dirty = true;
    }
    // Only the probe uses the asynchronous `create`.
    fn created(&mut self, _: &Connection, _: &QueueHandle<Self>, params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, buffer: WlBuffer) {
        buffer.destroy();
        params.destroy();
        self.probe = Probe::Ok;
        self.dirty = true;
    }
    fn failed(&mut self, _: &Connection, _: &QueueHandle<Self>, params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1) {
        tracing::info!("host rejected the dmabuf probe; using shm");
        params.destroy();
        self.probe = Probe::Failed;
        self.dirty = true;
    }
    fn released(&mut self, _: &Connection, _: &QueueHandle<Self>, buffer: &WlBuffer) {
        if let Some(Present::Dmabuf { busy, .. }) = &mut self.present
            && let Some(i) = busy.iter().position(|s| s.userdata().get::<OwnedBuffer>().is_some_and(|b| &b.0 == buffer))
        {
            busy.remove(i);
        }
    }
}

impl ProvidesRegistryState for HostState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<WpFractionalScaleV1, ()> for HostState {
    fn event(st: &mut Self, _: &WpFractionalScaleV1, ev: wp_fractional_scale_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = ev {
            st.scale = scale as f64 / 120.0;
            st.dirty = true;
        }
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for HostState {
    fn event(st: &mut Self, _: &ZwpPointerGesturePinchV1, ev: zwp_pointer_gesture_pinch_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_pointer_gesture_pinch_v1::Event as E;
        let g = match ev {
            E::Begin { fingers, .. } => Gesture::PinchBegin { fingers },
            E::Update { dx, dy, scale, rotation, .. } => Gesture::PinchUpdate { dx, dy, scale, rotation },
            E::End { cancelled, .. } => Gesture::PinchEnd { cancelled: cancelled != 0 },
            _ => return,
        };
        st.events.push(HostInput::Gesture(g));
        st.dirty = true;
    }
}

impl Dispatch<ZwpPointerGestureSwipeV1, ()> for HostState {
    fn event(st: &mut Self, _: &ZwpPointerGestureSwipeV1, ev: zwp_pointer_gesture_swipe_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_pointer_gesture_swipe_v1::Event as E;
        let g = match ev {
            E::Begin { fingers, .. } => Gesture::SwipeBegin { fingers },
            E::Update { dx, dy, .. } => Gesture::SwipeUpdate { dx, dy },
            E::End { cancelled, .. } => Gesture::SwipeEnd { cancelled: cancelled != 0 },
            _ => return,
        };
        st.events.push(HostInput::Gesture(g));
        st.dirty = true;
    }
}

delegate_noop!(HostState: WpViewporter);
delegate_noop!(HostState: WpViewport);
delegate_noop!(HostState: WpFractionalScaleManagerV1);
delegate_noop!(HostState: ZwpPointerGesturesV1);
delegate_compositor!(HostState);
delegate_output!(HostState);
delegate_shm!(HostState);
delegate_seat!(HostState);
delegate_keyboard!(HostState);
delegate_pointer!(HostState);
delegate_xdg_shell!(HostState);
delegate_xdg_window!(HostState);
delegate_dmabuf!(HostState);
delegate_registry!(HostState);

// `connect` runs on a helper thread and hands this over.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<Connected>();
};
