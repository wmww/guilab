//! Session sizing from the viewer: "Resize to window" / "Lock size to window" (the
//! session's logical size follows the view) and resize handles around the session image.

use egui::{CursorIcon, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use guilab_kit::Theme;

/// Output pixel limits (as `Session::set_output` allows, with a usable minimum).
pub const MIN_PX: u32 = 32;
pub const MAX_PX: u32 = 16384;
/// Seconds between resizes sent while dragging or following the window (apps shouldn't be
/// flooded with configures).
pub const THROTTLE: f64 = 0.1;

/// Output pixels whose logical size (at `scale`) fills `view` (points = host logical px).
pub fn fit_pixels(view: Rect, scale: f64) -> (u32, u32) {
    let px = |pt: f32| ((pt.floor() as f64 * scale).round() as u32).clamp(MIN_PX, MAX_PX);
    (px(view.width()), px(view.height()))
}

/// Which edges a handle moves: -1 = left/top, 1 = right/bottom, 0 = neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub x: i8,
    pub y: i8,
}

impl Edge {
    pub fn cursor(self) -> CursorIcon {
        match (self.x, self.y) {
            (0, _) => CursorIcon::ResizeVertical,
            (_, 0) => CursorIcon::ResizeHorizontal,
            (x, y) if x == y => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        }
    }
}

/// The handle under `p`: the band just outside the image's edges and corners. Only where
/// the band fits inside `view` ("when there's room").
pub fn handle_at(t: &Theme, img: Rect, view: Rect, p: Pos2) -> Option<Edge> {
    let band = t.size.handle_band;
    if !img.expand(band).contains(p) || img.contains(p) || !view.shrink(1.0).contains(p) {
        return None;
    }
    let side = |v: f32, lo: f32, hi: f32| if v < lo { -1 } else if v > hi { 1 } else { 0 };
    let e = Edge { x: side(p.x, img.min.x, img.max.x), y: side(p.y, img.min.y, img.max.y) };
    room(band, img, view, e).then_some(e)
}

/// Every handle: corners and edges.
pub const EDGES: [Edge; 8] = [
    Edge { x: -1, y: -1 },
    Edge { x: 0, y: -1 },
    Edge { x: 1, y: -1 },
    Edge { x: 1, y: 0 },
    Edge { x: 1, y: 1 },
    Edge { x: 0, y: 1 },
    Edge { x: -1, y: 1 },
    Edge { x: -1, y: 0 },
];

/// The grab area of handle `e` (the band outside that edge or corner), if it has room.
pub fn handle_rect(t: &Theme, img: Rect, view: Rect, e: Edge) -> Option<Rect> {
    let band = t.size.handle_band;
    let span = |d: i8, lo: f32, hi: f32| match d {
        -1 => (lo - band, lo),
        1 => (hi, hi + band),
        _ => (lo, hi),
    };
    let (x0, x1) = span(e.x, img.min.x, img.max.x);
    let (y0, y1) = span(e.y, img.min.y, img.max.y);
    let r = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)).intersect(view.shrink(1.0));
    (room(band, img, view, e) && r.is_positive()).then_some(r)
}

/// Whether the handle for `e` fits in the view.
fn room(band: f32, img: Rect, view: Rect, e: Edge) -> bool {
    let ok = |d: i8, lo: f32, hi: f32, vlo: f32, vhi: f32| match d {
        -1 => lo - band >= vlo,
        1 => hi + band <= vhi,
        _ => true,
    };
    ok(e.x, img.min.x, img.max.x, view.min.x, view.max.x) && ok(e.y, img.min.y, img.max.y, view.min.y, view.max.y)
}

/// Draws the eight handles that have room. `hot`: the hovered or dragged one.
pub fn paint_handles(painter: &egui::Painter, t: &Theme, img: Rect, view: Rect, hot: Option<Edge>) {
    let s = &t.size;
    let (base, lit) = (t.colors.handle, t.colors.handle_hot);
    for x in -1i8..=1 {
        for y in -1i8..=1 {
            let e = Edge { x, y };
            if (x, y) == (0, 0) || !room(s.handle_band, img, view, e) {
                continue;
            }
            let color = if hot == Some(e) { lit } else { base };
            let o = s.handle_gap + s.handle_w / 2.0;
            let at = |d: i8, lo: f32, hi: f32| match d {
                -1 => lo - o,
                1 => hi + o,
                _ => (lo + hi) / 2.0,
            };
            let c = pos2(at(x, img.min.x, img.max.x), at(y, img.min.y, img.max.y));
            let stroke = Stroke::new(s.handle_w, color);
            let half = s.handle_len / 2.0;
            match (x, y) {
                (0, _) => {
                    painter.line_segment([c - vec2(half, 0.0), c + vec2(half, 0.0)], stroke);
                }
                (_, 0) => {
                    painter.line_segment([c - vec2(0.0, half), c + vec2(0.0, half)], stroke);
                }
                _ => {
                    // An L hugging the corner.
                    let arm = half * 0.7;
                    let (dx, dy) = (-(x as f32), -(y as f32));
                    painter.add(egui::Shape::line(vec![c + vec2(dx * arm, 0.0), c, c + vec2(0.0, dy * arm)], stroke));
                }
            }
        }
    }
}

/// A handle drag in progress.
#[derive(Clone, Copy, Debug)]
pub struct Resizing {
    pub edge: Edge,
    start: Pos2,
    start_size: (u32, u32),
    /// Session pixels per point (fixed during the drag: the zoom doesn't change).
    k: f32,
    /// The image point that stays put, as a fraction of its size, and where it is on screen.
    frac: Vec2,
    anchor: Pos2,
    /// Fit was on before (restored after).
    pub was_fit: bool,
    /// The size the pointer asks for now.
    pub size: (u32, u32),
    /// Last size sent, and when (egui time).
    pub sent: Option<((u32, u32), f64)>,
}

impl Resizing {
    pub fn new(edge: Edge, start: Pos2, img: Rect, size: (u32, u32), was_fit: bool) -> Resizing {
        let frac = vec2(if edge.x < 0 { 1.0 } else if edge.x > 0 { 0.0 } else { 0.5 }, if edge.y < 0 { 1.0 } else if edge.y > 0 { 0.0 } else { 0.5 });
        let anchor = img.min + frac * img.size();
        Resizing { edge, start, start_size: size, k: size.0 as f32 / img.width(), frac, anchor, was_fit, size, sent: None }
    }

    pub fn moved(&mut self, p: Pos2) {
        let d = (p - self.start) * self.k;
        let f = |s: u32, e: i8, d: f32| ((s as f32 + e as f32 * d).round() as i64).clamp(MIN_PX as i64, MAX_PX as i64) as u32;
        self.size = (f(self.start_size.0, self.edge.x, d.x), f(self.start_size.1, self.edge.y, d.y));
    }

    /// Where an image of `size_pt` points goes so the anchor stays put.
    pub fn place(&self, size_pt: Vec2) -> Rect {
        Rect::from_min_size(self.anchor - self.frac * size_pt, size_pt)
    }

    /// The target size's rect on screen.
    pub fn target(&self) -> Rect {
        self.place(vec2(self.size.0 as f32, self.size.1 as f32) / self.k)
    }

    /// Whether to send the current size now (throttled).
    pub fn due(&self, now: f64) -> bool {
        self.sent.is_none_or(|(s, t)| s != self.size && now - t >= THROTTLE)
    }
}
