//! The data: images, bodies, elements, and the small geometry types they use.

use std::fmt;
use std::sync::Arc;

use crate::id::{AssetId, ImageId};

/// A point in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

pub const fn pt(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

impl std::ops::Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        pt(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        pt(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Mul<f32> for Pt {
    type Output = Pt;
    fn mul(self, k: f32) -> Pt {
        pt(self.x * k, self.y * k)
    }
}

impl Pt {
    pub fn len(self) -> f32 {
        self.x.hypot(self.y)
    }
    pub fn dot(self, o: Pt) -> f32 {
        self.x * o.x + self.y * o.y
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// `min` is the top-left corner. Sizes are never negative.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub min: Pt,
    pub size: Size,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { min: pt(x, y), size: Size { w, h } }
    }
    /// The rect spanned by two opposite corners, in any order.
    pub fn from_corners(a: Pt, b: Pt) -> Rect {
        Rect::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    }
    /// Bounding rect of points (empty rect at 0,0 for none).
    pub fn bounding(pts: &[Pt]) -> Rect {
        let Some(first) = pts.first() else { return Rect::default() };
        let (mut a, mut b) = (*first, *first);
        for p in pts {
            a = pt(a.x.min(p.x), a.y.min(p.y));
            b = pt(b.x.max(p.x), b.y.max(p.y));
        }
        Rect::from_corners(a, b)
    }
    pub fn max(&self) -> Pt {
        pt(self.min.x + self.size.w, self.min.y + self.size.h)
    }
    pub fn center(&self) -> Pt {
        pt(self.min.x + self.size.w / 2.0, self.min.y + self.size.h / 2.0)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let (a, b) = (self.max(), o.max());
        Rect::from_corners(pt(self.min.x.min(o.min.x), self.min.y.min(o.min.y)), pt(a.x.max(b.x), a.y.max(b.y)))
    }
    pub fn expand(&self, d: f32) -> Rect {
        Rect::new(self.min.x - d, self.min.y - d, (self.size.w + 2.0 * d).max(0.0), (self.size.h + 2.0 * d).max(0.0))
    }
    pub fn contains(&self, p: Pt) -> bool {
        let m = self.max();
        p.x >= self.min.x && p.y >= self.min.y && p.x <= m.x && p.y <= m.y
    }
    pub fn translate(&self, d: Pt) -> Rect {
        Rect { min: self.min + d, size: self.size }
    }
    /// The smallest integer rect containing this one.
    pub fn round_out(&self) -> IRect {
        let (a, b) = (self.min, self.max());
        let (x, y) = (a.x.floor() as i32, a.y.floor() as i32);
        IRect { x, y, w: (b.x.ceil() as i32 - x).max(0) as u32, h: (b.y.ceil() as i32 - y).max(0) as u32 }
    }
}

/// An integer rect (crops, source regions, rendered areas).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl IRect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> IRect {
        IRect { x, y, w, h }
    }
    pub fn of_size(size: [u32; 2]) -> IRect {
        IRect { x: 0, y: 0, w: size[0], h: size[1] }
    }
    pub fn to_rect(self) -> Rect {
        Rect::new(self.x as f32, self.y as f32, self.w as f32, self.h as f32)
    }
    pub fn right(&self) -> i64 {
        self.x as i64 + self.w as i64
    }
    pub fn bottom(&self) -> i64 {
        self.y as i64 + self.h as i64
    }
    pub fn intersect(&self, o: &IRect) -> Option<IRect> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        (r > x as i64 && b > y as i64).then(|| IRect { x, y, w: (r - x as i64) as u32, h: (b - y as i64) as u32 })
    }
    /// Whether `o` lies entirely inside this rect.
    pub fn covers(&self, o: &IRect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
}

impl fmt::Display for IRect {
    /// `X,Y WxH`, the text format's rect.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{},{} {}x{}", self.x, self.y, self.w, self.h)
    }
}

/// Straight (not premultiplied) RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const RED: Color = Color([230, 40, 40, 255]);
    pub const YELLOW: Color = Color([255, 200, 0, 255]);
    pub const GREEN: Color = Color([30, 170, 60, 255]);
    pub const BLUE: Color = Color([30, 110, 240, 255]);
    pub const WHITE: Color = Color([255, 255, 255, 255]);
    pub const BLACK: Color = Color([0, 0, 0, 255]);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color([r, g, b, 255])
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color([r, g, b, a])
    }
    /// Palette name, if it's a palette color.
    pub fn name(self) -> Option<&'static str> {
        PALETTE.iter().find(|(_, c)| *c == self).map(|(n, _)| *n)
    }
    /// Perceived brightness, 0..255.
    pub fn luma(self) -> f32 {
        let [r, g, b, _] = self.0;
        0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
    }
}

/// The marks palette, in toolbar order. Also the color names the text format knows.
pub const PALETTE: [(&str, Color); 6] = [
    ("red", Color::RED),
    ("yellow", Color::YELLOW),
    ("green", Color::GREEN),
    ("blue", Color::BLUE),
    ("white", Color::WHITE),
    ("black", Color::BLACK),
];

impl fmt::Display for Color {
    /// The palette name, else `#rrggbb` (alpha isn't part of the text format).
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.name() {
            Some(n) => f.write_str(n),
            None => write!(f, "#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2]),
        }
    }
}

/// Sizes in logical pixels, multiplied by the body's `scale`.
pub const STROKE: f32 = 3.0;
pub const FONT: f32 = 18.0;
/// Padding around text, inside its backing.
pub const TEXT_PAD: f32 = 4.0;
pub const TEXT_RADIUS: f32 = 3.0;
pub const ARROW_HEAD: f32 = 16.0;

/// Hard limits, checked whenever an image enters the store.
pub const MAX_ELEMENTS: usize = 10_000;
pub const MAX_PEN_POINTS: usize = 100_000;
pub const MAX_SIDE: u32 = 16_384;
/// Most pixels in a raster or a rendering (256 MiB of RGBA). Larger renders are shrunk.
pub const MAX_PIXELS: u64 = 64 << 20;

/// An immutable image. Edits make new images with `parent` set.
#[derive(Clone, Debug)]
pub struct Image {
    pub id: ImageId,
    pub parent: Option<ImageId>,
    pub body: Arc<Body>,
    pub made: Made,
}

/// Who made an image, for listings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Made {
    Screenshot { at_ms: u64, by_agent: bool },
    Human,
    Agent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub canvas: [u32; 2],
    /// UI scale of the source screen: stroke = `STROKE * scale`, default font = `FONT * scale`.
    pub scale: f32,
    pub background: Option<Color>,
    /// Non-destructive viewport over the canvas, inside it.
    pub crop: Option<IRect>,
    /// Bottom to top; listing number = index + 1.
    pub elements: Vec<Element>,
}

impl Body {
    pub fn new(canvas: [u32; 2], scale: f32) -> Body {
        Body { canvas, scale, background: None, crop: None, elements: Vec::new() }
    }

    /// A screenshot: one raster filling the canvas.
    pub fn screenshot(asset: AssetId, size: [u32; 2], scale: f32) -> Body {
        let dst = Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32);
        Body { elements: vec![Element::picture(Kind::Raster { asset, dst, src: None })], ..Body::new(size, scale) }
    }

    /// The part of the canvas the image shows.
    pub fn shown(&self) -> IRect {
        self.crop.unwrap_or(IRect::of_size(self.canvas))
    }

    /// Stroke width of marks.
    pub fn stroke(&self) -> f32 {
        STROKE * self.scale
    }

    /// Default text size.
    pub fn font_size(&self) -> f32 {
        FONT * self.scale
    }

    /// Whether element `i` is the locked base: element 0, a raster covering the whole canvas.
    pub fn is_base(&self, i: usize) -> bool {
        i == 0
            && self.elements.first().is_some_and(|e| match &e.kind {
                Kind::Raster { dst, .. } => {
                    dst.min.x <= 0.0 && dst.min.y <= 0.0 && dst.max().x >= self.canvas[0] as f32 && dst.max().y >= self.canvas[1] as f32
                }
                _ => false,
            })
    }

    /// Asset ids the body uses, each once.
    pub fn assets(&self) -> Vec<AssetId> {
        let mut v: Vec<AssetId> = self.elements.iter().filter_map(|e| e.kind.asset()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Checks the hard limits and the crop. The message is a full sentence.
    pub fn check(&self) -> Result<(), String> {
        let [w, h] = self.canvas;
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return Err(format!("canvas {w}x{h} is out of range (1 to {MAX_SIDE} px per side)"));
        }
        if !(self.scale.is_finite() && self.scale > 0.0 && self.scale <= 16.0) {
            return Err(format!("scale {} is out of range", self.scale));
        }
        if let Some(c) = self.crop
            && (c.w == 0 || c.h == 0 || !IRect::of_size(self.canvas).covers(&c))
        {
            return Err(format!("crop {c} isn't inside the {w}x{h} canvas"));
        }
        if self.elements.len() > MAX_ELEMENTS {
            return Err(format!("{} elements is too many (at most {MAX_ELEMENTS})", self.elements.len()));
        }
        let pts: usize = self.elements.iter().map(|e| if let Kind::Pen(p) = &e.kind { p.len() } else { 0 }).sum();
        if pts > MAX_PEN_POINTS {
            return Err(format!("{pts} pen points is too many (at most {MAX_PEN_POINTS})"));
        }
        for (i, e) in self.elements.iter().enumerate() {
            if !e.kind.is_finite() {
                return Err(format!("element {} has a coordinate that isn't a finite number", i + 1));
            }
            if let Kind::Pen(p) = &e.kind
                && p.is_empty()
            {
                return Err(format!("element {} is a pen with no points", i + 1));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// Unused for pictures.
    pub color: Color,
    pub kind: Kind,
}

impl Element {
    pub fn new(color: Color, kind: Kind) -> Element {
        Element { color, kind }
    }
    pub fn picture(kind: Kind) -> Element {
        Element { color: Color::RED, kind }
    }
    pub fn is_mark(&self) -> bool {
        !self.is_picture()
    }
    pub fn is_picture(&self) -> bool {
        matches!(self.kind, Kind::Raster { .. } | Kind::Svg { .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Pen(Vec<Pt>),
    Box(Rect),
    Ellipse(Rect),
    Line(Pt, Pt),
    /// Tail, tip.
    Arrow(Pt, Pt),
    /// `at` is the top-left of the text (inside the backing). `width`: wrap width, `None` =
    /// only explicit newlines break.
    Text { at: Pt, size: f32, width: Option<f32>, text: String },
    /// `src`: the part of the raster used, in its pixels (`None` = all of it).
    Raster { asset: AssetId, dst: Rect, src: Option<IRect> },
    Svg { asset: AssetId, dst: Rect },
}

impl Kind {
    pub fn asset(&self) -> Option<AssetId> {
        match self {
            Kind::Raster { asset, .. } | Kind::Svg { asset, .. } => Some(*asset),
            _ => None,
        }
    }

    /// The keyword in the text format.
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Pen(_) => "pen",
            Kind::Box(_) => "box",
            Kind::Ellipse(_) => "ellipse",
            Kind::Line(..) => "line",
            Kind::Arrow(..) => "arrow",
            Kind::Text { .. } => "text",
            Kind::Raster { .. } => "raster",
            Kind::Svg { .. } => "svg",
        }
    }

    fn is_finite(&self) -> bool {
        let p = |p: &Pt| p.x.is_finite() && p.y.is_finite();
        let r = |r: &Rect| p(&r.min) && r.size.w.is_finite() && r.size.h.is_finite() && r.size.w >= 0.0 && r.size.h >= 0.0;
        match self {
            Kind::Pen(v) => v.iter().all(p),
            Kind::Box(x) | Kind::Ellipse(x) | Kind::Raster { dst: x, .. } | Kind::Svg { dst: x, .. } => r(x),
            Kind::Line(a, b) | Kind::Arrow(a, b) => p(a) && p(b),
            Kind::Text { at, size, width, .. } => {
                p(at) && size.is_finite() && *size > 0.0 && width.is_none_or(|w| w.is_finite() && w > 0.0)
            }
        }
    }
}
