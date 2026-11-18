//! Popups: menus and tooltips. Every popup in the UI goes through [`Popup`] (egui's own
//! tooltips, popups and context menus are disallowed in `clippy.toml`), so they all behave
//! the same way.
//!
//! Two ways to show them:
//! - **In the window** (default): an egui area kept inside the window, flipped to the
//!   anchor's other side when it doesn't fit.
//! - **Native**, once the host calls [`enable_native`]: each popup may extend past the window.
//!   At the end of each pass the host takes them with [`take_native`] (their shapes are
//!   removed from the window's) and shows each as a real popup surface (`xdg_popup`) at the
//!   anchor, letting the compositor flip or slide it to fit the screen. Where it actually
//!   went comes back through [`nudge`], so egui's idea of the popup's place matches the
//!   screen; its dismissal (a click outside the app) through [`dismiss`].
//!
//! Coordinates stay egui's (window points) throughout: a native popup's surface shows the
//! window-space rect [`Native::rect`], and input on it maps back as `rect.min + local`.

use std::collections::{HashMap, HashSet};

use egui::{Context, Id, LayerId, Order, Pos2, Rect, Response, Ui, Vec2, epaint::ClippedShape, pos2};

use crate::Theme;

/// Where a popup goes next to its anchor; it flips to the opposite side if it doesn't fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// Under the anchor, left edges aligned (else above).
    Below,
    /// Right of the anchor, top edges aligned (else left).
    Right,
}

/// A popup as registered this pass.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: Id,
    pub layer: LayerId,
    /// The popup this one was shown from (inside its contents), if any.
    pub parent: Option<Id>,
    /// What it's placed next to, in window points.
    pub anchor: Rect,
    pub side: Side,
    pub gap: f32,
    /// Takes the pointer and keyboard while open; a click outside the app dismisses it.
    pub grab: bool,
    /// Takes input at all (menus). Tooltips don't: the pointer passes through.
    pub interactive: bool,
}

/// A popup for the host to show natively.
pub struct Native {
    pub req: Request,
    /// The surface's area in window points: the popup's visual bounds (shadow included),
    /// rounded out to whole points.
    pub rect: Rect,
    /// `rect` without the host's corrections ([`nudge`]): where the popup asked to be.
    pub nominal: Rect,
    /// Its shapes, in window points.
    pub shapes: Vec<ClippedShape>,
}

/// This pass's popups (reset each pass) and the stack of popups being filled.
#[derive(Clone, Default)]
struct Pass {
    pass: u64,
    reqs: Vec<Request>,
    stack: Vec<Id>,
}

/// From the host, kept across passes.
#[derive(Clone, Default)]
struct Host {
    native: bool,
    /// Per popup: how far the compositor moved it from where it asked to be.
    offsets: HashMap<Id, Vec2>,
    dismissed: HashSet<Id>,
}

fn pass_id() -> Id {
    Id::new("guilab-kit-popups-pass")
}

fn host_id() -> Id {
    Id::new("guilab-kit-popups-host")
}

fn with_pass<R>(ctx: &Context, f: impl FnOnce(&mut Pass) -> R) -> R {
    let nr = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let p = d.get_temp_mut_or_default::<Pass>(pass_id());
        if p.pass != nr {
            *p = Pass { pass: nr, ..Default::default() };
        }
        f(p)
    })
}

fn with_host<R>(ctx: &Context, f: impl FnOnce(&mut Host) -> R) -> R {
    ctx.data_mut(|d| f(d.get_temp_mut_or_default::<Host>(host_id())))
}

/// The host shows popups as real popup surfaces from now on.
pub fn enable_native(ctx: &Context) {
    with_host(ctx, |h| h.native = true);
}

/// Whether popups are native (see [`enable_native`]).
pub fn is_native(ctx: &Context) -> bool {
    with_host(ctx, |h| h.native)
}

/// The compositor put popup `id` `delta` away from its [`Native::rect`]; it's drawn there
/// from the next pass on.
pub fn nudge(ctx: &Context, id: Id, delta: Vec2) {
    with_host(ctx, |h| *h.offsets.entry(id).or_default() += delta);
}

/// The compositor dismissed these popups (a click outside the app); their owners close them.
pub fn dismiss(ctx: &Context, ids: impl IntoIterator<Item = Id>) {
    with_host(ctx, |h| h.dismissed.extend(ids));
}

/// Takes this pass's native popups and removes their shapes from the window's. Call at the
/// end of the pass (after the whole UI ran).
pub fn take_native(ctx: &Context) -> Vec<Native> {
    let reqs = with_pass(ctx, |p| p.reqs.clone());
    let offsets = with_host(ctx, |h| {
        // Popups that are gone start over next time.
        h.offsets.retain(|id, _| reqs.iter().any(|r| r.id == *id));
        h.dismissed.clear();
        h.offsets.clone()
    });
    reqs.into_iter()
        .filter_map(|req| {
            let shapes: Vec<ClippedShape> = ctx.graphics_mut(|g| g.get_mut(req.layer).map(std::mem::take)).map(|l| l.all_entries().cloned().collect()).unwrap_or_default();
            let bounds = shapes.iter().map(|s| s.shape.visual_bounding_rect().intersect(s.clip_rect)).filter(|r| r.is_positive()).reduce(|a, b| a.union(b))?;
            let rect = Rect::from_min_max(pos2(bounds.min.x.floor(), bounds.min.y.floor()), pos2(bounds.max.x.ceil(), bounds.max.y.ceil()));
            let nominal = rect.translate(-offsets.get(&req.id).copied().unwrap_or_default());
            Some(Native { req, rect, nominal, shapes })
        })
        .collect()
}

/// Where a popup of `size` goes next to `anchor`; flipped if `bounds` are given and it
/// doesn't fit on its side but does on the other.
fn place(anchor: Rect, side: Side, gap: f32, size: Vec2, bounds: Option<Rect>) -> Pos2 {
    match side {
        Side::Below => {
            let mut p = pos2(anchor.min.x, anchor.max.y + gap);
            if let Some(b) = bounds
                && p.y + size.y > b.max.y
                && anchor.min.y - gap - size.y >= b.min.y
            {
                p.y = anchor.min.y - gap - size.y;
            }
            p
        }
        Side::Right => {
            let mut p = pos2(anchor.max.x + gap, anchor.min.y);
            if let Some(b) = bounds
                && p.x + size.x > b.max.x
                && anchor.min.x - gap - size.x >= b.min.x
            {
                p.x = anchor.min.x - gap - size.x;
            }
            p
        }
    }
}

/// What [`Popup::show`] returns.
pub struct PopupResponse<R> {
    pub inner: R,
    pub response: Response,
    /// The compositor dismissed it (a click outside the app): close it.
    pub dismissed: bool,
}

/// A popup next to an anchor rect. Shown from inside another popup's contents, it's that
/// popup's child (a submenu, a tooltip in a menu).
pub struct Popup {
    id: Id,
    anchor: Rect,
    side: Side,
    gap: f32,
    grab: bool,
    order: Order,
}

impl Popup {
    /// A menu below `anchor` (e.g. its title): takes input, grabs.
    pub fn menu(id: Id, anchor: Rect) -> Popup {
        Popup { id, anchor, side: Side::Below, gap: 0.0, grab: true, order: Order::Foreground }
    }

    /// A tooltip next to `anchor`: no input. Its layer is egui's tooltip layer for the widget
    /// `id`, so egui's tooltip timing (`Tooltip::should_show_tooltip`) sees it.
    pub fn tooltip(widget: Id, anchor: Rect, side: Side, gap: f32) -> Popup {
        Popup { id: egui::Tooltip::tooltip_id(widget, 0), anchor, side, gap, grab: false, order: Order::Tooltip }
    }

    pub fn side(mut self, side: Side) -> Popup {
        self.side = side;
        self
    }

    pub fn gap(mut self, gap: f32) -> Popup {
        self.gap = gap;
        self
    }

    pub fn show<R>(self, ctx: &Context, add: impl FnOnce(&mut Ui) -> R) -> PopupResponse<R> {
        let interactive = self.order != Order::Tooltip;
        let layer = LayerId::new(self.order, self.id);
        let (native, offset, dismissed) = with_host(ctx, |h| (h.native, h.offsets.get(&self.id).copied().unwrap_or_default(), h.dismissed.contains(&self.id)));
        let parent = with_pass(ctx, |p| p.stack.last().copied());
        let size = ctx.memory(|m| m.area_rect(self.id)).map_or(Vec2::ZERO, |r| r.size());
        let area = egui::Area::new(self.id).order(self.order).interactable(interactive);
        let area = if native {
            with_pass(ctx, |p| {
                p.reqs.push(Request { id: self.id, layer, parent, anchor: self.anchor, side: self.side, gap: self.gap, grab: self.grab, interactive });
            });
            // The compositor fits it to the screen; egui mustn't clip or move it.
            area.fixed_pos(place(self.anchor, self.side, self.gap, size, None) + offset).constrain(false).constrain_to(Rect::EVERYTHING)
        } else {
            area.fixed_pos(place(self.anchor, self.side, self.gap, size, Some(ctx.content_rect()))).constrain(true)
        };
        with_pass(ctx, |p| p.stack.push(self.id));
        let r = area.show(ctx, add);
        with_pass(ctx, |p| {
            p.stack.pop();
        });
        PopupResponse { inner: r.inner, response: r.response, dismissed }
    }
}

/// Tooltips for any widget: `.tip(..)` below it, `.side_tip(..)` beside it (right, else
/// left; for widgets in a vertical strip, where one below would cover the next).
pub trait Tip {
    fn tip(self, text: &str) -> Self;
    fn side_tip(self, text: &str) -> Self;
}

impl Tip for Response {
    fn tip(self, text: &str) -> Response {
        tooltip(&self, Side::Below, text);
        self
    }

    fn side_tip(self, text: &str) -> Response {
        tooltip(&self, Side::Right, text);
        self
    }
}

fn tooltip(resp: &Response, side: Side, text: &str) {
    if !resp.enabled() || !egui::Tooltip::should_show_tooltip(resp, false) {
        return;
    }
    let t = Theme::get(&resp.ctx);
    let ctx = resp.ctx.clone();
    Popup::tooltip(resp.id, resp.rect, side, t.space.sm).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_max_width(t.size.tooltip_w);
            ui.style_mut().interaction.selectable_labels = false;
            ui.label(text);
        });
    });
    // egui's tooltip timing: the next tooltip shows at once while moving between widgets.
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp::<f64>(Id::new("when_was_a_toolip_last_shown"), now));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn place_flips_only_when_the_other_side_fits() {
        let b = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        let anchor = Rect::from_min_size(pos2(10.0, 80.0), vec2(20.0, 10.0));
        assert_eq!(place(anchor, Side::Below, 0.0, vec2(30.0, 40.0), Some(b)), pos2(10.0, 40.0));
        assert_eq!(place(anchor, Side::Below, 0.0, vec2(30.0, 90.0), Some(b)), pos2(10.0, 90.0), "fits neither: stays");
        assert_eq!(place(anchor, Side::Below, 0.0, vec2(30.0, 40.0), None), pos2(10.0, 90.0), "native: the compositor flips");
        let anchor = Rect::from_min_size(pos2(80.0, 10.0), vec2(10.0, 10.0));
        assert_eq!(place(anchor, Side::Right, 2.0, vec2(30.0, 10.0), Some(b)), pos2(48.0, 10.0));
    }
}
