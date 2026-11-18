//! Native popups: each popup the UI shows (`guilab_kit::popup::Native`) becomes an
//! `xdg_popup` of the window (or of its parent popup), so it can extend past the window
//! and the compositor fits it to the screen (flip, slide).
//!
//! - `sync_popups` (after each UI pass) makes, repositions and destroys surfaces to match
//!   the UI's popups, parents before children (destroying children first).
//! - The compositor's placement comes back in `configure`; `popup_nudges` tells the UI how
//!   far that is from where egui put the popup, so egui moves it there (its hit testing then
//!   matches the screen).
//! - `present_popup` draws one popup's shapes; a surface shows the window-space rect
//!   `HostPopup::rect`, so input on it maps to egui as `rect.min + local` (`surface_origin`).
//! - `done` (a click outside the app, for grabbing popups) is reported to the UI, which
//!   closes the popup.

use guilab_kit::popup::{Native, Side};
use sctk::{
    compositor::{Region, Surface},
    shell::{
        WaylandSurface,
        xdg::{XdgSurface as _, popup::{Popup, PopupConfigure, PopupHandler}},
    },
};
use smithay::backend::renderer::gles::GlesRenderer;
use wayland_client::{Connection, Proxy, QueueHandle, protocol::wl_surface::WlSurface};
use wayland_protocols::{
    wp::viewporter::client::wp_viewport::WpViewport,
    xdg::shell::client::xdg_positioner::{Anchor, ConstraintAdjustment, Gravity},
};

use crate::{host::HostState, surface::SurfaceBuffers};

/// What a popup surface was made for. Geometry changes reposition it; the rest remakes it.
#[derive(Clone, Debug, PartialEq)]
struct Shape {
    parent: Option<egui::Id>,
    grab: bool,
    interactive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    /// The anchor rect relative to the parent surface (logical px; at least 1x1).
    anchor: [i32; 4],
    side: Side,
    size: (i32, i32),
    /// The surface's offset from its unconstrained place next to the anchor.
    offset: (i32, i32),
}

pub(crate) struct HostPopup {
    pub id: egui::Id,
    popup: Popup,
    viewport: Option<WpViewport>,
    pub buffers: SurfaceBuffers,
    shape: Shape,
    geometry: Geometry,
    /// Where the compositor put it, relative to its parent surface, once configured.
    placed: Option<(i32, i32)>,
    /// The window-space rect (egui points) its surface shows.
    pub rect: egui::Rect,
    /// Dismissed by the compositor: unmapped, waiting for the UI to drop it.
    dead: bool,
}

impl HostPopup {
    fn surface(&self) -> &WlSurface {
        self.popup.wl_surface()
    }
}

impl Drop for HostPopup {
    fn drop(&mut self) {
        if let Some(v) = &self.viewport {
            v.destroy();
        }
    }
}

fn round_rect(r: egui::Rect) -> [i32; 4] {
    let (x, y) = (r.min.x.round() as i32, r.min.y.round() as i32);
    [x, y, (r.max.x.round() as i32 - x).max(1), (r.max.y.round() as i32 - y).max(1)]
}

impl HostState {
    /// Where `surface`'s (0, 0) is in egui's window space: the window, or a popup.
    pub(crate) fn surface_origin(&self, surface: &WlSurface) -> Option<egui::Vec2> {
        if surface == self.window.wl_surface() {
            return Some(egui::Vec2::ZERO);
        }
        self.popups.iter().find(|p| p.surface() == surface).map(|p| p.rect.min.to_vec2())
    }

    /// Whether a shown popup covers window point `pos`.
    pub fn over_popup(&self, pos: egui::Pos2) -> bool {
        self.popups.iter().any(|p| !p.dead && p.placed.is_some() && p.rect.contains(pos))
    }

    /// A popup's position in window coordinates, from its and its parents' placements.
    fn placed_in_window(&self, i: usize) -> Option<egui::Pos2> {
        let p = &self.popups[i];
        let (x, y) = p.placed?;
        let parent = match p.shape.parent.and_then(|id| self.popups[..i].iter().position(|q| q.id == id)) {
            Some(j) => self.placed_in_window(j)?,
            None => egui::Pos2::ZERO,
        };
        Some(parent + egui::vec2(x as f32, y as f32))
    }

    /// Makes, repositions and destroys popup surfaces to match this pass's popups.
    pub fn sync_popups(&mut self, natives: &[Native]) {
        // Popups need a mapped window to attach to.
        if self.size.is_none() {
            return;
        }
        let wanted: Vec<(Shape, Geometry, egui::Rect)> = natives
            .iter()
            .map(|n| {
                let parent = n.req.parent.and_then(|id| natives.iter().find(|p| p.req.id == id));
                let origin = parent.map_or(egui::Pos2::ZERO, |p| p.rect.min);
                let anchor = n.req.anchor.translate(-origin.to_vec2());
                let at = match n.req.side {
                    Side::Below => anchor.left_bottom() + egui::vec2(0.0, n.req.gap),
                    Side::Right => anchor.right_top() + egui::vec2(n.req.gap, 0.0),
                };
                let nominal = n.nominal.translate(-origin.to_vec2());
                let offset = nominal.min - at;
                let geometry = Geometry {
                    anchor: round_rect(anchor),
                    side: n.req.side,
                    size: (n.rect.width().round() as i32, n.rect.height().round() as i32),
                    offset: (offset.x.round() as i32, offset.y.round() as i32),
                };
                (Shape { parent: parent.map(|p| p.req.id), grab: n.req.grab, interactive: n.req.interactive }, geometry, n.rect)
            })
            .collect();
        // Keep the longest matching prefix (same popups, same shape); remake the rest,
        // destroying children before parents.
        // Without `xdg_popup.reposition` (v3) a geometry change remakes it too.
        let can_move = self.xdg_base.version() >= 3;
        let keep = self
            .popups
            .iter()
            .zip(natives.iter().zip(&wanted))
            .take_while(|(h, (n, (shape, geometry, _)))| h.id == n.req.id && h.shape == *shape && !h.dead && (can_move || h.geometry == *geometry))
            .count();
        while self.popups.len() > keep {
            self.popups.pop();
        }
        for (i, (n, (shape, geometry, rect))) in natives.iter().zip(wanted).enumerate() {
            if i < keep {
                let p = &mut self.popups[i];
                p.rect = rect;
                if p.geometry != geometry {
                    p.geometry = geometry;
                    let pos = self.positioner(&geometry);
                    self.popups[i].popup.xdg_surface().set_window_geometry(0, 0, geometry.size.0.max(1), geometry.size.1.max(1));
                    self.popups[i].popup.reposition(&pos, 0);
                    pos.destroy();
                }
                continue;
            }
            match self.make_popup(n.req.id, shape, geometry, rect) {
                Ok(p) => self.popups.push(p),
                Err(e) => {
                    tracing::warn!("viewer popup: {e:#}");
                    break;
                }
            }
        }
    }

    fn positioner(&self, g: &Geometry) -> wayland_protocols::xdg::shell::client::xdg_positioner::XdgPositioner {
        let pos = self.xdg_base.create_positioner(&self.qh, ());
        pos.set_size(g.size.0.max(1), g.size.1.max(1));
        let [x, y, w, h] = g.anchor;
        pos.set_anchor_rect(x, y, w, h);
        let (anchor, gravity, adjust) = match g.side {
            Side::Below => (Anchor::BottomLeft, Gravity::BottomRight, ConstraintAdjustment::FlipY | ConstraintAdjustment::SlideX | ConstraintAdjustment::SlideY),
            Side::Right => (Anchor::TopRight, Gravity::BottomRight, ConstraintAdjustment::FlipX | ConstraintAdjustment::SlideY | ConstraintAdjustment::SlideX),
        };
        pos.set_anchor(anchor);
        pos.set_gravity(gravity);
        pos.set_offset(g.offset.0, g.offset.1);
        pos.set_constraint_adjustment(adjust);
        if pos.version() >= 3 {
            // Re-fit when the window moves or resizes.
            pos.set_reactive();
        }
        pos
    }

    fn make_popup(&mut self, id: egui::Id, shape: Shape, geometry: Geometry, rect: egui::Rect) -> anyhow::Result<HostPopup> {
        let parent = match shape.parent {
            Some(pid) => self.popups.iter().find(|p| p.id == pid).map(|p| p.popup.xdg_surface().clone()).ok_or_else(|| anyhow::anyhow!("parent popup missing"))?,
            None => self.window.xdg_surface().clone(),
        };
        let surface = Surface::new(&self.compositor, &self.qh)?;
        let wl = surface.wl_surface().clone();
        // egui draws GL-style (row 0 at the bottom), like the window.
        wl.set_buffer_transform(wayland_client::protocol::wl_output::Transform::Flipped180);
        let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(&wl, &self.qh, ()));
        if !shape.interactive {
            let region = Region::new(&self.compositor)?;
            wl.set_input_region(Some(region.wl_region()));
        }
        let pos = self.positioner(&geometry);
        let popup = Popup::from_surface(Some(&parent), &pos, &self.qh, surface, &self.xdg)?;
        pos.destroy();
        if shape.grab
            && let (Some(seat), Some(serial)) = (&self.seat, self.input_serial)
        {
            popup.xdg_popup().grab(seat, serial);
        }
        popup.xdg_surface().set_window_geometry(0, 0, geometry.size.0.max(1), geometry.size.1.max(1));
        wl.commit();
        Ok(HostPopup { id, popup, viewport, buffers: SurfaceBuffers::default(), shape, geometry, placed: None, rect, dead: false })
    }

    /// How far the compositor put each popup from where egui has it (window points).
    pub fn popup_nudges(&self) -> Vec<(egui::Id, egui::Vec2)> {
        (0..self.popups.len())
            .filter_map(|i| {
                let p = &self.popups[i];
                let at = self.placed_in_window(i)?;
                let d = at - p.rect.min;
                (!p.dead && d.length() >= 0.5).then_some((p.id, d))
            })
            .collect()
    }

    /// Draws popup `id` with `paint` (as for the window) and commits it. False if it can't
    /// be drawn now (not configured yet, dismissed, or no free buffer).
    pub fn present_popup(&mut self, id: egui::Id, renderer: &mut GlesRenderer, paint: impl FnOnce((u32, u32))) -> anyhow::Result<bool> {
        let Some(i) = self.popups.iter().position(|p| p.id == id && !p.dead && p.placed.is_some()) else { return Ok(false) };
        let (lw, lh) = self.popups[i].geometry.size;
        let (lw, lh) = (lw.max(1), lh.max(1));
        // Without a viewport: an integer buffer scale.
        let k = if self.popups[i].viewport.is_some() { self.scale } else { self.scale.round().max(1.0) };
        let size = (((lw as f64 * k).round() as u32).max(1), ((lh as f64 * k).round() as u32).max(1));
        let surface = self.popups[i].surface().clone();
        // Out of the list while rendering (`env` borrows the rest of the state).
        let mut buffers = std::mem::take(&mut self.popups[i].buffers);
        let drawn = match self.env() {
            Some(env) => buffers.render(env, renderer, &surface, size, [0.0; 4], paint),
            None => Ok(false),
        };
        self.popups[i].buffers = buffers;
        if !drawn? {
            return Ok(false);
        }
        match &self.popups[i].viewport {
            Some(v) => v.set_destination(lw, lh),
            None => surface.set_buffer_scale(k as i32),
        }
        surface.damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        surface.commit();
        Ok(true)
    }
}

impl PopupHandler for HostState {
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup, config: PopupConfigure) {
        if let Some(p) = self.popups.iter_mut().find(|p| p.surface() == popup.wl_surface()) {
            tracing::debug!("popup {:?} configured at {:?} {}x{}", p.id, config.position, config.width, config.height);
            p.placed = Some(config.position);
            self.dirty = true;
        }
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup) {
        if let Some(p) = self.popups.iter_mut().find(|p| p.surface() == popup.wl_surface()) {
            p.dead = true;
            self.dismissed.push(p.id);
            self.dirty = true;
        }
    }
}

sctk::delegate_xdg_popup!(HostState);
wayland_client::delegate_noop!(HostState: wayland_protocols::xdg::shell::client::xdg_positioner::XdgPositioner);
