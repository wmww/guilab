//! Element geometry in canvas px: bounds, hit testing, key points and reshaping. `scale` is
//! the body's scale (stroke width and text padding depend on it).

use crate::model::{Element, Kind, Pt, Rect, TEXT_PAD, pt, STROKE};
use crate::text::{Fonts, TextLayout};

/// A handle the move tool can drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Bounding box corner: 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left.
    Corner(u8),
    /// Line/arrow end: 0 the first point (tail), 1 the second (tip).
    End(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyPoint {
    pub key: Key,
    pub at: Pt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
}

/// Smallest box side a corner drag can make.
pub const MIN_SIDE: f32 = 4.0;

fn corners(r: &Rect) -> [Pt; 4] {
    let (a, b) = (r.min, r.max());
    [a, pt(b.x, a.y), b, pt(a.x, b.y)]
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f32 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t - p).len()
}

impl Element {
    /// The text's layout and its backing rect (text rect + padding).
    pub fn text_layout(&self, scale: f32, fonts: &Fonts) -> Option<(TextLayout, Rect)> {
        let Kind::Text { at, size, width, text } = &self.kind else { return None };
        let l = fonts.layout(text, *size, *width);
        let r = Rect::new(at.x, at.y, l.size[0], l.size[1]).expand(TEXT_PAD * scale);
        Some((l, r))
    }

    /// The area the element covers: strokes include half their width, text its backing.
    pub fn bounds(&self, scale: f32, fonts: &Fonts) -> Rect {
        let half = STROKE * scale / 2.0;
        match &self.kind {
            Kind::Pen(p) => Rect::bounding(p).expand(half),
            Kind::Box(r) | Kind::Ellipse(r) => r.expand(half),
            Kind::Line(a, b) => Rect::from_corners(*a, *b).expand(half),
            // The head is wider than the stroke.
            Kind::Arrow(a, b) => {
                let head = crate::draw::arrow_head(*a, *b, scale);
                let mut pts = vec![*a, *b];
                if let Some((_, h)) = head {
                    pts.extend(h);
                }
                Rect::bounding(&pts).expand(half)
            }
            Kind::Text { .. } => self.text_layout(scale, fonts).unwrap().1,
            Kind::Raster { dst, .. } | Kind::Svg { dst, .. } => *dst,
        }
    }

    /// The rect key points sit on and reshapes act on (no stroke width or padding, except
    /// text, whose box is its backing).
    pub fn frame(&self, scale: f32, fonts: &Fonts) -> Rect {
        match &self.kind {
            Kind::Pen(p) => Rect::bounding(p),
            Kind::Box(r) | Kind::Ellipse(r) | Kind::Raster { dst: r, .. } | Kind::Svg { dst: r, .. } => *r,
            Kind::Line(a, b) | Kind::Arrow(a, b) => Rect::from_corners(*a, *b),
            Kind::Text { .. } => self.text_layout(scale, fonts).unwrap().1,
        }
    }

    /// Whether `p` is on the element: within `tol` of a stroke's edge, inside a text's
    /// backing, or inside a picture.
    pub fn hit(&self, p: Pt, tol: f32, scale: f32, fonts: &Fonts) -> bool {
        let t = tol + STROKE * scale / 2.0;
        match &self.kind {
            Kind::Pen(pts) => match pts.len() {
                0 => false,
                1 => (pts[0] - p).len() <= t,
                _ => pts.windows(2).any(|w| seg_dist(p, w[0], w[1]) <= t),
            },
            Kind::Line(a, b) => seg_dist(p, *a, *b) <= t,
            Kind::Arrow(a, b) => {
                seg_dist(p, *a, *b) <= t
                    || crate::draw::arrow_head(*a, *b, scale).is_some_and(|(_, h)| in_triangle(p, h) || seg_dist(p, h[1], h[2]) <= t)
            }
            Kind::Box(r) => {
                let c = corners(r);
                (0..4).any(|i| seg_dist(p, c[i], c[(i + 1) % 4]) <= t)
            }
            Kind::Ellipse(r) => {
                let (c, rx, ry) = (r.center(), r.size.w / 2.0, r.size.h / 2.0);
                if rx.min(ry) <= t {
                    return r.expand(t).contains(p);
                }
                // Distance to the outline, approximated by the radial offset in px.
                let d = p - c;
                let n = (d.x / rx).hypot(d.y / ry);
                let dist = (n - 1.0).abs() * if n > 0.0 { d.len() / n } else { rx.min(ry) };
                dist <= t
            }
            Kind::Text { .. } => self.bounds(scale, fonts).expand(tol).contains(p),
            Kind::Raster { dst, .. } | Kind::Svg { dst, .. } => dst.expand(tol).contains(p),
        }
    }

    pub fn key_points(&self, scale: f32, fonts: &Fonts) -> Vec<KeyPoint> {
        match &self.kind {
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                vec![KeyPoint { key: Key::End(0), at: *a }, KeyPoint { key: Key::End(1), at: *b }]
            }
            _ => corners(&self.frame(scale, fonts))
                .into_iter()
                .enumerate()
                .map(|(i, at)| KeyPoint { key: Key::Corner(i as u8), at })
                .collect(),
        }
    }

    /// The element with key point `k` dragged to `to`. Corners resize against the opposite
    /// corner (min `MIN_SIDE`, never crossing it). Pictures keep their aspect ratio unless
    /// Shift is held; marks keep it only with Shift.
    pub fn drag_key(&self, k: Key, to: Pt, mods: Mods, scale: f32, fonts: &Fonts) -> Element {
        let mut out = self.clone();
        match (&mut out.kind, k) {
            (Kind::Line(a, b) | Kind::Arrow(a, b), Key::End(i)) => {
                if i == 0 {
                    *a = to
                } else {
                    *b = to
                }
            }
            (Kind::Text { .. }, Key::Corner(i)) => {
                // Scales by the height ratio, anchored at the opposite corner.
                let frame = self.frame(scale, fonts);
                let pad = TEXT_PAD * scale;
                let c = corners(&frame);
                let (anchor, moving) = (c[(i as usize + 2) % 4], c[i as usize % 4]);
                let sx = if moving.x >= anchor.x { 1.0 } else { -1.0 };
                let sy = if moving.y >= anchor.y { 1.0 } else { -1.0 };
                let new_h = ((to.y - anchor.y) * sy).max(MIN_SIDE + 2.0 * pad);
                let k = (new_h - 2.0 * pad) / (frame.size.h - 2.0 * pad).max(1.0);
                let (w, h) = ((frame.size.w - 2.0 * pad) * k + 2.0 * pad, new_h);
                let x = if sx > 0.0 { anchor.x } else { anchor.x - w };
                let y = if sy > 0.0 { anchor.y } else { anchor.y - h };
                return self.fit_to(Rect::new(x, y, w, h), scale, fonts);
            }
            (_, Key::Corner(i)) => {
                let frame = self.frame(scale, fonts);
                let keep = self.is_picture() != mods.shift;
                let r = corner_drag(&frame, i % 4, to, keep);
                return self.fit_to(r, scale, fonts);
            }
            _ => {}
        }
        out
    }

    pub fn translated(&self, d: Pt) -> Element {
        let mut out = self.clone();
        match &mut out.kind {
            Kind::Pen(p) => p.iter_mut().for_each(|q| *q = *q + d),
            Kind::Box(r) | Kind::Ellipse(r) | Kind::Raster { dst: r, .. } | Kind::Svg { dst: r, .. } => *r = r.translate(d),
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                *a = *a + d;
                *b = *b + d;
            }
            Kind::Text { at, .. } => *at = *at + d,
        }
        out
    }

    /// The element reshaped so its `frame` becomes `r`. Pen points scale into it (each axis on
    /// its own), lines map their ends, text scales `size` and `width` by the height ratio
    /// (its box can't stretch on one axis) and keeps the top-left of `r`.
    pub fn fit_to(&self, r: Rect, scale: f32, fonts: &Fonts) -> Element {
        let mut out = self.clone();
        let from = self.frame(scale, fonts);
        let map = |p: Pt| {
            let fx = if from.size.w > 0.0 { (p.x - from.min.x) / from.size.w } else { 0.0 };
            let fy = if from.size.h > 0.0 { (p.y - from.min.y) / from.size.h } else { 0.0 };
            pt(r.min.x + fx * r.size.w, r.min.y + fy * r.size.h)
        };
        match &mut out.kind {
            Kind::Pen(p) => p.iter_mut().for_each(|q| *q = map(*q)),
            Kind::Box(x) | Kind::Ellipse(x) | Kind::Raster { dst: x, .. } | Kind::Svg { dst: x, .. } => *x = r,
            Kind::Line(a, b) | Kind::Arrow(a, b) => {
                *a = map(*a);
                *b = map(*b);
            }
            Kind::Text { at, size, width, .. } => {
                let pad = TEXT_PAD * scale;
                let old_h = (from.size.h - 2.0 * pad).max(1.0);
                let k = ((r.size.h - 2.0 * pad).max(1.0) / old_h).max(0.01);
                *size *= k;
                if let Some(w) = width {
                    *w *= k;
                }
                *at = pt(r.min.x + pad, r.min.y + pad);
            }
        }
        out
    }
}

/// Moves corner `i` of `r` to `to`, keeping the opposite corner. Optionally keeps the aspect
/// ratio (the larger relative change wins).
fn corner_drag(r: &Rect, i: u8, to: Pt, keep_aspect: bool) -> Rect {
    let c = corners(r);
    let anchor = c[(i as usize + 2) % 4];
    let moving = c[i as usize];
    // Direction from the anchor to the dragged corner, per axis.
    let sx = if moving.x >= anchor.x { 1.0 } else { -1.0 };
    let sy = if moving.y >= anchor.y { 1.0 } else { -1.0 };
    let mut w = ((to.x - anchor.x) * sx).max(MIN_SIDE);
    let mut h = ((to.y - anchor.y) * sy).max(MIN_SIDE);
    if keep_aspect && r.size.w > 0.0 && r.size.h > 0.0 {
        let k = (w / r.size.w).max(h / r.size.h);
        w = (r.size.w * k).max(MIN_SIDE);
        h = (r.size.h * k).max(MIN_SIDE);
    }
    Rect::from_corners(anchor, pt(anchor.x + w * sx, anchor.y + h * sy))
}

fn in_triangle(p: Pt, t: [Pt; 3]) -> bool {
    let s = |a: Pt, b: Pt| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let (d1, d2, d3) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}
