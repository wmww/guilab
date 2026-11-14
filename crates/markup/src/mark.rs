//! Marks (the things drawn on a screenshot) and how they turn into egui shapes. The same
//! shapes are painted on screen and rasterized into the composited image ([`crate::raster`]),
//! so what you see is what gets sent.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Shape, Stroke, Vec2, epaint::Galley, pos2, vec2};

/// Drawing tools. `Interact` = not drawing (the host uses pointer input itself). `Move`
/// drags marks, and the host pans the view when the press isn't on a mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Interact,
    Move,
    Pen,
    Box,
    Ellipse,
    Line,
    Arrow,
    Text,
}

impl Tool {
    pub const ALL: [Tool; 8] = [Tool::Interact, Tool::Move, Tool::Pen, Tool::Box, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Text];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Interact => "Use the app (no drawing)",
            Tool::Move => "Move marks, or drag to pan the view",
            Tool::Pen => "Pen",
            Tool::Box => "Box",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Text => "Text (or double-click with any tool)",
        }
    }
}

/// The palette.
pub const COLORS: [(Color32, &str); 6] = [
    (Color32::from_rgb(230, 40, 40), "Red"),
    (Color32::from_rgb(255, 200, 0), "Yellow"),
    (Color32::from_rgb(30, 170, 60), "Green"),
    (Color32::from_rgb(30, 110, 240), "Blue"),
    (Color32::from_rgb(255, 255, 255), "White"),
    (Color32::from_rgb(0, 0, 0), "Black"),
];

/// The palette name of a color ("Red"), if it's one of [`COLORS`].
pub fn color_name(c: Color32) -> Option<&'static str> {
    COLORS.iter().find(|(k, _)| *k == c).map(|(_, n)| *n)
}

/// Sizes in logical pixels; multiplied by the screenshot's scale (pixels per logical pixel).
pub const STROKE: f32 = 3.0;
pub const FONT: f32 = 18.0;
/// Padding around text, inside its backing.
pub const PAD: f32 = 4.0;
/// Text wraps at this many font sizes (so it follows the font size if that ever changes).
pub const WRAP_EMS: f32 = 16.0;
const ARROW_HEAD: f32 = 16.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Freehand polyline.
    Pen(Vec<Pos2>),
    /// Two opposite corners.
    Box(Pos2, Pos2),
    /// Bounding box corners.
    Ellipse(Pos2, Pos2),
    Line(Pos2, Pos2),
    /// From tail to tip.
    Arrow(Pos2, Pos2),
    /// Top-left of the text, the text, and its wrap width (all in screenshot pixels).
    Text(Pos2, String, f32),
}

/// One drawn thing, in screenshot pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: Kind,
    pub color: Color32,
}

/// Maps screenshot pixels to paint coordinates (`p * zoom + offset`).
#[derive(Clone, Copy, Debug)]
pub struct Xf {
    pub offset: Vec2,
    pub zoom: f32,
}

impl Xf {
    pub const IDENTITY: Xf = Xf { offset: Vec2::ZERO, zoom: 1.0 };

    pub fn apply(&self, p: Pos2) -> Pos2 {
        pos2(p.x * self.zoom, p.y * self.zoom) + self.offset
    }

    pub fn invert(&self, p: Pos2) -> Pos2 {
        let q = p - self.offset;
        pos2(q.x / self.zoom, q.y / self.zoom)
    }
}

/// Lays out text: (text, font size, color, wrap width) -> galley, in the target's coordinates.
pub type Layout<'a> = dyn FnMut(String, f32, Color32, f32) -> Arc<Galley> + 'a;

/// Backing behind text so it reads on any screenshot: translucent black, except behind very
/// dark text (of the palette, only black), which gets translucent white.
pub fn text_backing(c: Color32) -> Color32 {
    let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if lum < 50.0 { Color32::from_white_alpha(200) } else { Color32::from_black_alpha(170) }
}

impl Mark {
    /// Whether the mark is too small to be intentional (`min` in screenshot pixels).
    pub fn is_tiny(&self, min: f32) -> bool {
        let (a, b) = match &self.kind {
            Kind::Pen(pts) => {
                let r = Rect::from_points(pts);
                (r.min, r.max)
            }
            Kind::Box(a, b) | Kind::Ellipse(a, b) | Kind::Line(a, b) | Kind::Arrow(a, b) => (*a, *b),
            Kind::Text(_, t, _) => return t.trim().is_empty(),
        };
        (b - a).length() < min
    }

    pub fn translate(&mut self, d: Vec2) {
        match &mut self.kind {
            Kind::Pen(pts) => pts.iter_mut().for_each(|p| *p += d),
            Kind::Box(a, b) | Kind::Ellipse(a, b) | Kind::Line(a, b) | Kind::Arrow(a, b) => {
                *a += d;
                *b += d;
            }
            Kind::Text(p, ..) => *p += d,
        }
    }

    /// Whether screen point `p` is on the mark (within `tol` points of its stroke).
    /// `text_rect`: the text's backing on screen, for text marks.
    pub fn hit(&self, p: Pos2, xf: Xf, tol: f32, text_rect: Option<Rect>) -> bool {
        let q = |a: Pos2| xf.apply(a);
        let seg = |a: Pos2, b: Pos2| {
            let (a, b) = (q(a), q(b));
            let ab = b - a;
            let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
            (a + ab * t - p).length() <= tol
        };
        match &self.kind {
            Kind::Pen(pts) => pts.len() == 1 && (q(pts[0]) - p).length() <= tol || pts.windows(2).any(|w| seg(w[0], w[1])),
            Kind::Line(a, b) | Kind::Arrow(a, b) => seg(*a, *b),
            Kind::Box(a, b) => {
                let r = Rect::from_two_pos(q(*a), q(*b));
                r.expand(tol).contains(p) && !r.shrink(tol).contains(p)
            }
            Kind::Ellipse(a, b) => {
                let r = Rect::from_two_pos(q(*a), q(*b));
                let (c, rad) = (r.center(), r.size() / 2.0);
                // Distance from the outline, roughly: radial offset scaled to points.
                let d = p - c;
                let n = vec2(d.x / rad.x.max(1.0), d.y / rad.y.max(1.0)).length();
                (n - 1.0).abs() * rad.min_elem().max(1.0) <= tol || r.width().min(r.height()) < tol * 2.0 && r.expand(tol).contains(p)
            }
            Kind::Text(..) => text_rect.is_some_and(|r| r.contains(p)),
        }
    }

    /// Appends the mark's shapes. `scale`: screenshot pixels per logical pixel (sizes).
    pub fn shapes(&self, scale: f32, xf: Xf, layout: &mut Layout, out: &mut Vec<Shape>) {
        let k = scale * xf.zoom;
        let stroke = Stroke::new(STROKE * k, self.color);
        let p = |q: Pos2| xf.apply(q);
        match &self.kind {
            Kind::Pen(pts) => {
                let pts: Vec<Pos2> = pts.iter().map(|q| p(*q)).collect();
                if pts.len() == 1 {
                    out.push(Shape::circle_filled(pts[0], stroke.width / 2.0, self.color));
                } else {
                    out.push(Shape::line(pts, stroke));
                }
            }
            Kind::Box(a, b) => {
                out.push(Shape::rect_stroke(Rect::from_two_pos(p(*a), p(*b)), 0.0, stroke, egui::StrokeKind::Middle));
            }
            Kind::Ellipse(a, b) => {
                let r = Rect::from_two_pos(p(*a), p(*b));
                out.push(Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            Kind::Line(a, b) => out.push(Shape::line_segment([p(*a), p(*b)], stroke)),
            Kind::Arrow(a, b) => {
                let (a, b) = (p(*a), p(*b));
                let d = b - a;
                let len = d.length();
                if len < 0.5 {
                    return;
                }
                let dir = d / len;
                let head = (ARROW_HEAD * k).min(len * 0.6);
                let base = b - dir * head;
                let n = vec2(-dir.y, dir.x) * head * 0.5;
                // The shaft stops inside the head so its square end doesn't poke out at the tip.
                out.push(Shape::line_segment([a, b - dir * head * 0.5], stroke));
                out.push(Shape::convex_polygon(vec![b, base + n, base - n], self.color, Stroke::NONE));
            }
            Kind::Text(at, text, width) => {
                let g = layout(text.clone(), FONT * k, self.color, width * xf.zoom);
                let pos = p(*at);
                let bg = g.rect.translate(pos.to_vec2()).expand(PAD * k);
                out.push(Shape::rect_filled(bg, 3.0 * k, text_backing(self.color)));
                out.push(Shape::galley(pos, g, self.color));
            }
        }
    }
}
