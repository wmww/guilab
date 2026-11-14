//! CPU renderer (tiny-skia): paints the display list into pixels.

use std::fmt;

use skrifa::outline::OutlinePen;
use tiny_skia::{FillRule, FilterQuality, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, PixmapRef, Transform};

use crate::asset::{Asset, SvgDoc};
use crate::draw::{Prim, display_list};
use crate::id::{AssetId, ImageId};
use crate::model::{Body, Color, IRect, MAX_PIXELS, MAX_SIDE, Rect};
use crate::store::Assets;
use crate::text::Fonts;

/// PNG `tEXt` keyword on every PNG guilab renders. `load_file` refuses files that have it.
pub const RENDER_MARK: &str = "guilab-render";

/// Which part of the canvas to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Region {
    /// The image's crop (the whole canvas without one).
    #[default]
    ImageCrop,
    Canvas,
    /// Any canvas rect; it's clipped to the canvas.
    Rect(IRect),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOpts {
    pub region: Region,
    /// Shrink to fit (never enlarges). Renders over `MAX_PIXELS` are always shrunk.
    pub max: Option<[u32; 2]>,
    /// false: pictures only.
    pub marks: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts { region: Region::ImageCrop, max: None, marks: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    MissingAsset(AssetId),
    /// The region doesn't overlap the canvas.
    Outside(IRect),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            RenderError::MissingAsset(a) => write!(f, "{a} has expired"),
            RenderError::Outside(r) => write!(f, "{r} is outside the canvas"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Rendered pixels. There's deliberately no way to turn this into a `Raster`; its PNGs carry
/// the render mark.
#[derive(Clone)]
pub struct Rendered {
    pixmap: Pixmap,
    /// The canvas area shown.
    pub shows: IRect,
    /// Output pixels per canvas pixel (1 unless shrunk).
    pub zoom: f32,
}

impl fmt::Debug for Rendered {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Rendered({}x{}, shows {}, zoom {})", self.pixmap.width(), self.pixmap.height(), self.shows, self.zoom)
    }
}

impl Rendered {
    pub fn size(&self) -> [u32; 2] {
        [self.pixmap.width(), self.pixmap.height()]
    }
    /// Premultiplied RGBA8 (what GPU textures want).
    pub fn premultiplied(&self) -> &[u8] {
        self.pixmap.data()
    }
    /// Straight RGBA8.
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = self.pixmap.data().to_vec();
        for p in out.as_chunks_mut::<4>().0 {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }
    /// Straight RGBA of one pixel.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let c = self.pixmap.pixel(x, y).expect("pixel in range").demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }
    /// PNG bytes with the render mark: `tEXt guilab-render=<id>` (`draft` when there's no id
    /// yet).
    pub fn png(&self, id: Option<ImageId>) -> Vec<u8> {
        let mut out = Vec::new();
        let [w, h] = self.size();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mark = id.map(|i| i.to_string()).unwrap_or_else(|| "draft".into());
        enc.add_text_chunk(RENDER_MARK.into(), mark).expect("latin1 text chunk");
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(&self.rgba()).expect("png data");
        wr.finish().expect("png finish");
        out
    }
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0[0], c.0[1], c.0[2], c.0[3]);
    p.anti_alias = true;
    p
}

/// The region in canvas px, clipped to the canvas.
pub fn region_rect(body: &Body, region: Region) -> Result<IRect, RenderError> {
    let canvas = IRect::of_size(body.canvas);
    match region {
        Region::ImageCrop => Ok(body.shown()),
        Region::Canvas => Ok(canvas),
        Region::Rect(r) => r.intersect(&canvas).ok_or(RenderError::Outside(r)),
    }
}

/// Renders a body. Text uses the bundled fonts only, so output is deterministic.
pub fn render(body: &Body, assets: &dyn Assets, fonts: &Fonts, opts: &RenderOpts) -> Result<Rendered, RenderError> {
    let shows = region_rect(body, opts.region)?;
    let mut zoom = 1.0f32;
    if let Some([mw, mh]) = opts.max {
        zoom = zoom.min(mw.max(1) as f32 / shows.w as f32).min(mh.max(1) as f32 / shows.h as f32);
    }
    let area = shows.w as f64 * shows.h as f64 * (zoom as f64).powi(2);
    if area > MAX_PIXELS as f64 {
        zoom *= (MAX_PIXELS as f64 / area).sqrt() as f32 * 0.999;
    }
    let out_w = ((shows.w as f32 * zoom).round() as u32).clamp(1, MAX_SIDE);
    let out_h = ((shows.h as f32 * zoom).round() as u32).clamp(1, MAX_SIDE);
    let mut pm = Pixmap::new(out_w, out_h).expect("pixmap size");
    if let Some(bg) = body.background {
        let c = bg.0;
        pm.fill(tiny_skia::Color::from_rgba8(c[0], c[1], c[2], c[3]));
    }
    let xf = Transform::from_row(zoom, 0.0, 0.0, zoom, -shows.x as f32 * zoom, -shows.y as f32 * zoom);
    for (_, prim) in display_list(body, fonts, opts.marks) {
        paint_prim(&mut pm, &prim, xf, zoom, assets, fonts)?;
    }
    Ok(Rendered { pixmap: pm, shows, zoom })
}

fn paint_prim(pm: &mut Pixmap, prim: &Prim, xf: Transform, zoom: f32, assets: &dyn Assets, fonts: &Fonts) -> Result<(), RenderError> {
    match prim {
        Prim::Raster { asset, dst, src } => {
            let a = assets.asset(*asset).ok_or(RenderError::MissingAsset(*asset))?;
            let Asset::Raster(r) = &*a else { return Err(RenderError::MissingAsset(*asset)) };
            draw_raster(pm, r.premultiplied(), r.size(), *dst, *src, xf, zoom);
        }
        Prim::Svg { asset, dst } => {
            let a = assets.asset(*asset).ok_or(RenderError::MissingAsset(*asset))?;
            let Asset::Svg(s) = &*a else { return Err(RenderError::MissingAsset(*asset)) };
            draw_svg(pm, s, *dst, xf);
        }
        Prim::Stroke { pts, closed, width, color } => {
            if pts.len() == 1 {
                if let Some(path) = PathBuilder::from_circle(pts[0].x, pts[0].y, width / 2.0) {
                    pm.fill_path(&path, &paint(*color), FillRule::Winding, xf, None);
                }
                return Ok(());
            }
            let mut pb = PathBuilder::new();
            for (i, p) in pts.iter().enumerate() {
                if i == 0 { pb.move_to(p.x, p.y) } else { pb.line_to(p.x, p.y) }
            }
            if *closed {
                pb.close();
            }
            if let Some(path) = pb.finish() {
                let mut s = tiny_skia::Stroke { width: *width, ..Default::default() };
                if *closed {
                    s.line_join = LineJoin::Miter;
                } else {
                    s.line_cap = LineCap::Round;
                    s.line_join = LineJoin::Round;
                }
                pm.stroke_path(&path, &paint(*color), &s, xf, None);
            }
        }
        Prim::Fill { poly, color } => {
            let mut pb = PathBuilder::new();
            for (i, p) in poly.iter().enumerate() {
                if i == 0 { pb.move_to(p.x, p.y) } else { pb.line_to(p.x, p.y) }
            }
            pb.close();
            if let Some(path) = pb.finish() {
                pm.fill_path(&path, &paint(*color), FillRule::Winding, xf, None);
            }
        }
        Prim::Ellipse { rect, width, color } => {
            if let Some(r) = tiny_skia::Rect::from_xywh(rect.min.x, rect.min.y, rect.size.w.max(0.01), rect.size.h.max(0.01))
                && let Some(path) = PathBuilder::from_oval(r)
            {
                let s = tiny_skia::Stroke { width: *width, ..Default::default() };
                pm.stroke_path(&path, &paint(*color), &s, xf, None);
            }
        }
        Prim::RoundRect { rect, radius, fill } => {
            if let Some(path) = round_rect(rect, *radius) {
                pm.fill_path(&path, &paint(*fill), FillRule::Winding, xf, None);
            }
        }
        Prim::Text { layout, at, color } => {
            let mut pen = Pen(PathBuilder::new());
            fonts.outline(layout, (at.x, at.y), &mut pen);
            if let Some(path) = pen.0.finish() {
                pm.fill_path(&path, &paint(*color), FillRule::Winding, xf, None);
            }
        }
    }
    Ok(())
}

fn round_rect(r: &Rect, radius: f32) -> Option<tiny_skia::Path> {
    let (x0, y0, x1, y1) = (r.min.x, r.min.y, r.max().x, r.max().y);
    let k = radius.min(r.size.w / 2.0).min(r.size.h / 2.0).max(0.0);
    // Circle approximation constant for cubic arcs.
    let c = k * (1.0 - 0.552_284_8);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + k, y0);
    pb.line_to(x1 - k, y0);
    pb.cubic_to(x1 - c, y0, x1, y0 + c, x1, y0 + k);
    pb.line_to(x1, y1 - k);
    pb.cubic_to(x1, y1 - c, x1 - c, y1, x1 - k, y1);
    pb.line_to(x0 + k, y1);
    pb.cubic_to(x0 + c, y1, x0, y1 - c, x0, y1 - k);
    pb.line_to(x0, y0 + k);
    pb.cubic_to(x0, y0 + c, x0 + c, y0, x0 + k, y0);
    pb.close();
    pb.finish()
}

struct Pen(PathBuilder);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y)
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y)
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0.quad_to(cx, cy, x, y)
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        self.0.cubic_to(ax, ay, bx, by, x, y)
    }
    fn close(&mut self) {
        self.0.close()
    }
}

/// Box-filters premultiplied RGBA down by integer factors.
fn box_shrink(px: &[u8], size: [u32; 2], fx: u32, fy: u32) -> (Vec<u8>, [u32; 2]) {
    let (w, h) = ((size[0] / fx).max(1), (size[1] / fy).max(1));
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 4];
            let mut n = 0;
            for sy in y * fy..((y + 1) * fy).min(size[1]) {
                for sx in x * fx..((x + 1) * fx).min(size[0]) {
                    let i = ((sy * size[0] + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += px[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = ((acc[c] + n / 2) / n.max(1)) as u8;
            }
        }
    }
    (out, [w, h])
}

fn draw_raster(pm: &mut Pixmap, px: &[u8], size: [u32; 2], dst: Rect, src: Option<IRect>, xf: Transform, zoom: f32) {
    let full = IRect::of_size(size);
    let src = src.and_then(|s| s.intersect(&full)).unwrap_or(full);
    let (mut buf, mut bsize);
    let mut data = px;
    let mut dsize = size;
    if src != full {
        let mut v = Vec::with_capacity((src.w * src.h * 4) as usize);
        for y in src.y as u32..src.y as u32 + src.h {
            let i = ((y * size[0] + src.x as u32) * 4) as usize;
            v.extend_from_slice(&px[i..i + (src.w * 4) as usize]);
        }
        buf = v;
        bsize = [src.w, src.h];
        data = &buf;
        dsize = bsize;
    }
    // Output px per source px; a box prefilter avoids aliasing when shrinking more than 2x.
    let sx = dst.size.w * zoom / dsize[0] as f32;
    let sy = dst.size.h * zoom / dsize[1] as f32;
    let fx = if sx < 0.5 { (1.0 / sx).floor() as u32 } else { 1 };
    let fy = if sy < 0.5 { (1.0 / sy).floor() as u32 } else { 1 };
    if fx > 1 || fy > 1 {
        (buf, bsize) = box_shrink(data, dsize, fx, fy);
        data = &buf;
        dsize = bsize;
    }
    let Some(src_pm) = PixmapRef::from_bytes(data, dsize[0], dsize[1]) else { return };
    let t = xf.pre_translate(dst.min.x, dst.min.y).pre_scale(dst.size.w / dsize[0] as f32, dst.size.h / dsize[1] as f32);
    // Pixel-exact when it's a whole-pixel translation (screenshots at 1x).
    let exact = t.sx == 1.0 && t.sy == 1.0 && t.kx == 0.0 && t.ky == 0.0 && t.tx.fract() == 0.0 && t.ty.fract() == 0.0;
    let quality = if exact { FilterQuality::Nearest } else { FilterQuality::Bicubic };
    let paint = PixmapPaint { quality, ..Default::default() };
    pm.draw_pixmap(0, 0, src_pm, &paint, t, None);
}

fn draw_svg(pm: &mut Pixmap, s: &SvgDoc, dst: Rect, xf: Transform) {
    let size = s.tree().size();
    let t = xf.pre_translate(dst.min.x, dst.min.y).pre_scale(dst.size.w / size.width(), dst.size.h / size.height());
    resvg::render(s.tree(), t, &mut pm.as_mut());
}

/// An SVG rendered at `size` px (editor textures).
pub fn svg_pixmap(s: &SvgDoc, size: [u32; 2]) -> Rendered {
    let (w, h) = (size[0].clamp(1, MAX_SIDE), size[1].clamp(1, MAX_SIDE));
    let mut pm = Pixmap::new(w, h).expect("pixmap size");
    draw_svg(&mut pm, s, Rect::new(0.0, 0.0, w as f32, h as f32), Transform::identity());
    Rendered { pixmap: pm, shows: IRect::new(0, 0, w, h), zoom: 1.0 }
}
