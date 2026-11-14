//! Eased animations on top of egui's clock. Timings come from [`crate::Theme::motion`].

use egui::{Context, Id, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    OutCubic,
    InOutCubic,
}

impl Easing {
    /// Maps linear progress `t` (0..=1) to eased progress.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::OutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::InOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

/// A duration and an easing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub secs: f32,
    pub easing: Easing,
}

#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    t0: f64,
}

/// Animates toward `target`: when the target changes, eases from the current value to the
/// new one over `m`. The first call jumps straight to `target`. Repaints while moving.
pub fn tween(ctx: &Context, id: Id, target: f32, m: Motion) -> f32 {
    let now = ctx.input(|i| i.time);
    let mut tw = ctx.data(|d| d.get_temp::<Tween>(id)).unwrap_or(Tween { from: target, to: target, t0: now - m.secs as f64 });
    let progress = |tw: &Tween| if m.secs <= 0.0 { 1.0 } else { ((now - tw.t0) / m.secs as f64) as f32 };
    let value = |tw: &Tween| tw.from + (tw.to - tw.from) * m.easing.apply(progress(tw));
    if tw.to != target {
        tw = Tween { from: value(&tw), to: target, t0: now };
        ctx.data_mut(|d| d.insert_temp(id, tw));
    } else if !ctx.data(|d| d.get_temp::<Tween>(id).is_some()) {
        ctx.data_mut(|d| d.insert_temp(id, tw));
    }
    if progress(&tw) < 1.0 {
        ctx.request_repaint();
    }
    value(&tw)
}

/// Linear interpolation between two rects (`t` already eased).
pub fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min.lerp(b.min, t), a.max.lerp(b.max, t))
}

/// Progress (eased, 0..=1) of something that started at egui time `t0`.
pub fn progress(ctx: &Context, t0: f64, m: Motion) -> f32 {
    let now = ctx.input(|i| i.time);
    let t = if m.secs <= 0.0 { 1.0 } else { ((now - t0) / m.secs as f64) as f32 };
    if t < 1.0 {
        ctx.request_repaint();
    }
    m.easing.apply(t)
}

/// A value easing toward a target.
#[derive(Clone, Copy, Debug)]
pub struct Fader {
    from: f32,
    pub to: f32,
    t0: f64,
    m: Motion,
}

/// No animation: changes apply at once.
pub const INSTANT: Motion = Motion { secs: 0.0, easing: Easing::Linear };

impl Fader {
    /// Resting at `v`.
    pub fn at(v: f32) -> Fader {
        Fader { from: v, to: v, t0: f64::NEG_INFINITY, m: INSTANT }
    }

    /// The value at egui time `now`.
    pub fn value(&self, now: f64) -> f32 {
        if self.m.secs <= 0.0 {
            return self.to;
        }
        let k = self.m.easing.apply(((now - self.t0) / self.m.secs as f64) as f32);
        self.from + (self.to - self.from) * k
    }

    /// Eases from the current value to `target` over `m` (no-op if that's the target already).
    pub fn set(&mut self, target: f32, now: f64, m: Motion) {
        if target != self.to {
            *self = Fader { from: self.value(now), to: target, t0: now, m };
        }
    }

    /// Still easing at `now`.
    pub fn moving(&self, now: f64) -> bool {
        self.m.secs > 0.0 && now - self.t0 < self.m.secs as f64
    }
}

