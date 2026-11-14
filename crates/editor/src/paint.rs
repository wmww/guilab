//! Display-list primitives to egui shapes, and the texture cache for pictures.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, ColorImage, Context, FontFamily, FontId, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions, epaint::text::{Galley, LayoutJob}, pos2};
use guilab_image::{Asset, AssetId, Assets, Color, Prim, TextLayout, svg_pixmap};

use crate::Xf;

/// The egui font family holding the image fonts (the same bytes core lays out with).
pub const FAMILY: &str = "guilab-image";

pub fn family() -> FontFamily {
    FontFamily::Name(FAMILY.into())
}

/// Registers [`FAMILY`] with `ctx` (takes effect next pass). Returns whether it's usable now.
pub fn install_fonts(ctx: &Context) -> bool {
    let ready = ctx.fonts(|f| f.families().iter().any(|fam| *fam == family()));
    if !ready {
        for (i, data) in guilab_image::text::FONT_DATA.iter().enumerate() {
            ctx.add_font(egui::epaint::text::FontInsert::new(
                &format!("{FAMILY}-{i}"),
                egui::FontData::from_static(data),
                vec![egui::epaint::text::InsertFontFamily { family: family(), priority: egui::epaint::text::FontPriority::Lowest }],
            ));
        }
    }
    ready
}

pub fn c32(c: Color) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// The font for image text at screen size `px` (Proportional until the family is installed).
pub fn font(px: f32, ready: bool) -> FontId {
    FontId::new(px.max(0.5), if ready { family() } else { FontFamily::Proportional })
}

/// A one-line egui layout of `text`, with core's line height so rows match core's.
pub fn line_job(text: &str, font: FontId, color: Color32, line_h: f32, wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::simple(text.to_string(), font.clone(), color, wrap);
    job.sections[0].format.line_height = Some(line_h);
    job
}

pub struct Ctx<'a> {
    pub xf: Xf,
    pub assets: &'a dyn Assets,
    pub textures: &'a mut Textures,
    pub ctx: &'a Context,
    pub fonts_ready: bool,
    pub ppp: f32,
    /// Keep each raster texture's filter instead of picking one from the zoom (a second
    /// view of a picture mustn't make the canvas's texture flip filters every frame).
    pub keep_filter: bool,
}

/// Appends shapes for `prim`. `halo`: draw a wider outline of strokes in this color instead
/// (the move tool's hover glow), extra screen width `w`.
pub fn prim_shapes(p: &Prim, c: &mut Ctx, out: &mut Vec<Shape>, halo: Option<(Color32, f32)>) {
    let xf = c.xf;
    let z = xf.zoom;
    match p {
        Prim::Raster { asset, dst, src } => {
            if halo.is_some() {
                return;
            }
            let r = xf.rect(*dst);
            let nearest = if c.keep_filter { c.textures.nearest(*asset).unwrap_or(false) } else { z * c.ppp >= 2.0 };
            match c.assets.asset(*asset) {
                Some(a) if a.as_raster().is_some() => {
                    let raster = a.as_raster().unwrap();
                    let tex = c.textures.raster(c.ctx, *asset, raster, nearest);
                    let [w, h] = raster.size();
                    let uv = match src {
                        Some(s) => Rect::from_min_max(
                            pos2(s.x as f32 / w as f32, s.y as f32 / h as f32),
                            pos2((s.x as f32 + s.w as f32) / w as f32, (s.y as f32 + s.h as f32) / h as f32),
                        ),
                        None => Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    };
                    out.push(Shape::image(tex, r, uv, Color32::WHITE));
                }
                _ => missing(r, out),
            }
        }
        Prim::Svg { asset, dst } => {
            if halo.is_some() {
                return;
            }
            let r = xf.rect(*dst);
            match c.assets.asset(*asset) {
                Some(a) if a.as_svg().is_some() => {
                    let px = [r.width() * c.ppp, r.height() * c.ppp];
                    let tex = c.textures.svg(c.ctx, *asset, &a, px);
                    out.push(Shape::image(tex, r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE));
                }
                _ => missing(r, out),
            }
        }
        Prim::Stroke { pts, closed, width, color } => {
            let (col, w) = match halo {
                Some((h, extra)) => (h, width * z + 2.0 * extra),
                None => (c32(*color), width * z),
            };
            let pts: Vec<Pos2> = pts.iter().map(|p| xf.apply(*p)).collect();
            if pts.len() == 1 {
                out.push(Shape::circle_filled(pts[0], w / 2.0, col));
                return;
            }
            if *closed {
                out.push(Shape::closed_line(pts, Stroke::new(w, col)));
                return;
            }
            // egui strokes have butt ends and miter joins; core draws round ones. Round the
            // ends and sharp turns with discs.
            let r = w / 2.0;
            out.push(Shape::circle_filled(pts[0], r, col));
            out.push(Shape::circle_filled(*pts.last().unwrap(), r, col));
            for win in pts.windows(3) {
                let (a, b) = (win[1] - win[0], win[2] - win[1]);
                if a.length() > 0.0 && b.length() > 0.0 && a.normalized().dot(b.normalized()) < 0.5 {
                    out.push(Shape::circle_filled(win[1], r, col));
                }
            }
            out.push(Shape::line(pts, Stroke::new(w, col)));
        }
        Prim::Fill { poly, color } => {
            let pts: Vec<Pos2> = poly.iter().map(|p| xf.apply(*p)).collect();
            match halo {
                Some((h, extra)) => out.push(Shape::convex_polygon(pts, h, Stroke::new(2.0 * extra, h))),
                None => out.push(Shape::convex_polygon(pts, c32(*color), Stroke::NONE)),
            }
        }
        Prim::Ellipse { rect, width, color } => {
            let r = xf.rect(*rect);
            let (col, w) = match halo {
                Some((h, extra)) => (h, width * z + 2.0 * extra),
                None => (c32(*color), width * z),
            };
            out.push(ellipse(r, Stroke::new(w, col)));
        }
        Prim::RoundRect { rect, radius, fill } => {
            if halo.is_none() {
                out.push(Shape::rect_filled(xf.rect(*rect), radius * z, c32(*fill)));
            }
        }
        Prim::Text { layout, at, color } => {
            if halo.is_none() {
                text_shapes(layout, *at, c32(*color), c, out);
            }
        }
    }
}

/// egui's font atlas keeps glyphs per exact size, so text drawn at `size * zoom` would add a
/// glyph set at every zoom step, and a full atlas rebuilds every font (UI text too). So image
/// text is rasterized at a few on-screen sizes and its galley scaled by the rest: whole
/// physical px up to this size (exact at common zooms), 1/16-octave steps above it.
const EXACT_GLYPH_PX: f32 = 64.0;
/// Larger text is a scaled-up galley of this size (keeps big zooms from filling the atlas).
const MAX_GLYPH_PX: f32 = 128.0;

/// The physical px size egui rasterizes text of on-screen size `px` at.
fn glyph_px(px: f32) -> f32 {
    if px <= EXACT_GLYPH_PX {
        px.round().max(1.0)
    } else {
        2f32.powf((px.log2() * 16.0).round() / 16.0).min(MAX_GLYPH_PX)
    }
}

/// Core's lines, each laid out by egui without wrapping at core's positions.
fn text_shapes(l: &TextLayout, at: guilab_image::Pt, color: Color32, c: &mut Ctx, out: &mut Vec<Shape>) {
    let z = c.xf.zoom;
    let screen = l.font_size * z;
    let size = glyph_px(screen * c.ppp) / c.ppp;
    let k = screen / size;
    let font = font(size, c.fonts_ready);
    for line in &l.lines {
        if line.text.is_empty() {
            continue;
        }
        let job = line_job(&line.text, font.clone(), color, l.line_height * z / k, f32::INFINITY);
        let galley = c.ctx.fonts_mut(|f| f.layout_job(job));
        let galley = if (k - 1.0).abs() < 1e-4 { galley } else { scaled(&galley, k) };
        let pos = c.xf.apply(guilab_image::pt(at.x, at.y + line.y));
        out.push(Shape::galley(pos, galley, color));
    }
}

/// `g` drawn `k` times bigger (glyph quads stretched; their atlas UVs stay).
fn scaled(g: &Galley, k: f32) -> Arc<Galley> {
    let mut g = g.clone();
    let sr = |r: Rect| Rect::from_min_max((r.min.to_vec2() * k).to_pos2(), (r.max.to_vec2() * k).to_pos2());
    for pr in &mut g.rows {
        pr.pos = (pr.pos.to_vec2() * k).to_pos2();
        let row = Arc::make_mut(&mut pr.row);
        for v in &mut row.visuals.mesh.vertices {
            v.pos = (v.pos.to_vec2() * k).to_pos2();
        }
        row.visuals.mesh_bounds = sr(row.visuals.mesh_bounds);
        row.size *= k;
    }
    g.rect = sr(g.rect);
    g.mesh_bounds = sr(g.mesh_bounds);
    Arc::new(g)
}

/// A picture whose asset is gone: a gray box with a cross.
fn missing(r: Rect, out: &mut Vec<Shape>) {
    let gray = Color32::from_gray(128);
    out.push(Shape::rect_filled(r, 0.0, Color32::from_gray(128).gamma_multiply(0.3)));
    out.push(Shape::line_segment([r.left_top(), r.right_bottom()], Stroke::new(1.0, gray)));
    out.push(Shape::line_segment([r.right_top(), r.left_bottom()], Stroke::new(1.0, gray)));
}

/// Textures of pictures. Rasters are immutable, so one upload per asset is exact: the filter
/// follows the zoom (one for all instances, so they never fight over it), and a change
/// replaces the texture rather than adding a second full copy. Rasters over the GPU's max
/// texture side are uploaded box-downscaled (the CPU renderer stays exact). SVGs are rendered
/// per asset and on-screen size, rounded up to √2 steps. Entries unused for a while are
/// dropped.
#[derive(Default)]
pub struct Textures {
    frame: u64,
    rasters: HashMap<AssetId, RasterTex>,
    svgs: HashMap<(AssetId, i32), (TextureHandle, u64)>,
}

struct RasterTex {
    tex: TextureHandle,
    nearest: bool,
    used: u64,
}

const KEEP_FRAMES: u64 = 300;

impl Textures {
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        let f = self.frame;
        self.rasters.retain(|_, t| f - t.used < KEEP_FRAMES);
        self.svgs.retain(|_, (_, used)| f - *used < KEEP_FRAMES);
    }

    /// The filter of a raster's texture, if it has one.
    fn nearest(&self, id: AssetId) -> Option<bool> {
        self.rasters.get(&id).map(|t| t.nearest)
    }

    fn raster(&mut self, ctx: &Context, id: AssetId, r: &guilab_image::Raster, nearest: bool) -> egui::TextureId {
        let opts = if nearest {
            TextureOptions::NEAREST
        } else {
            TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR }
        };
        let frame = self.frame;
        match self.rasters.get_mut(&id) {
            Some(t) if t.nearest == nearest => t.used = frame,
            Some(t) => {
                // Same pixels, other filter: egui sets options per texture, so re-upload in place.
                t.tex.set(raster_image(ctx, r), opts);
                (t.nearest, t.used) = (nearest, frame);
            }
            None => {
                let tex = ctx.load_texture(format!("raster-{id}"), raster_image(ctx, r), opts);
                self.rasters.insert(id, RasterTex { tex, nearest, used: frame });
            }
        }
        self.rasters[&id].tex.id()
    }

    fn svg(&mut self, ctx: &Context, id: AssetId, a: &Arc<Asset>, px: [f32; 2]) -> egui::TextureId {
        let s = a.as_svg().unwrap();
        let max = ctx.input(|i| i.max_texture_side).min(4096) as f32;
        let side = px[0].max(px[1]).clamp(8.0, max);
        let step = (side.log2() * 2.0).ceil() as i32;
        let frame = self.frame;
        if let Some(e) = self.svgs.get_mut(&(id, step)) {
            e.1 = frame;
            return e.0.id();
        }
        let target = 2f32.powf(step as f32 / 2.0).min(max);
        let k = target / px[0].max(px[1]).max(1.0);
        let size = [(px[0] * k).round().clamp(1.0, max) as u32, (px[1] * k).round().clamp(1.0, max) as u32];
        let r = svg_pixmap(s, size);
        let img = ColorImage::from_rgba_premultiplied([r.size()[0] as usize, r.size()[1] as usize], r.premultiplied());
        let tex = ctx.load_texture(format!("svg-{id}-{step}"), img, TextureOptions::LINEAR);
        let tid = tex.id();
        self.svgs.insert((id, step), (tex, frame));
        tid
    }
}

/// A raster's pixels as an egui image, box-downscaled by a whole factor if a side exceeds
/// the max texture side.
fn raster_image(ctx: &Context, r: &guilab_image::Raster) -> ColorImage {
    let [w, h] = r.size().map(|v| v as usize);
    let max = ctx.input(|i| i.max_texture_side).max(1);
    let k = w.max(h).div_ceil(max);
    if k <= 1 {
        return ColorImage::from_rgba_premultiplied([w, h], r.premultiplied());
    }
    let px = r.premultiplied();
    let (nw, nh) = (w.div_ceil(k), h.div_ceil(k));
    let mut out = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let (mut acc, mut n) = ([0u32; 4], 0u32);
            for sy in y * k..((y + 1) * k).min(h) {
                for sx in x * k..((x + 1) * k).min(w) {
                    let i = (sy * w + sx) * 4;
                    for (a, c) in acc.iter_mut().zip(&px[i..i + 4]) {
                        *a += *c as u32;
                    }
                    n += 1;
                }
            }
            let c = acc.map(|a| ((a + n / 2) / n) as u8);
            out.push(Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3]));
        }
    }
    ColorImage::new([nw, nh], out)
}

/// A key point: a small disc with a ring; `hot` lights it up.
pub fn key_point(out: &mut Vec<Shape>, at: Pos2, hot: bool, t: &guilab_kit::Theme) {
    let c = &t.colors;
    let r = t.size.key_point_r + if hot { 1.0 } else { 0.0 };
    out.push(Shape::circle_filled(at, r, if hot { c.key_point_hot } else { c.key_point }));
    out.push(Shape::circle_stroke(at, r, Stroke::new(t.size.line, c.key_point_ring)));
}

/// Outline just outside a screen rect (hover on text and pictures).
pub fn outline(out: &mut Vec<Shape>, r: Rect, color: Color32, w: f32) {
    out.push(Shape::rect_stroke(r.expand(w / 2.0 + 1.0), 2.0, Stroke::new(w, color), egui::StrokeKind::Middle));
}

pub fn shade(out: &mut Vec<Shape>, outer: Rect, hole: Rect, color: Color32) {
    let h = hole.intersect(outer);
    for r in [
        Rect::from_min_max(outer.min, pos2(outer.max.x, h.min.y)),
        Rect::from_min_max(pos2(outer.min.x, h.max.y), outer.max),
        Rect::from_min_max(pos2(outer.min.x, h.min.y), pos2(h.min.x, h.max.y)),
        Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(outer.max.x, h.max.y)),
    ] {
        if r.width() > 0.0 && r.height() > 0.0 {
            out.push(Shape::rect_filled(r, 0.0, color));
        }
    }
}

/// An ellipse outline with enough segments to look smooth at its screen size (egui's own
/// is visibly faceted at large sizes).
fn ellipse(r: Rect, stroke: Stroke) -> Shape {
    let (c, rx, ry) = (r.center(), r.width() / 2.0, r.height() / 2.0);
    let n = ((rx + ry) * 0.6).clamp(16.0, 512.0) as usize;
    let pts = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            pos2(c.x + rx * a.cos(), c.y + ry * a.sin())
        })
        .collect();
    Shape::closed_line(pts, stroke)
}
