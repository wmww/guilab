//! Overlay showing input that reaches a session's apps, from agents and humans alike
//! (colors are by input type: pointer blue, touch green, keys purple): a soft
//! glow at the pointer, ripples for clicks, a fading trail (agent motion, and any drag),
//! rings for touch points, and a stream of pressed keys and other changes at the bottom
//! right. Subtle on purpose; the chat has the exact record.

use std::collections::{BTreeMap, VecDeque};

use egui::{Color32, FontId, Pos2, Rect, Stroke, Ui, pos2, vec2};
use guilab_proto::viz::{TouchPhase, VizKind, VizLog};

/// Colors over dark content, and deeper ones over light content (picked from the color
/// under the pointer).
/// Colors by input type (the same for agents and humans).
struct Palette {
    pointer: Color32,
    touch: Color32,
    /// Glow layer opacity.
    glow: f64,
}
const DARK_BG: Palette = Palette { pointer: Color32::from_rgb(110, 180, 255), touch: Color32::from_rgb(120, 230, 150), glow: 0.14 };
const LIGHT_BG: Palette = Palette { pointer: Color32::from_rgb(20, 95, 215), touch: Color32::from_rgb(20, 145, 70), glow: 0.2 };
/// Seconds to cross-fade between the two palettes.
const PALETTE_FADE: f64 = 0.3;

impl Palette {
    fn mix(a: &Palette, b: &Palette, t: f32) -> Palette {
        let c = |x: Color32, y: Color32| Color32::from_rgb(lerp(x.r(), y.r(), t), lerp(x.g(), y.g(), t), lerp(x.b(), y.b(), t));
        Palette { pointer: c(a.pointer, b.pointer), touch: c(a.touch, b.touch), glow: a.glow + (b.glow - a.glow) * t as f64 }
    }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}
/// Seconds things stay visible.
const TRAIL: f64 = 0.5;
const RIPPLE: f64 = 0.45;
const KEYS: f64 = 3.0;
/// The keys' fade-out, at the end of `KEYS`.
const KEY_FADE: f64 = 0.4;
/// Glow fade after the pointer leaves; scroll marks' life.
const LEAVE: f64 = 0.3;
const SCROLL: f64 = 0.5;

#[derive(Default)]
pub struct Overlay {
    next: u64,
    /// Pointer: position (session px), last event time, by an agent, button held.
    pointer: Option<(Pos2, f64, bool, bool)>,
    /// When the pointer left (the glow fades out from then).
    left: Option<f64>,
    /// Wheel notches: where, when, direction (unit, screen-ish), by an agent.
    notches: Vec<(Pos2, f64, egui::Vec2, bool)>,
    /// Smooth scrolling: when, delta (session px), by an agent.
    smooth: VecDeque<(f64, egui::Vec2, bool)>,
    /// Hover trail: position, time, by an agent.
    trail: VecDeque<(Pos2, f64, bool)>,
    /// The drag in progress (path, by an agent), and finished drags fading from release.
    drag: Option<(Vec<Pos2>, bool)>,
    dropped: Vec<(Vec<Pos2>, f64, bool)>,
    /// Press ripples: where, when, a touch (else a button).
    ripples: Vec<(Pos2, f64, bool)>,
    /// Touch points: position, last event time, lifted.
    touches: BTreeMap<u32, (Pos2, f64, bool)>,
    keys: VecDeque<(String, f64, bool)>,
    /// 0 = dark-content palette, 1 = light; eases toward the content under the pointer.
    light: f32,
    last_paint: f64,
}

fn tint(c: Color32, a: f64) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

impl Overlay {
    /// Takes in new events; `now` is egui time.
    pub fn update(&mut self, log: &VizLog, now: f64) {
        let from = self.next;
        self.next = log.end();
        for ev in log.events.iter().filter(|e| e.seq >= from) {
            let age = ev.at.elapsed().as_secs_f64();
            if age > KEYS {
                continue;
            }
            let t = now - age;
            let (agent, held) = (ev.agent, self.pointer.is_some_and(|p| p.3));
            match &ev.kind {
                VizKind::Motion { x, y } => {
                    let p = pos2(*x as f32, *y as f32);
                    self.pointer = Some((p, t, agent, held));
                    self.left = None;
                    match &mut self.drag {
                        Some((path, _)) => path.push(p),
                        None => self.trail.push_back((p, t, agent)),
                    }
                }
                VizKind::Button { pressed } => {
                    if let Some(ptr) = &mut self.pointer {
                        ptr.1 = t;
                        ptr.2 = agent;
                        ptr.3 = *pressed;
                        if *pressed {
                            self.ripples.push((ptr.0, t, false));
                            // The drag's path starts at the press.
                            self.drag = Some((vec![ptr.0], agent));
                        } else if let Some((path, a)) = self.drag.take()
                            && path.len() > 1
                        {
                            self.dropped.push((path, t, a));
                        }
                    }
                }
                VizKind::Leave => self.left = Some(t),
                VizKind::Scroll { dx, dy, discrete } => {
                    let Some(ptr) = &mut self.pointer else { continue };
                    ptr.1 = t;
                    ptr.2 = agent;
                    let d = vec2(*dx as f32, *dy as f32);
                    if *discrete {
                        // One mark per notch (a few at most), per axis.
                        for (n, unit) in [(d.x, vec2(1.0, 0.0)), (d.y, vec2(0.0, 1.0))] {
                            let count = n.abs().round().clamp(if n != 0.0 { 1.0 } else { 0.0 }, 3.0) as usize;
                            for i in 0..count {
                                self.notches.push((ptr.0, t + i as f64 * 0.06, unit * n.signum(), agent));
                            }
                        }
                    } else {
                        self.smooth.push_back((t, d, agent));
                    }
                }
                VizKind::Touch { id, x, y, phase } => {
                    let p = pos2(*x as f32, *y as f32);
                    let up = *phase == TouchPhase::Up;
                    let p = if up { self.touches.get(id).map_or(p, |t| t.0) } else { p };
                    self.touches.insert(*id, (p, t, up));
                    if *phase == TouchPhase::Down {
                        self.ripples.push((p, t, true));
                    }
                }
                VizKind::Key(k) | VizKind::Note(k) => self.keys.push_back((k.clone(), t, agent)),
            }
        }
        while self.trail.front().is_some_and(|e| now - e.1 > TRAIL) {
            self.trail.pop_front();
        }
        self.ripples.retain(|r| now - r.1 < RIPPLE);
        self.dropped.retain(|d| now - d.1 < TRAIL);
        self.notches.retain(|n| now - n.1 < SCROLL);
        while self.smooth.front().is_some_and(|e| now - e.0 > SCROLL) {
            self.smooth.pop_front();
        }
        if self.left.is_some_and(|t| now - t > LEAVE) {
            self.pointer = None;
            self.left = None;
        }
        self.touches.retain(|_, t| !t.2 || now - t.1 < RIPPLE);
        while self.keys.len() > 12 || self.keys.front().is_some_and(|k| now - k.1 > KEYS) {
            self.keys.pop_front();
        }
    }

    /// Paints over the session `view`, whose image (of `size` pixels) is at `img`.
    /// `light`: the content under the pointer is light (the palette fades toward it).
    pub fn paint(&mut self, ui: &Ui, view: Rect, img: Rect, size: (f32, f32), now: f64, light: bool) {
        let dt = (now - self.last_paint).clamp(0.0, 0.1);
        self.last_paint = now;
        let target = if light { 1.0 } else { 0.0 };
        let step = (dt / PALETTE_FADE) as f32;
        let fading = self.light != target;
        self.light = if self.light < target { (self.light + step).min(target) } else { (self.light - step).max(target) };
        let pal = &Palette::mix(&DARK_BG, &LIGHT_BG, self.light);
        let mut animating = fading;
        let to_screen = |p: Pos2| img.min + vec2(p.x / size.0 * img.width(), p.y / size.1 * img.height());
        let painter = ui.painter_at(view);

        // Hover trail: thin and dim, fading with age; drags: brighter and thicker, held
        // until release, then fading. Each run is one strip, so joints don't double up.
        let mut run: Vec<(Pos2, f32)> = Vec::new();
        let mut run_agent = false;
        let flush = |run: &mut Vec<(Pos2, f32)>, _agent: bool| {
            if run.len() > 1 {
                strip(&painter, run, 2.0, pal.pointer, 0.35);
            }
            run.clear();
        };
        let mut prev_t = f64::NEG_INFINITY;
        for &(p, t, agent) in &self.trail {
            if t - prev_t > 0.15 || agent != run_agent {
                flush(&mut run, run_agent);
                run_agent = agent;
            }
            run.push((to_screen(p), (1.0 - (now - t) / TRAIL) as f32));
            prev_t = t;
            animating = true;
        }
        flush(&mut run, run_agent);
        let drags = self.drag.iter().map(|(path, a)| (path, 1.0, *a)).chain(self.dropped.iter().map(|(path, t, a)| (path, (1.0 - (now - t) / TRAIL) as f32, *a)));
        for (path, f, _) in drags {
            let pts: Vec<(Pos2, f32)> = path.iter().map(|p| (to_screen(*p), f)).collect();
            strip(&painter, &pts, 2.75, pal.pointer, 0.85);
            animating |= f < 1.0;
        }
        // Pointer glow: soft concentric discs, until the pointer leaves; a ring while a
        // button is held. Smooth scrolling stretches it into a droplet whose tail trails
        // opposite the scroll direction, longer the faster it scrolls.
        if let Some((p, _, _, down)) = self.pointer {
            let f = self.left.map_or(1.0, |t| 1.0 - (now - t) / LEAVE);
            let mut tail = egui::Vec2::ZERO;
            if let Some(&(last, _, _)) = self.smooth.back() {
                let window = 0.15;
                let sum = self.smooth.iter().filter(|e| last - e.0 < window).fold(vec2(0.0, 0.0), |s, e| s + e.1);
                let k = (1.0 - (now - last) / SCROLL).max(0.0) as f32;
                // Speed (session px/s) -> tail length, growing slower than linear.
                let speed = sum.length() / window as f32;
                if speed > 0.0 {
                    tail = -sum / sum.length() * 0.6 * speed.powf(0.7) * (img.width() / size.0) * k;
                }
                animating = true;
            }
            if f > 0.0 {
                let c = pal.pointer;
                let at = to_screen(p);
                for r in [14.0, 10.5, 7.0, 4.5] {
                    droplet(&painter, at, r, tail * (r / 14.0), tint(c, f * pal.glow));
                }
                if down {
                    painter.circle_stroke(at, 8.0, Stroke::new(2.0, tint(c, 0.85)));
                }
                animating |= self.left.is_some();
            }
            // Wheel notches: chevrons stepping out from the pointer in the scroll direction.
            for (at, t, dir, _) in &self.notches {
                let age = now - t;
                if age < 0.0 {
                    animating = true;
                    continue;
                }
                let f = (age / SCROLL) as f32;
                let c = tint(pal.pointer, (1.0 - f as f64) * 0.9);
                let tip = to_screen(*at) + *dir * (16.0 + 14.0 * f);
                let n = vec2(-dir.y, dir.x) * 5.0;
                painter.line_segment([tip - *dir * 5.0 + n, tip], Stroke::new(2.0, c));
                painter.line_segment([tip - *dir * 5.0 - n, tip], Stroke::new(2.0, c));
                animating = true;
            }
        }
        for (p, t, touch) in &self.ripples {
            let f = ((now - t) / RIPPLE) as f32;
            let c = if *touch { pal.touch } else { pal.pointer };
            // A quick bright flash, then the ring spreads.
            if f < 0.35 {
                painter.circle_filled(to_screen(*p), 9.0, tint(c, (1.0 - f as f64 / 0.35) * 0.55));
            }
            painter.circle_stroke(to_screen(*p), 6.0 + 18.0 * f, Stroke::new(2.0, tint(c, (1.0 - f as f64) * 0.8)));
            animating = true;
        }
        for (p, t, up) in self.touches.values() {
            let f = if *up { 1.0 - (now - t) / RIPPLE } else { 1.0 };
            let at = to_screen(*p);
            painter.circle_filled(at, 18.0, tint(pal.touch, f * 0.15));
            painter.circle_stroke(at, 18.0, Stroke::new(2.0, tint(pal.touch, f * 0.8)));
            animating |= *up;
        }
        // Key stream, newest at the bottom: big bold labels in equal-width boxes (wider
        // for symbol keys), growing for chords.
        let font = 34.0;
        let key_color = Color32::from_rgb(190, 140, 255);
        let mut y = view.max.y - 14.0;
        for (label, t, _) in self.keys.iter().rev() {
            let age = now - t;
            let f = if age > KEYS - KEY_FADE { (KEYS - age) / KEY_FADE } else { 1.0 };
            let color = tint(key_color, f);
            // "Ctrl+Enter" -> text "Ctrl+" and an Enter symbol.
            let (prefix, key) = match label.rfind('+') {
                Some(i) if i + 1 < label.len() => label.split_at(i + 1),
                _ => ("", label.as_str()),
            };
            let icon = matches!(key, "Enter" | "Space" | "Backspace" | "Tab" | "Left" | "Right" | "Up" | "Down");
            let text = if icon { prefix.to_string() } else { label.clone() };
            let g = painter.layout_no_wrap(text, FontId::proportional(font), color);
            let h = font * 1.2;
            let icon_w = if icon { font * 1.3 } else { 0.0 };
            let content = g.size().x + icon_w;
            let min_w = if icon { font * 1.9 } else { font * 1.5 };
            let size = vec2((content + font * 0.6).max(min_w), h + font * 0.3);
            let r = Rect::from_min_size(pos2(view.max.x - 14.0 - size.x, y - size.y), size);
            painter.rect_filled(r, font * 0.25, Color32::from_black_alpha((f * 180.0) as u8));
            let x0 = r.center().x - content / 2.0;
            let ty = r.center().y - g.size().y / 2.0;
            // Faux bold: the default font has no bold face.
            for dx in [0.0, 0.8, 1.6] {
                painter.galley(pos2(x0 + dx, ty), g.clone(), color);
            }
            if icon {
                let b = Rect::from_min_size(pos2(x0 + g.size().x, r.center().y - font * 0.4), vec2(icon_w, font * 0.8)).shrink(font * 0.1);
                key_icon(&painter, b, key, color, font / 18.0);
            }
            y = r.min.y - 6.0;
            animating = true;
        }
        if animating {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        }
    }
}

/// Symbols the UI font lacks, drawn in `r`; `k` scales strokes (1 = 18 px text).
fn key_icon(p: &egui::Painter, r: Rect, key: &str, c: Color32, k: f32) {
    let s = Stroke::new(2.2 * k, c);
    let head = |tip: Pos2, dir: egui::Vec2| {
        let n = vec2(-dir.y, dir.x);
        p.line_segment([tip, tip - dir * 4.5 * k + n * 4.0 * k], s);
        p.line_segment([tip, tip - dir * 4.5 * k - n * 4.0 * k], s);
    };
    let (l, rr, t, b, cx, cy) = (r.left(), r.right(), r.top(), r.bottom(), r.center().x, r.center().y);
    let half = r.width().min(r.height() * 1.4) / 2.0;
    match key {
        "Enter" => {
            // ↵: down the right side, then left, with a head.
            let y = cy + 2.0 * k;
            p.line_segment([pos2(rr, t + k), pos2(rr, y)], s);
            p.line_segment([pos2(rr, y), pos2(l, y)], s);
            head(pos2(l, y), vec2(-1.0, 0.0));
        }
        "Space" => {
            // ⎵: a wide bracket.
            let y = b - 2.0 * k;
            p.line_segment([pos2(l, y - 4.0 * k), pos2(l, y)], s);
            p.line_segment([pos2(l, y), pos2(rr, y)], s);
            p.line_segment([pos2(rr, y), pos2(rr, y - 4.0 * k)], s);
        }
        "Backspace" => {
            p.line_segment([pos2(rr, cy), pos2(l, cy)], s);
            head(pos2(l, cy), vec2(-1.0, 0.0));
            p.line_segment([pos2(rr, t + 3.0 * k), pos2(rr, b - 3.0 * k)], s);
        }
        "Tab" => {
            // ⇥: an arrow to a bar.
            p.line_segment([pos2(l, cy), pos2(rr - 2.0 * k, cy)], s);
            head(pos2(rr - 2.0 * k, cy), vec2(1.0, 0.0));
            p.line_segment([pos2(rr, t + 3.0 * k), pos2(rr, b - 3.0 * k)], s);
        }
        "Left" | "Right" => {
            let d = if key == "Left" { -1.0 } else { 1.0 };
            p.line_segment([pos2(cx - d * half, cy), pos2(cx + d * half, cy)], s);
            head(pos2(cx + d * half, cy), vec2(d, 0.0));
        }
        _ => {
            let d = if key == "Up" { -1.0 } else { 1.0 };
            let half = r.height() / 2.0;
            p.line_segment([pos2(cx, cy - d * half), pos2(cx, cy + d * half)], s);
            head(pos2(cx, cy + d * half), vec2(0.0, d));
        }
    }
}

/// A disc of radius `r` at `c`, stretched into a teardrop whose tip is `tail` beyond the
/// circle (a plain disc when `tail` is short).
fn droplet(p: &egui::Painter, c: Pos2, r: f32, tail: egui::Vec2, color: Color32) {
    let len = tail.length();
    if len < 1.0 {
        p.circle_filled(c, r, color);
        return;
    }
    let u = tail / len;
    let d = r + len;
    // Tangents from the tip touch the circle at ±θ from `u`; the arc goes round the back.
    let theta = (r / d).acos();
    let mut pts = vec![c + u * d];
    let steps = 24;
    for i in 0..=steps {
        let a = theta + (std::f32::consts::TAU - 2.0 * theta) * i as f32 / steps as f32;
        let (sin, cos) = a.sin_cos();
        pts.push(c + vec2(u.x * cos - u.y * sin, u.x * sin + u.y * cos) * r);
    }
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// A polyline as one triangle strip (no overlapping caps at joints). Each point carries
/// its own opacity factor.
fn strip(p: &egui::Painter, pts: &[(Pos2, f32)], width: f32, color: Color32, alpha: f32) {
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    let add = |w: f32, col: &dyn Fn(f32) -> Color32| {
        let mut mesh = Mesh::default();
        for (i, &(q, f)) in pts.iter().enumerate() {
            let prev = pts[i.saturating_sub(1)].0;
            let next = pts[(i + 1).min(pts.len() - 1)].0;
            let d = next - prev;
            let n = if d.length() > 0.0 { vec2(-d.y, d.x) / d.length() } else { vec2(0.0, 1.0) };
            let (inner, outer) = (col(f), Color32::TRANSPARENT);
            // Core plus a 1 px feather on each side.
            for (off, c) in [(-(w / 2.0 + 1.0), outer), (-w / 2.0, inner), (w / 2.0, inner), (w / 2.0 + 1.0, outer)] {
                mesh.vertices.push(Vertex { pos: q + n * off, uv: WHITE_UV, color: c });
            }
            if i > 0 {
                let b = (i as u32 - 1) * 4;
                let a = i as u32 * 4;
                for j in 0..3 {
                    mesh.add_triangle(b + j, b + j + 1, a + j);
                    mesh.add_triangle(b + j + 1, a + j + 1, a + j);
                }
            }
        }
        p.add(egui::Shape::mesh(mesh));
    };
    add(width, &|f| tint(color, (f.max(0.0) * alpha) as f64));
}
