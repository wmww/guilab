//! Marks (the things drawn on a screenshot) and how they turn into egui shapes. The same
//! shapes are painted on screen and rasterized into the composited image ([`crate::raster`]),
//! so what you see is what gets sent.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Shape, Stroke, Vec2, epaint::Galley, pos2, vec2};

/// Drawing tools. `Interact` = not drawing (the host uses pointer input itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Interact,
    Pen,
    Box,
    Ellipse,
    Line,
    Arrow,
    Text,
}

impl Tool {
    pub const ALL: [Tool; 7] = [Tool::Interact, Tool::Pen, Tool::Box, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Text];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Interact => "Use the app (no drawing)",
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
pub const COLORS: [(Color32, &str); 5] = [
    (Color32::from_rgb(230, 40, 40), "Red"),
    (Color32::from_rgb(255, 200, 0), "Yellow"),
    (Color32::from_rgb(30, 170, 60), "Green"),
    (Color32::from_rgb(30, 110, 240), "Blue"),
    (Color32::from_rgb(255, 255, 255), "White"),
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
    /// Top-left of the text.
    Text(Pos2, String),
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

/// Lays out text: (text, font size, color) -> galley, in the target's coordinates.
pub type Layout<'a> = dyn FnMut(String, f32, Color32) -> Arc<Galley> + 'a;

/// Backing behind text so it reads on any screenshot: light behind dark colors, dark behind light.
pub fn text_backing(c: Color32) -> Color32 {
    let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if lum > 150.0 { Color32::from_black_alpha(215) } else { Color32::from_white_alpha(235) }
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
            Kind::Text(_, t) => return t.trim().is_empty(),
        };
        (b - a).length() < min
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
            Kind::Text(at, text) => {
                let g = layout(text.clone(), FONT * k, self.color);
                let pos = p(*at);
                let bg = g.rect.translate(pos.to_vec2()).expand(PAD * k);
                out.push(Shape::rect_filled(bg, 3.0 * k, text_backing(self.color)));
                out.push(Shape::galley(pos, g, self.color));
            }
        }
    }
}
