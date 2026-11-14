//! Motion that links the editor and the chat: flights ("whooshes") of an image between the
//! main view and its chat slot, the live app fading out under an incoming one, and the glow that leads
//! from the open image's slot to the image.
//!
//! [`Flights`] is the state; the workspace starts flights when the open image changes and
//! the UI resolves both ends every frame (this frame's slot rects and the view), so a
//! flight follows scrolling, layout changes and pans mid-air. Painting is direct, on a
//! layer painter after everything else (an `Area` would take presses from below).

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Vec2, epaint::Vertex, pos2, vec2};
use guilab_chat::SlotId;
use guilab_image::IRect;
use guilab_kit::anim::{Fader, INSTANT, Motion, lerp_rect};
use guilab_kit::theme::Motions;

use egui::{LayerId, Order};
use guilab_image::Store;
use guilab_kit::Theme;

use crate::workspace::{Att, Workspace};

/// Where an image is drawn: the screen rect its whole canvas maps to, the part of the
/// screen it shows in, and its opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub canvas: Rect,
    pub clip: Rect,
    pub alpha: f32,
}

impl Place {
    pub fn lerp(a: Place, b: Place, t: f32) -> Place {
        Place { canvas: lerp_rect(a.canvas, b.canvas, t), clip: lerp_rect(a.clip, b.clip, t), alpha: a.alpha + (b.alpha - a.alpha) * t }
    }

    /// An image (`canvas` px, showing `shown`) in a slot drawn at `rect`, as the slot shows
    /// it: `shown` covers the slot, centered (extreme aspects are center-cropped), clipped to
    /// the slot's visible `area`. A slot scrolled out of `area` gives a place at its edge
    /// with alpha 0, so an image flying there fades out instead of vanishing off screen.
    pub fn in_slot(canvas: [u32; 2], shown: IRect, rect: Rect, area: Rect) -> Place {
        let (sw, sh) = (shown.w.max(1) as f32, shown.h.max(1) as f32);
        let k = (rect.width() / sw).max(rect.height() / sh);
        let region = Rect::from_center_size(rect.center(), vec2(sw, sh) * k);
        let min = region.min - vec2(shown.x as f32, shown.y as f32) * k;
        let canvas = Rect::from_min_size(min, vec2(canvas[0] as f32, canvas[1] as f32) * k);
        let clip = rect.intersect(area);
        if clip.is_positive() {
            return Place { canvas, clip, alpha: 1.0 };
        }
        let d = keep_inside(rect, area).min - rect.min;
        Place { canvas: canvas.translate(d), clip: rect.translate(d), alpha: 0.0 }
    }
}

/// Shifts `r` into `bounds` (on each axis where it fits).
pub fn keep_inside(r: Rect, bounds: Rect) -> Rect {
    let mut d = Vec2::ZERO;
    if r.width() <= bounds.width() {
        d.x = (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0);
    }
    if r.height() <= bounds.height() {
        d.y = (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0);
    }
    r.translate(d)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// From the slot into the view (opening).
    Out,
    /// From the view back into the slot (closing).
    Home,
}

#[derive(Clone, Debug)]
pub struct Flight {
    pub att: Att,
    pub slot: SlotId,
    pub dir: Dir,
    /// The start when it isn't the slot: the view it left, or where an interrupted flight was.
    from: Option<Place>,
    t0: f64,
    /// The slot end as last resolved (used while the slot isn't laid out).
    slot_seen: Option<Place>,
    /// Where it was drawn last (an interruption starts there).
    last: Option<Place>,
}

/// The glow's source: the open image's slot. When the open image changes without a flight
/// (a past image's first edit moves it to a new composer slot), the source slides over.
#[derive(Clone, Copy, Debug)]
struct GlowSrc {
    key: (SlotId, Att),
    rect: Rect,
    slide: Option<(Rect, f64)>,
    frame: u64,
    /// Frames shown in a row without its slot.
    held: u8,
}

pub struct Flights {
    list: Vec<Flight>,
    whoosh: Motion,
    fade: Motion,
    /// The live app's opacity in the view: it fades out as an image flies in; it's back at
    /// once when the image leaves (it was under the image all along).
    pub live: Fader,
    glow: Fader,
    glow_src: Option<GlowSrc>,
    frame: u64,
    now: f64,
}

impl Default for Flights {
    fn default() -> Self {
        Flights { list: Vec::new(), whoosh: INSTANT, fade: INSTANT, live: Fader::at(1.0), glow: Fader::at(0.0), glow_src: None, frame: 0, now: 0.0 }
    }
}

impl Flights {
    /// Start of a frame: the clock and timings; finished flights land.
    pub fn begin(&mut self, now: f64, m: &Motions) {
        self.now = now;
        self.frame += 1;
        self.whoosh = m.whoosh;
        self.fade = m.fade;
        let secs = self.whoosh.secs as f64;
        self.list.retain(|f| now - f.t0 < secs);
    }

    pub fn now(&self) -> f64 {
        self.now
    }

    pub fn flights(&self) -> &[Flight] {
        &self.list
    }

    /// Something is still moving (keep repainting).
    pub fn moving(&self) -> bool {
        !self.list.is_empty() || self.live.moving(self.now) || self.glow.moving(self.now) || self.glow_src.is_some_and(|g| g.slide.is_some())
    }

    fn take(&mut self, att: Att) -> Option<Flight> {
        let i = self.list.iter().position(|f| f.att == att)?;
        Some(self.list.remove(i))
    }

    /// `att` flies out of `slot` into the view. A flight of it already in the air turns
    /// around from where it is.
    pub fn out(&mut self, att: Att, slot: SlotId) {
        let from = self.take(att).and_then(|f| f.last.or(f.from));
        self.list.push(Flight { att, slot, dir: Dir::Out, from, t0: self.now, slot_seen: None, last: None });
        // Switching images: the view goes from one image to the other, not via the live app.
        if self.list.iter().any(|f| f.dir == Dir::Home && f.t0 == self.now) {
            self.live = Fader::at(0.0);
        }
        self.live.set(0.0, self.now, self.whoosh);
        self.glow = Fader::at(1.0);
    }

    /// `att`, shown at `view` (last frame), flies home to `slot`. Without a place to start
    /// from (it wasn't drawn yet) it's just there.
    pub fn home(&mut self, att: Att, slot: SlotId, view: Option<Place>) {
        let from = self.take(att).and_then(|f| f.last.or(f.from)).or(view);
        if from.is_some() {
            self.list.push(Flight { att, slot, dir: Dir::Home, from, t0: self.now, slot_seen: None, last: None });
        }
        // The live app is right there under it, as the image lifts off.
        self.live = Fader::at(1.0);
        self.glow = Fader::at(0.0);
    }

    /// The view changed without motion (a drawing became a draft, a removed or sent image
    /// left): the live app shows (or not) at once.
    pub fn quiet(&mut self, live: bool) {
        self.live = Fader::at(live as u8 as f32);
    }

    /// Lands every flight into the view now (a press there: the human wants to work on it).
    pub fn land_out(&mut self) {
        self.list.retain(|f| f.dir == Dir::Home);
    }

    /// Lands everything (the window went away).
    pub fn finish(&mut self) {
        self.list.clear();
        self.live = Fader::at(self.live.to);
        self.glow_src = None;
    }

    /// `att` is flying into the view (the view shouldn't paint it yet).
    pub fn flying_in(&self, att: Att) -> bool {
        self.list.iter().any(|f| f.att == att && f.dir == Dir::Out)
    }

    /// A flight starts or ends at this slot (it shows empty meanwhile).
    pub fn touching(&self, slot: SlotId, att: Att) -> bool {
        self.list.iter().any(|f| f.att == att && f.slot == slot)
    }

    /// Eased progress of a flight.
    pub fn progress(&self, f: &Flight) -> f32 {
        let m = self.whoosh;
        if m.secs <= 0.0 { 1.0 } else { m.easing.apply(((self.now - f.t0) / m.secs as f64) as f32) }
    }

    /// Where each flight is now, given its slot end (`slot`: None while the slot isn't laid
    /// out; the last one seen is used) and the view end (`view`: the open image's place).
    /// A flight with an end that was never known lands at once.
    pub fn places(&mut self, slot: impl Fn(&Flight) -> Option<Place>, view: impl Fn(Att) -> Option<Place>) -> Vec<(Flight, Place, f32)> {
        let mut out = Vec::new();
        let mut list = std::mem::take(&mut self.list);
        list.retain_mut(|f| {
            if let Some(p) = slot(f) {
                f.slot_seen = Some(p);
            }
            let (a, b) = match f.dir {
                Dir::Out => (f.from.or(f.slot_seen), view(f.att)),
                Dir::Home => (f.from, f.slot_seen),
            };
            let (Some(a), Some(b)) = (a, b) else { return false };
            let t = self.progress(f);
            let p = Place::lerp(a, b, t);
            f.last = Some(p);
            out.push((f.clone(), p, t));
            true
        });
        self.list = list;
        out
    }

    /// The open image's glow source and opacity this frame, given its slot (key and visible
    /// rect). Call only while the open image isn't flying (its flight draws its own beam).
    /// `slot` None: the open image has no slot on screen. A source shown last frame holds
    /// for a few frames (a new composer slot is laid out a frame or two after the change
    /// that made it), then the glow goes.
    pub fn glow_source(&mut self, slot: Option<((SlotId, Att), Rect)>) -> Option<(Rect, f32)> {
        let Some((key, rect)) = slot else {
            if let Some(g) = &mut self.glow_src
                && g.frame + 1 >= self.frame
                && g.held < 3
            {
                g.held += 1;
                g.frame = self.frame;
                return Some((g.rect, self.glow.value(self.now)));
            }
            self.glow = Fader::at(0.0);
            self.glow_src = None;
            return None;
        };
        let now = self.now;
        self.glow.set(1.0, now, self.fade);
        let mut slide = match self.glow_src {
            // Continuous with last frame: the same slot keeps sliding, another one slides from there.
            Some(g) if g.frame + 1 >= self.frame => if g.key == key { g.slide } else { Some((g.rect, now)) },
            _ => None,
        };
        let mut src = rect;
        if let Some((from, t0)) = slide {
            let m = self.whoosh;
            let t = if m.secs <= 0.0 { 1.0 } else { ((now - t0) / m.secs as f64) as f32 };
            if t >= 1.0 {
                slide = None;
            } else {
                src = lerp_rect(from, rect, m.easing.apply(t));
            }
        }
        self.glow_src = Some(GlowSrc { key, rect: src, slide, frame: self.frame, held: 0 });
        Some((src, self.glow.value(now)))
    }
}

// ---- Painting ----

/// The image's drop shadow in flight: strongest mid-air.
pub fn shadow(p: &Painter, clip: Rect, t: f32, color: Color32, radius: f32) {
    let k = (t * std::f32::consts::PI).sin().max(0.0);
    if k > 0.01 {
        let s = egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: color.gamma_multiply(k) };
        p.add(s.as_shape(clip, radius));
    }
}

fn vtx(pos: Pos2, color: Color32) -> Vertex {
    Vertex { pos, uv: egui::epaint::WHITE_UV, color }
}

/// A feathered glow around `r`: `color` at its edge, fading out over `w`.
pub fn halo(p: &Painter, r: Rect, w: f32, color: Color32) {
    let mut m = Mesh::default();
    let inner = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    let o = r.expand(w);
    let outer = [o.left_top(), o.right_top(), o.right_bottom(), o.left_bottom()];
    for i in 0..4 {
        m.vertices.push(vtx(inner[i], color));
        m.vertices.push(vtx(outer[i], Color32::TRANSPARENT));
    }
    for i in 0..4u32 {
        let (a, b) = (i * 2, ((i + 1) % 4) * 2);
        m.indices.extend([a, a + 1, b + 1, a, b + 1, b]);
    }
    p.add(Shape::mesh(m));
}

/// Light from `src` (a slot) toward `dst` (the image): a soft fan from the slot's side that
/// faces the image to the image's facing side, brightest at the slot, with a few faint
/// rays inside. Nothing when the rects overlap; it fades as they meet.
pub fn beam(p: &Painter, src: Rect, dst: Rect, color: Color32) {
    // The axis they're apart on, and the facing edges along it.
    let gaps = [(dst.min.y - src.max.y, 1), (src.min.y - dst.max.y, 3), (dst.min.x - src.max.x, 0), (src.min.x - dst.max.x, 2)];
    let Some(&(gap, side)) = gaps.iter().filter(|(g, _)| *g > 0.0).max_by(|a, b| a.0.total_cmp(&b.0)) else { return };
    let k = (gap / 24.0).min(1.0);
    if k <= 0.0 {
        return;
    }
    // side: 0 = dst right of src, 1 = below, 2 = left, 3 = above. a = src edge, b = dst edge,
    // each from one end to the other along the edge; `n` points sideways (along the edges).
    let (a, b, n) = match side {
        0 => ([src.right_top(), src.right_bottom()], [dst.left_top(), dst.left_bottom()], vec2(0.0, 1.0)),
        1 => ([src.left_bottom(), src.right_bottom()], [dst.left_top(), dst.right_top()], vec2(1.0, 0.0)),
        2 => ([src.left_top(), src.left_bottom()], [dst.right_top(), dst.right_bottom()], vec2(0.0, 1.0)),
        _ => ([src.left_top(), src.right_top()], [dst.left_bottom(), dst.right_bottom()], vec2(1.0, 0.0)),
    };
    // The far end narrows toward the source's extent, so it reads as leading to the image
    // rather than flooding its whole side.
    let (b0, b1) = narrow(a, b, n);
    let ca = color.gamma_multiply(k);
    let cb = color.gamma_multiply(k * 0.12);
    let mut m = Mesh::default();
    let fa = 6.0;
    let fb = ((b1 - b0).length() * 0.25).clamp(12.0, 80.0);
    // Columns across the beam: feather, edge, edge, feather.
    let cols = [(a[0] - n * fa, b0 - n * fb, 0.0), (a[0], b0, 1.0), (a[1], b1, 1.0), (a[1] + n * fa, b1 + n * fb, 0.0)];
    for (pa, pb, w) in cols {
        m.vertices.push(vtx(pa, ca.gamma_multiply(w)));
        m.vertices.push(vtx(pb, cb.gamma_multiply(w)));
    }
    for i in 0..3u32 {
        let (l, r) = (i * 2, i * 2 + 2);
        m.indices.extend([l, l + 1, r + 1, l, r + 1, r]);
    }
    // Rays: thin streaks fanning out, a little brighter than the beam.
    for (u, v, w) in [(0.22, 0.12, 0.05), (0.5, 0.48, 0.035), (0.74, 0.86, 0.045)] {
        let (pa, pb) = (a[0].lerp(a[1], u), b0.lerp(b1, v));
        let (wa, wb) = ((a[1] - a[0]).length() * w * 0.5, (b1 - b0).length() * w);
        let base = m.vertices.len() as u32;
        let (ra, rb) = (color.gamma_multiply(k * 0.6), color.gamma_multiply(0.0));
        for (q, c) in [(pa - n * wa, Color32::TRANSPARENT), (pb - n * wb, Color32::TRANSPARENT), (pa, ra), (pb, rb), (pa + n * wa, Color32::TRANSPARENT), (pb + n * wb, Color32::TRANSPARENT)] {
            m.vertices.push(vtx(q, c));
        }
        for i in 0..2u32 {
            let (l, r) = (base + i * 2, base + i * 2 + 2);
            m.indices.extend([l, l + 1, r + 1, l, r + 1, r]);
        }
    }
    p.add(Shape::mesh(m));
}

/// The far edge, pulled in toward the near edge's span (half way), within the far edge.
fn narrow(a: [Pos2; 2], b: [Pos2; 2], n: Vec2) -> (Pos2, Pos2) {
    let along = |q: Pos2| q.to_vec2().dot(n);
    let (a0, a1, b0, b1) = (along(a[0]), along(a[1]), along(b[0]), along(b[1]));
    let mid = (a0 + a1) / 2.0;
    let half = ((b1 - b0) / 2.0).min(((a1 - a0) / 2.0) * 3.0 + 120.0);
    let center = ((b0 + b1) / 2.0 + mid) / 2.0;
    let lo = (center - half).max(b0);
    let hi = (center + half).min(b1);
    let at = |v: f32| b[0] + n * (v - b0);
    (at(lo), at(hi.max(lo)))
}

/// A slot scrolled out of `area`: a soft band of light along `area`'s edge, where it is.
pub fn edge_glow(p: &Painter, slot: Rect, area: Rect, color: Color32) {
    let (depth, feather) = (18.0, 24.0);
    // The edge it's off on: a point on the edge line, the direction along it and inward.
    let (edge, along, inward, span) = if slot.max.y <= area.min.y {
        (area.min.y, vec2(1.0, 0.0), vec2(0.0, 1.0), (slot.min.x, slot.max.x))
    } else if slot.min.y >= area.max.y {
        (area.max.y, vec2(1.0, 0.0), vec2(0.0, -1.0), (slot.min.x, slot.max.x))
    } else if slot.max.x <= area.min.x {
        (area.min.x, vec2(0.0, 1.0), vec2(1.0, 0.0), (slot.min.y, slot.max.y))
    } else {
        (area.max.x, vec2(0.0, 1.0), vec2(-1.0, 0.0), (slot.min.y, slot.max.y))
    };
    let at = |s: f32| if along.x > 0.0 { pos2(s, edge) } else { pos2(edge, s) };
    let c = color.gamma_multiply(1.6);
    let mut m = Mesh::default();
    for (s, w) in [(span.0 - feather, 0.0), (span.0, 1.0), (span.1, 1.0), (span.1 + feather, 0.0)] {
        m.vertices.push(vtx(at(s), c.gamma_multiply(w)));
        m.vertices.push(vtx(at(s) + inward * depth, Color32::TRANSPARENT));
    }
    for i in 0..3u32 {
        let (l, r) = (i * 2, i * 2 + 2);
        m.indices.extend([l, l + 1, r + 1, l, r + 1, r]);
    }
    p.with_clip_rect(area).add(Shape::mesh(m));
}

// ---- The workspace's side ----

impl Workspace {
    /// The visible area of a slot's kind (the feed scrolls; the composer doesn't).
    fn slot_area(&self, slot: SlotId) -> Rect {
        match slot {
            SlotId::Composer => self.composer,
            SlotId::Feed { .. } => self.feed_clip,
        }
    }

    /// Where `att` shows in its slot this frame.
    fn slot_place(&self, slot: SlotId, att: Att, store: &Store) -> Option<Place> {
        let rect = self.slot_rect(slot, att)?;
        let (canvas, shown) = match att {
            Att::Draft(k) => self.pending.iter().chain(&self.shelf).find(|d| d.key == k).map(|d| (d.doc.body().canvas, d.doc.body().shown()))?,
            Att::Image(id) => store.image(id).ok().map(|i| (i.body.canvas, i.body.shown()))?,
        };
        Some(Place::in_slot(canvas, shown, rect, self.slot_area(slot)))
    }

    /// Paints the flights and the glow over everything. Call after the view ran (both ends
    /// are this frame's).
    pub fn paint_motion(&mut self, ctx: &egui::Context, store: &Store, t: &Theme) {
        // Above the chat's hovered (lifted) slots, which are on Foreground.
        let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, self.id.with("flights")));
        let glow = t.colors.glow;
        let open = self.open_key();
        let view = self.view_place;
        let ends: Vec<(Att, Option<Place>)> = self.flights.flights().iter().map(|f| (f.att, self.slot_place(f.slot, f.att, store))).collect();
        let places = self.flights.places(|f| ends.iter().find(|(a, _)| *a == f.att).and_then(|(_, p)| *p), |att| if open.map(|o| o.1) == Some(att) { view } else { None });
        // Light first, the images over it.
        for (f, place, _) in &places {
            if let Some(r) = self.slot_rect(f.slot, f.att) {
                self.light(&painter, r, f.slot, place.clip, glow);
            }
        }
        if let Some((slot, att)) = open
            && !self.flights.flying_in(att)
            && let Some(view) = view
        {
            // (A slot growing in starts as a point.)
            let src = self.slot_rect(slot, att).filter(|r| self.slot_area(slot).contains(r.center()));
            match self.flights.glow_source(src.map(|r| ((slot, att), r))) {
                Some((src, alpha)) => self.beam_from(&painter, src, view.clip, glow.gamma_multiply(alpha)),
                None => {
                    if let Some(r) = self.slot_rect(slot, att) {
                        edge_glow(&painter, r, self.slot_area(slot), glow);
                    }
                }
            }
        } else if open.is_none() {
            self.flights.glow_source(None);
        }
        for (f, place, t_) in places {
            shadow(&painter, place.clip, t_, t.colors.shadow.gamma_multiply(place.alpha), t.radius.thumb);
            let mut clipped = painter.with_clip_rect(place.clip);
            clipped.set_opacity(place.alpha);
            match f.att {
                Att::Draft(k) => {
                    if let Some(d) = self.pending.iter().chain(&self.shelf).find(|d| d.key == k) {
                        self.editor.paint_body(&clipped, d.doc.body(), &d.doc, place.canvas);
                    }
                }
                Att::Image(id) => {
                    if let Ok(img) = store.image(id) {
                        self.editor.paint_body(&clipped, &img.body, store, place.canvas);
                    }
                }
            }
        }
        if self.flights.moving() {
            ctx.request_repaint();
        }
    }

    /// The beam from a slot (drawn at `slot_rect`) to an image showing at `dst`, and the
    /// slot's halo; for a slot scrolled out of view, its edge of the feed glows instead.
    fn light(&self, p: &Painter, slot_rect: Rect, slot: SlotId, dst: Rect, color: Color32) {
        let area = self.slot_area(slot);
        if area.contains(slot_rect.center()) {
            self.beam_from(p, slot_rect.intersect(area), dst, color);
        } else {
            edge_glow(p, slot_rect, area, color);
        }
    }

    /// The beam from `src` (on screen) to `dst`, and a halo around `src` within the chat.
    fn beam_from(&self, p: &Painter, src: Rect, dst: Rect, color: Color32) {
        beam(p, src, dst, color);
        halo(&p.with_clip_rect(self.feed_clip.union(self.composer)), src, 10.0, color.gamma_multiply(0.7));
    }
}

#[cfg(test)]
mod tests;
