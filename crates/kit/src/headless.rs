//! A headless egui host for tests and screenshots: runs frames from synthetic input and
//! paints them on the CPU (epaint's tessellation, rasterized here). No window or GPU, so
//! widget crates can check their look by eye and their behavior in unit tests.

use std::collections::HashMap;

use egui::{
    Color32, ColorImage, Context, Event, FullOutput, Pos2, RawInput, Rect, TextureFilter, TextureId, Ui, Vec2,
    epaint::{ImageData, Mesh, Primitive},
};

pub struct Headless {
    pub ctx: Context,
    /// Screen size in points.
    pub size: Vec2,
    pub pixels_per_point: f32,
    /// egui time in seconds; [`Self::run`] advances it.
    pub time: f64,
    pub modifiers: egui::Modifiers,
    textures: HashMap<TextureId, Texture>,
}

struct Texture {
    image: ColorImage,
    nearest: bool,
    /// Halved copies for minification, built lazily (level 1 = half size).
    mips: Vec<ColorImage>,
}

impl Headless {
    pub fn new(size: Vec2, pixels_per_point: f32) -> Headless {
        Headless { ctx: Context::default(), size, pixels_per_point, time: 0.0, modifiers: Default::default(), textures: HashMap::new() }
    }

    /// Runs one frame `dt` seconds after the previous one, applying texture changes.
    pub fn run(&mut self, events: Vec<Event>, dt: f64, ui: impl FnMut(&mut Ui)) -> FullOutput {
        self.time += dt;
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            events,
            time: Some(self.time),
            modifiers: self.modifiers,
            ..Default::default()
        };
        raw.viewports.entry(raw.viewport_id).or_default().native_pixels_per_point = Some(self.pixels_per_point);
        let out = self.ctx.run_ui(raw, ui);
        for (id, delta) in &out.textures_delta.set {
            let ImageData::Color(img) = &delta.image;
            let nearest = delta.options.magnification == TextureFilter::Nearest;
            match delta.pos {
                None => {
                    self.textures.insert(*id, Texture { image: (**img).clone(), nearest, mips: Vec::new() });
                }
                Some([x0, y0]) => {
                    let t = self.textures.get_mut(id).expect("patch of a known texture");
                    let w = t.image.size[0];
                    for y in 0..img.size[1] {
                        for x in 0..img.size[0] {
                            t.image.pixels[(y0 + y) * w + x0 + x] = img.pixels[y * img.size[0] + x];
                        }
                    }
                    t.mips.clear();
                }
            }
        }
        for id in &out.textures_delta.free {
            self.textures.remove(id);
        }
        out
    }

    /// Paints a frame's output over `bg`, in physical pixels.
    pub fn render(&mut self, out: &FullOutput, bg: Color32) -> ColorImage {
        let ppp = out.pixels_per_point;
        let (w, h) = ((self.size.x * ppp).round() as usize, (self.size.y * ppp).round() as usize);
        let mut img = ColorImage::new([w, h], vec![bg; w * h]);
        let prims = self.ctx.tessellate(out.shapes.clone(), ppp);
        for p in prims {
            let Primitive::Mesh(mesh) = p.primitive else { continue };
            let clip = Rect::from_min_max((p.clip_rect.min.to_vec2() * ppp).to_pos2(), (p.clip_rect.max.to_vec2() * ppp).to_pos2());
            if let Some(t) = self.textures.get_mut(&mesh.texture_id) {
                draw_mesh(&mut img, &mesh, t, clip, ppp);
            }
        }
        img
    }
}

/// Premultiplied RGBA as floats 0..1.
fn rgba(c: Color32) -> [f32; 4] {
    let [r, g, b, a] = c.to_array();
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]
}

fn halve(img: &ColorImage) -> ColorImage {
    let [w, h] = img.size;
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut px = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((2 * x + dx).min(w - 1), (2 * y + dy).min(h - 1));
                for (a, c) in acc.iter_mut().zip(img.pixels[sy * w + sx].to_array()) {
                    *a += c as u32;
                }
            }
            let a = acc.map(|a| ((a + 2) / 4) as u8);
            px.push(Color32::from_rgba_premultiplied(a[0], a[1], a[2], a[3]));
        }
    }
    ColorImage::new([nw, nh], px)
}

fn sample(tex: &ColorImage, uv: Pos2, nearest: bool) -> [f32; 4] {
    let (w, h) = (tex.size[0], tex.size[1]);
    let px = |x: usize, y: usize| rgba(tex.pixels[y * w + x]);
    if nearest {
        let x = ((uv.x * w as f32) as usize).min(w - 1);
        let y = ((uv.y * h as f32) as usize).min(h - 1);
        return px(x, y);
    }
    let x = (uv.x * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (uv.y * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
}

/// Rasterizes a mesh (positions in points) with premultiplied "over" blending, like egui's GL
/// painter. Pixel centers on a shared edge belong to one triangle only (top-left rule), so
/// translucent fills have no seams. Minified textures use a mip level.
fn draw_mesh(img: &mut ColorImage, mesh: &Mesh, tex: &mut Texture, clip: Rect, ppp: f32) {
    let (w, h) = (img.size[0] as i32, img.size[1] as i32);
    for tri in mesh.indices.as_chunks::<3>().0 {
        let v = [&mesh.vertices[tri[0] as usize], &mesh.vertices[tri[1] as usize], &mesh.vertices[tri[2] as usize]];
        let p = |i: usize| (v[i].pos.to_vec2() * ppp).to_pos2();
        let (p0, mut p1, mut p2) = (p(0), p(1), p(2));
        let mut order = [0, 1, 2];
        let mut area = (p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x);
        if area == 0.0 {
            continue;
        }
        if area < 0.0 {
            std::mem::swap(&mut p1, &mut p2);
            order.swap(1, 2);
            area = -area;
        }
        let pts = [p0, p1, p2];
        let cols = order.map(|i| rgba(v[i].color));
        let uvs = order.map(|i| v[i].uv);
        // Texels per pixel along the triangle, for the mip level.
        let tw = tex.image.size[0] as f32;
        let uv_area = ((uvs[1].x - uvs[0].x) * (uvs[2].y - uvs[0].y) - (uvs[1].y - uvs[0].y) * (uvs[2].x - uvs[0].x)).abs() * tw * tex.image.size[1] as f32;
        let level = if tex.nearest || uv_area == 0.0 { 0 } else { ((uv_area / area).sqrt().log2().floor().max(0.0) as usize).min(12) };
        while tex.mips.len() < level {
            let next = halve(tex.mips.last().unwrap_or(&tex.image));
            tex.mips.push(next);
        }
        let src_tex = if level == 0 { &tex.image } else { &tex.mips[level - 1] };
        let min_x = (pts.iter().map(|p| p.x).fold(f32::MAX, f32::min).floor().max(clip.min.x.floor()) as i32).max(0);
        let max_x = (pts.iter().map(|p| p.x).fold(f32::MIN, f32::max).ceil().min(clip.max.x.ceil() - 1.0) as i32).min(w - 1);
        let min_y = (pts.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor().max(clip.min.y.floor()) as i32).max(0);
        let max_y = (pts.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil().min(clip.max.y.ceil() - 1.0) as i32).min(h - 1);
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
                let t = sample(src_tex, uv, tex.nearest);
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

/// Pointer and key events, for scripting [`Headless::run`].
pub mod ev {
    use egui::{Event, Key, Modifiers, PointerButton, Pos2};

    pub fn moved(p: Pos2) -> Event {
        Event::PointerMoved(p)
    }
    pub fn button(p: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers }
    }
    pub fn press(p: Pos2) -> Event {
        button(p, true, Modifiers::NONE)
    }
    pub fn release(p: Pos2) -> Event {
        button(p, false, Modifiers::NONE)
    }
    pub fn key(k: Key, modifiers: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers }
    }
    pub fn text(s: &str) -> Event {
        Event::Text(s.into())
    }
}
