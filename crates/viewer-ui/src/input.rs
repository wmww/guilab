//! Host-side types the UI reads and writes: input in arrival order, cursors, window ops.
//! The viewer crate fills and applies them from its Wayland window.

/// Host input in arrival order: egui events, and raw keys (for forwarding to a session).
#[derive(Debug, Clone)]
pub enum HostInput {
    Egui(egui::Event),
    Key(RawKey),
    Gesture(Gesture),
    Touch(Touch),
}

/// Window management the UI asks for (with client-side decorations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOp {
    Move,
    /// Edges: -1 = left/top, 1 = right/bottom, 0 = neither.
    Resize(i8, i8),
    ToggleMaximize,
    Minimize,
    Close,
}

/// A touch point on the host (surface points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Touch {
    pub id: i32,
    pub phase: egui::TouchPhase,
    pub pos: egui::Pos2,
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
    pub fn same(&self, o: &HostCursor) -> bool {
        match (self, o) {
            (HostCursor::Icon(a), HostCursor::Icon(b)) => a == b,
            (HostCursor::Image(a), HostCursor::Image(b)) => a.id == b.id,
            _ => false,
        }
    }
}

