//! Display list: the one place element geometry turns into drawable primitives. The CPU
//! renderer and the egui editor both paint it, so arrowheads, text backings and line breaks
//! can't drift apart.

use std::sync::Arc;

use crate::id::AssetId;
use crate::model::{ARROW_HEAD, Body, Color, IRect, Kind, Pt, Rect, TEXT_RADIUS};
use crate::text::{Fonts, TextLayout};

#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Raster { asset: AssetId, dst: Rect, src: Option<IRect> },
    Svg { asset: AssetId, dst: Rect },
    /// Open strokes get round caps and joins; closed ones (boxes) mitred corners. A single
    /// point is a dot.
    Stroke { pts: Vec<Pt>, closed: bool, width: f32, color: Color },
    /// Arrowheads.
    Fill { poly: Vec<Pt>, color: Color },
    Ellipse { rect: Rect, width: f32, color: Color },
    /// Text backing.
    RoundRect { rect: Rect, radius: f32, fill: Color },
    /// `at`: top-left of the first line.
    Text { layout: Arc<TextLayout>, at: Pt, color: Color },
}

/// Backing behind text so it reads on any background: translucent black, or translucent white
/// behind very dark text.
pub fn text_backing(c: Color) -> Color {
    if c.luma() < 50.0 { Color::rgba(255, 255, 255, 200) } else { Color::rgba(0, 0, 0, 170) }
}

/// Where the shaft ends and the head triangle (tip, base corners), or `None` for a
/// zero-length arrow.
pub fn arrow_head(tail: Pt, tip: Pt, scale: f32) -> Option<(Pt, [Pt; 3])> {
    let d = tip - tail;
    let len = d.len();
    if len < 0.5 {
        return None;
    }
    let dir = d * (1.0 / len);
    let head = (ARROW_HEAD * scale).min(len * 0.6);
    let base = tip - dir * head;
    let n = Pt { x: -dir.y, y: dir.x } * (head * 0.5);
    // The shaft stops inside the head so its end doesn't poke out at the tip.
    Some((tip - dir * (head * 0.5), [tip, base + n, base - n]))
}

/// Primitives of every element, bottom to top, each with its element index. `marks: false`
/// leaves out marks (pictures only).
pub fn display_list(body: &Body, fonts: &Fonts, marks: bool) -> Vec<(usize, Prim)> {
    let mut out = Vec::new();
    let width = body.stroke();
    for (i, e) in body.elements.iter().enumerate() {
        if !marks && e.is_mark() {
            continue;
        }
        let color = e.color;
        let mut push = |p: Prim| out.push((i, p));
        match &e.kind {
            Kind::Raster { asset, dst, src } => push(Prim::Raster { asset: *asset, dst: *dst, src: *src }),
            Kind::Svg { asset, dst } => push(Prim::Svg { asset: *asset, dst: *dst }),
            Kind::Pen(pts) => push(Prim::Stroke { pts: pts.clone(), closed: false, width, color }),
            Kind::Box(r) => {
                let (a, b) = (r.min, r.max());
                let pts = vec![a, Pt { x: b.x, y: a.y }, b, Pt { x: a.x, y: b.y }];
                push(Prim::Stroke { pts, closed: true, width, color })
            }
            Kind::Ellipse(r) => push(Prim::Ellipse { rect: *r, width, color }),
            Kind::Line(a, b) => push(Prim::Stroke { pts: vec![*a, *b], closed: false, width, color }),
            Kind::Arrow(a, b) => {
                if let Some((shaft_end, head)) = arrow_head(*a, *b, body.scale) {
                    push(Prim::Stroke { pts: vec![*a, shaft_end], closed: false, width, color });
                    push(Prim::Fill { poly: head.to_vec(), color });
                }
            }
            Kind::Text { at, .. } => {
                let (layout, backing) = e.text_layout(body.scale, fonts).unwrap();
                push(Prim::RoundRect { rect: backing, radius: TEXT_RADIUS * body.scale, fill: text_backing(color) });
                push(Prim::Text { layout: Arc::new(layout), at: *at, color });
            }
        }
    }
    out
}
