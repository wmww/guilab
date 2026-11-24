//! One thumbnail slot, shared by the feed and the composer: hover grow (lifted onto a
//! foreground layer, no relayout), the open state (washed),
//! center-cropped extreme aspects, the remove badge, click to open.

use guilab_kit::Tip as _;
use std::collections::HashMap;
use std::hash::Hash;

use egui::{Color32, Context, CursorIcon, Id, LayerId, Order, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, TextureId, Ui, pos2, vec2};
use guilab_kit::paint_close_badge;
use guilab_kit::anim::Fader;
use guilab_kit::Theme;

use crate::{ChatEvent, ChatOut, SlotId, SlotState};

struct Anim {
    /// 1 = washed (its image is open).
    wash: Fader,
    hover: Fader,
    /// The remove badge's opacity: gone while covered, fading back after.
    badge: Fader,
    seen: u64,
}

impl Anim {
    fn moving(&self, now: f64) -> bool {
        self.wash.moving(now) || self.hover.moving(now) || self.badge.moving(now)
    }
}

pub(crate) struct Slots<K> {
    anims: HashMap<(SlotId, K), Anim>,
    frame: u64,
    now: f64,
}

impl<K> Default for Slots<K> {
    fn default() -> Self {
        Slots { anims: HashMap::new(), frame: 0, now: 0.0 }
    }
}

/// How one slot is drawn.
pub(crate) struct Look {
    /// The layout rect at rest (full size, before `appear`).
    pub rest: Rect,
    /// Image aspect (uncropped), for the center crop.
    pub aspect: f32,
    /// The rest size is faint (smaller, dimmed).
    pub faint: bool,
    /// 0..1: growing in or shrinking out (composer).
    pub appear: f32,
    /// Shows the remove badge.
    pub removable: bool,
    /// Shrinking out after removal: no input, the cached texture, the last state.
    pub dying: bool,
}

/// The aspect a slot is laid out with: extreme images are center-cropped.
pub(crate) fn slot_aspect(t: &Theme, aspect: f32) -> f32 {
    let max = t.size.thumb_aspect_max;
    if aspect.is_finite() && aspect > 0.0 { aspect.clamp(1.0 / max, max) } else { 1.0 }
}

impl<K: Clone + Eq + Hash> Slots<K> {
    pub fn begin_frame(&mut self, ctx: &Context) {
        self.frame += 1;
        self.now = ctx.input(|i| i.time);
    }

    /// Drops state of slots not shown any more; keeps repainting while something moves.
    pub fn end_frame(&mut self, ctx: &Context) {
        let (frame, now) = (self.frame, self.now);
        self.anims.retain(|_, a| a.seen + 2 >= frame);
        if self.anims.values().any(|a| a.moving(now)) {
            ctx.request_repaint();
        }
    }

    /// Lays out nothing (the caller allocated `look.rest`); paints the slot and handles its
    /// input. `texture` is asked only when the slot is on screen. Returns the texture used.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        chat: Id,
        slot: SlotId,
        key: &K,
        look: Look,
        state: Option<SlotState>,
        texture: impl FnOnce(f32) -> Option<TextureId>,
        out: &mut ChatOut<K>,
    ) -> Option<TextureId> {
        let now = self.now;
        let ctx = ui.ctx().clone();
        let a = self.anims.entry((slot, key.clone())).or_insert_with(|| {
            let st = state.unwrap_or_default();
            Anim { wash: Fader::at(washed(st)), hover: Fader::at(0.0), badge: Fader::at(badge_on(st)), seen: 0 }
        });
        a.seen = self.frame;
        if let Some(st) = state {
            a.wash.set(washed(st), now, t.motion.fade);
            // Gone at once when covered, fading in after (no pop at a landing).
            if badge_on(st) == 0.0 {
                a.badge = Fader::at(0.0);
            } else {
                a.badge.set(1.0, now, t.motion.fade);
            }
        }

        // Hover grow: to `hover_grow` of full size, faint slots too (they grow more).
        let base = if look.faint { t.size.faint_scale } else { 1.0 };
        let grow = t.motion.hover_grow / base;
        let screen = ctx.content_rect();
        let drawn_at = |h: f32| {
            let r = scale(look.rest, (1.0 + (grow - 1.0) * h) * look.appear);
            // A grown thumbnail stays on screen (only shifted for the growth).
            if screen.contains_rect(look.rest) { keep_inside(r, screen) } else { r }
        };
        let lifted_before = a.hover.to > 0.5;
        let mut drawn = drawn_at(a.hover.value(now));
        let id = chat.with(("slot", slot, hash(key)));
        if !look.dying {
            let resp = ui.interact(if lifted_before { drawn } else { scale(look.rest, look.appear) }, id, Sense::click());
            // The badge is painted with the slot below; registered after the thumbnail, its
            // input wins over the thumbnail's.
            let badge_hover = look.removable && a.badge.to > 0.5 && {
                let r = Rect::from_center_size(badge_center(t, drawn), egui::Vec2::splat(t.size.close_badge));
                let b = ui.interact(r, id.with("x"), Sense::click()).tip("Remove");
                if b.clicked() {
                    out.events.push(ChatEvent::Remove { key: key.clone() });
                }
                b.hovered()
            };
            let hovered = (resp.hovered() || badge_hover) && a.wash.to < 0.5;
            if resp.hovered() {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
            a.hover.set(hovered as u8 as f32, now, t.motion.hover);
            drawn = drawn_at(a.hover.value(now));
            if resp.clicked() {
                out.events.push(ChatEvent::Open { slot, key: key.clone(), rect: drawn });
            }
            out.slots.push((slot, key.clone(), drawn));
        }

        let h = a.hover.value(now);
        let lifted = h > 0.001;
        let visible = lifted || ui.is_rect_visible(drawn);
        if !visible {
            return None;
        }
        let painter = if lifted {
            ctx.layer_painter(LayerId::new(Order::Foreground, chat.with("lifted")))
        } else {
            ui.painter().clone()
        };
        let c = &t.colors;
        let radius = t.radius.thumb;
        let opacity = look.appear.min(1.0) * if look.faint { lerp(t.size.faint_opacity, 1.0, h) } else { 1.0 };
        let wash = a.wash.value(now);
        if lifted {
            let mut shadow = t.shadow.lifted;
            shadow.color = shadow.color.gamma_multiply(h);
            painter.add(shadow.as_shape(drawn, radius));
        }
        let height_px = look.rest.height() * grow * ctx.pixels_per_point();
        let used = texture(height_px);
        match used {
            Some(tex) => {
                let uv = crop_uv(look.aspect, drawn.width() / drawn.height());
                painter.add(egui::epaint::RectShape::filled(drawn, radius, Color32::WHITE.gamma_multiply(opacity)).with_texture(tex, uv));
            }
            None => placeholder(&painter, t, drawn, opacity),
        }
        if wash > 0.0 {
            let hot = ui.rect_contains_pointer(drawn) && !look.dying;
            painter.rect_filled(drawn, radius, c.slot_open.gamma_multiply(wash * opacity * if hot { 1.3 } else { 1.0 }));
        }
        let badge = a.badge.value(now);
        if look.removable && !look.dying && look.appear > 0.5 && badge > 0.0 {
            let center = badge_center(t, drawn);
            let hot = ui.rect_contains_pointer(Rect::from_center_size(center, egui::Vec2::splat(t.size.close_badge)));
            paint_close_badge(&painter, t, center, hot, badge);
        }
        used
    }
}

fn hash(k: &impl Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(k)
}

fn washed(s: SlotState) -> f32 {
    match s {
        SlotState::Shown | SlotState::Covered { open: false } => 0.0,
        SlotState::Open | SlotState::Covered { open: true } => 1.0,
    }
}

fn badge_on(s: SlotState) -> f32 {
    if matches!(s, SlotState::Covered { .. }) { 0.0 } else { 1.0 }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn scale(r: Rect, k: f32) -> Rect {
    Rect::from_center_size(r.center(), r.size() * k)
}

/// Shifts `r` into `bounds` where it fits.
fn keep_inside(r: Rect, bounds: Rect) -> Rect {
    let mut d = vec2(0.0, 0.0);
    if r.width() <= bounds.width() {
        d.x = (bounds.min.x - r.min.x).max(0.0) + (bounds.max.x - r.max.x).min(0.0);
    }
    if r.height() <= bounds.height() {
        d.y = (bounds.min.y - r.min.y).max(0.0) + (bounds.max.y - r.max.y).min(0.0);
    }
    r.translate(d)
}

/// UVs showing the middle of an image of `aspect` in a slot of `slot` aspect.
fn crop_uv(aspect: f32, slot: f32) -> Rect {
    if !(aspect.is_finite() && aspect > 0.0) || (aspect - slot).abs() < 1e-3 {
        return Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    }
    if aspect > slot {
        let w = slot / aspect;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = aspect / slot;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}

fn badge_center(t: &Theme, r: Rect) -> Pos2 {
    let off = t.size.close_badge / 2.0 + t.space.sm;
    r.right_top() + vec2(-off, off)
}

/// No texture: the empty fill with a small picture glyph.
fn placeholder(p: &Painter, t: &Theme, r: Rect, k: f32) {
    let c = &t.colors;
    p.rect_filled(r, t.radius.thumb, c.slot_empty.gamma_multiply(k));
    let s = (r.width().min(r.height()) * 0.32).max(8.0);
    let g = Rect::from_center_size(r.center(), vec2(s * 1.25, s));
    let col = c.weak.gamma_multiply(k);
    let stroke = Stroke::new(t.size.icon_stroke, col);
    p.rect_stroke(g, t.radius.small, stroke, StrokeKind::Middle);
    p.circle_filled(g.min + vec2(g.width() * 0.7, g.height() * 0.3), s * 0.09, col);
    p.line_segment([g.left_bottom() + vec2(s * 0.12, -s * 0.12), g.min + vec2(g.width() * 0.4, g.height() * 0.45)], stroke);
    p.line_segment([g.min + vec2(g.width() * 0.4, g.height() * 0.45), g.right_bottom() + vec2(-s * 0.12, -s * 0.12)], stroke);
}
