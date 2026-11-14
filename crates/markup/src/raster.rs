//! CPU compositing: marks are turned into egui shapes, tessellated by epaint and the
//! triangles rasterized onto the screenshot. Uses egui's own fonts, so text looks the same
//! as on screen, and needs no GPU or extra dependencies.

use egui::{
    Color32, ColorImage, Pos2, Rect,
    epaint::{
        ClippedShape, Mesh, Primitive, TessellationOptions, Tessellator,
        text::{FontDefinitions, Fonts, TextOptions},
    },
};

use crate::mark::{Mark, Xf};

/// Holds the fonts (expensive to create; keep one around).
pub struct Renderer {
    fonts: Fonts,
}

impl Default for Renderer {
    fn default() -> Self {
        Renderer { fonts: Fonts::new(TextOptions::default(), FontDefinitions::default()) }
    }
}

impl Renderer {
    /// `base` with `marks` drawn on it. `scale`: screenshot pixels per logical pixel.
    pub fn composite(&mut self, base: &ColorImage, marks: &[Mark], scale: f32) -> ColorImage {
        self.fonts.begin_pass(TextOptions::default());
        let mut shapes = Vec::new();
        {
            let mut view = self.fonts.with_pixels_per_point(1.0);
            let mut layout = |t: String, size: f32, c: Color32, wrap: f32| view.layout(t, egui::FontId::proportional(size), c, wrap);
            for m in marks {
                m.shapes(scale, Xf::IDENTITY, &mut layout, &mut shapes);
            }
        }
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(base.size[0] as f32, base.size[1] as f32));
        let clipped = shapes.into_iter().map(|shape| ClippedShape { clip_rect: rect, shape }).collect();
        let atlas = self.fonts.texture_atlas();
        let prims = Tessellator::new(1.0, TessellationOptions::default(), atlas.size(), atlas.prepared_discs()).tessellate_shapes(clipped);
        let tex = self.fonts.image();
        let mut out = base.clone();
        for p in prims {
            if let Primitive::Mesh(m) = p.primitive {
                draw_mesh(&mut out, &m, &tex);
            }
        }
        out
    }
}

/// Premultiplied RGBA as floats 0..1.
fn rgba(c: Color32) -> [f32; 4] {
    let [r, g, b, a] = c.to_array();
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]
}

fn sample(tex: &ColorImage, uv: Pos2) -> [f32; 4] {
    let (w, h) = (tex.size[0], tex.size[1]);
    let x = (uv.x * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (uv.y * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| rgba(tex.pixels[y * w + x]);
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
}

/// Rasterizes a mesh with premultiplied "over" blending, like egui's GL painter. Pixel
/// centers on a shared edge belong to one triangle only (top-left rule), so translucent
/// fills have no seams.
fn draw_mesh(img: &mut ColorImage, mesh: &Mesh, tex: &ColorImage) {
    let (w, h) = (img.size[0] as i32, img.size[1] as i32);
    for tri in mesh.indices.as_chunks::<3>().0 {
        let v = [&mesh.vertices[tri[0] as usize], &mesh.vertices[tri[1] as usize], &mesh.vertices[tri[2] as usize]];
        let (p0, p1, mut p2) = (v[0].pos, v[1].pos, v[2].pos);
        let mut order = [0, 1, 2];
        // Counter-clockwise in y-down coordinates means area < 0; normalize to area > 0.
        let mut area = (p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x);
        if area == 0.0 {
            continue;
        }
        let mut p1 = p1;
        if area < 0.0 {
            std::mem::swap(&mut p1, &mut p2);
            order.swap(1, 2);
            area = -area;
        }
        let pts = [p0, p1, p2];
        let cols = order.map(|i| rgba(v[i].color));
        let uvs = order.map(|i| v[i].uv);
        let min_x = (pts.iter().map(|p| p.x).fold(f32::MAX, f32::min).floor() as i32).max(0);
        let max_x = (pts.iter().map(|p| p.x).fold(f32::MIN, f32::max).ceil() as i32).min(w - 1);
        let min_y = (pts.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor() as i32).max(0);
        let max_y = (pts.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil() as i32).min(h - 1);
        // Edge i is opposite vertex i: from pts[i+1] to pts[i+2].
        let edges: [(Pos2, Pos2); 3] = [(p1, p2), (p2, p0), (p0, p1)];
        let top_left = edges.map(|(a, b)| {
            let d = b - a;
            (d.y == 0.0 && d.x > 0.0) || d.y < 0.0
        });
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let mut wts = [0.0; 3];
                let mut inside = true;
                for (i, (a, b)) in edges.iter().enumerate() {
                    let e = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
                    if e < 0.0 || (e == 0.0 && !top_left[i]) {
                        inside = false;
                        break;
                    }
                    wts[i] = e / area;
                }
                if !inside {
                    continue;
                }
                let uv = Pos2::new(
                    uvs[0].x * wts[0] + uvs[1].x * wts[1] + uvs[2].x * wts[2],
                    uvs[0].y * wts[0] + uvs[1].y * wts[1] + uvs[2].y * wts[2],
                );
                let t = sample(tex, uv);
                let src: [f32; 4] = std::array::from_fn(|i| (cols[0][i] * wts[0] + cols[1][i] * wts[1] + cols[2][i] * wts[2]) * t[i]);
                if src[3] <= 0.0 && src[..3].iter().all(|c| *c <= 0.0) {
                    continue;
                }
                let d = &mut img.pixels[(y * w + x) as usize];
                let dst = rgba(*d);
                let o: [u8; 4] = std::array::from_fn(|i| ((src[i] + dst[i] * (1.0 - src[3])).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                *d = Color32::from_rgba_premultiplied(o[0], o[1], o[2], o[3]);
            }
        }
    }
}

/// Box-filtered copy fitting in `max` (never upscales).
pub fn downscale(img: &ColorImage, max: [usize; 2]) -> ColorImage {
    let [w, h] = img.size;
    let f = (max[0] as f32 / w as f32).min(max[1] as f32 / h as f32).min(1.0);
    let (nw, nh) = (((w as f32 * f).round() as usize).max(1), ((h as f32 * f).round() as usize).max(1));
    let mut px = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1));
            let mut acc = [0u32; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    for (a, c) in acc.iter_mut().zip(img.pixels[yy * w + xx].to_array()) {
                        *a += c as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let a = acc.map(|a| (a / n) as u8);
            px.push(Color32::from_rgba_premultiplied(a[0], a[1], a[2], a[3]));
        }
    }
    ColorImage::new([nw, nh], px)
}
