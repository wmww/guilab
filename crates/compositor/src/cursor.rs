//! The cursor the session's apps want, for the viewer: a named shape (cursor-shape
//! protocol), hidden, or a client surface (`wl_pointer.set_cursor`) rendered to pixels.

use std::sync::Arc;

use smithay::{
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::compositor::{SurfaceAttributes, get_parent, with_states},
};

use crate::session::Session;

/// A cursor surface's pixels.
pub struct CursorImage {
    /// Changes whenever the image does.
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, rendered at the output scale.
    pub rgba: Vec<u8>,
    /// Hotspot in image pixels.
    pub hotspot: (i32, i32),
    /// Image pixels per logical pixel (the output scale): the cursor's real size is the
    /// image size divided by this.
    pub scale: f64,
}

/// A theme cursor by (icon, scale bits); None if the theme lacks it.
pub(crate) type NamedCursor = ((CursorIcon, u64), Option<Arc<CursorImage>>);

pub enum AppCursor {
    Named(CursorIcon),
    Hidden,
    Image(Arc<CursorImage>),
}

impl Session {
    /// Called on every commit: keeps the hotspot right and notes image changes.
    pub(crate) fn cursor_commit(&mut self, surface: &WlSurface) -> bool {
        let CursorImageStatus::Surface(cursor) = &self.cursor_status else { return false };
        let mut root = surface.clone();
        while let Some(p) = get_parent(&root) {
            root = p;
        }
        if &root != cursor {
            return false;
        }
        if surface == cursor {
            // A buffer offset moves the image, not the hotspot (as anvil does).
            with_states(surface, |s| {
                let delta = s.cached_state.get::<SurfaceAttributes>().current().buffer_delta.take();
                if let (Some(d), Some(attrs)) = (delta, s.data_map.get::<CursorImageSurfaceData>()) {
                    attrs.lock().unwrap().hotspot -= d;
                }
            });
        }
        self.cursor_seq += 1;
        true
    }

    pub(crate) fn set_cursor_status(&mut self, status: CursorImageStatus) {
        if status != self.cursor_status {
            self.cursor_status = status;
            self.cursor_seq += 1;
        }
    }

    /// Changes whenever [`Self::app_cursor`] would return something different.
    pub fn cursor_seq(&self) -> u64 {
        self.cursor_seq
    }

    /// The app's cursor. Surface cursors are rendered lazily (only when asked, and only
    /// once per change). A surface without content counts as hidden.
    pub fn app_cursor(&mut self) -> AppCursor {
        let surface = match &self.cursor_status {
            CursorImageStatus::Hidden => return AppCursor::Hidden,
            CursorImageStatus::Named(i) => return AppCursor::Named(*i),
            CursorImageStatus::Surface(s) => s.clone(),
        };
        if let Some(c) = &self.cursor_image
            && c.seq == self.cursor_seq
        {
            return AppCursor::Image(c.clone());
        }
        let scale = self.output.current_scale().fractional_scale();
        let rendered = match self.gpu.borrow_mut().render_surface(&surface, scale) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("{}: rendering the cursor: {e:#}", self.name);
                None
            }
        };
        let Some(r) = rendered else {
            self.cursor_image = None;
            return AppCursor::Hidden;
        };
        let hotspot = with_states(&surface, |s| s.data_map.get::<CursorImageSurfaceData>().map(|a| a.lock().unwrap().hotspot)).unwrap_or_default();
        let hotspot = ((hotspot.x as f64 * scale).round() as i32 - r.origin.0, (hotspot.y as f64 * scale).round() as i32 - r.origin.1);
        let img = Arc::new(CursorImage { seq: self.cursor_seq, width: r.width, height: r.height, rgba: r.rgba, hotspot, scale });
        self.cursor_image = Some(img.clone());
        AppCursor::Image(img)
    }

    /// The cursor as pixels: surface cursors as they are, named ones from the xcursor
    /// theme (`XCURSOR_THEME`, `XCURSOR_SIZE`). None if hidden or not found.
    pub fn cursor_pixels(&mut self) -> Option<Arc<CursorImage>> {
        match self.app_cursor() {
            AppCursor::Hidden => None,
            AppCursor::Image(i) => Some(i),
            AppCursor::Named(icon) => {
                let scale = self.output.current_scale().fractional_scale();
                if let Some((k, img)) = &self.named_cursor
                    && *k == (icon, scale.to_bits())
                {
                    return img.clone();
                }
                let img = load_named(icon, scale).map(Arc::new);
                self.named_cursor = Some(((icon, scale.to_bits()), img.clone()));
                img
            }
        }
    }

    /// Draws the cursor at the pointer into `rgba` (the output's pixels, `w` x `h`).
    pub(crate) fn draw_cursor(&mut self, w: u32, h: u32, rgba: &mut [u8]) {
        let Some(c) = self.cursor_pixels() else { return };
        let scale = self.output.current_scale().fractional_scale();
        let p = self.pointer_location();
        let (x0, y0) = ((p.x * scale).round() as i64 - c.hotspot.0 as i64, (p.y * scale).round() as i64 - c.hotspot.1 as i64);
        for cy in 0..c.height as i64 {
            for cx in 0..c.width as i64 {
                let (x, y) = (x0 + cx, y0 + cy);
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    continue;
                }
                let src = &c.rgba[((cy * c.width as i64 + cx) * 4) as usize..][..4];
                let dst = &mut rgba[((y * w as i64 + x) * 4) as usize..][..4];
                let a = src[3] as u32;
                for i in 0..3 {
                    dst[i] = (src[i] as u32 + dst[i] as u32 * (255 - a) / 255).min(255) as u8;
                }
            }
        }
    }
}

/// A named cursor from the xcursor theme, at the image size closest to `XCURSOR_SIZE`
/// (default 24) times `scale`, premultiplied.
fn load_named(icon: CursorIcon, scale: f64) -> Option<CursorImage> {
    let theme = std::env::var("XCURSOR_THEME").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "default".into());
    let size = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(24.0);
    let want = (size * scale).round() as u32;
    let themes = [theme.as_str(), "default", "Adwaita"];
    let path = themes.iter().find_map(|t| {
        let th = xcursor::CursorTheme::load(t);
        std::iter::once(icon.name()).chain(icon.alt_names().iter().copied()).find_map(|n| th.load_icon(n))
    })?;
    let images = xcursor::parser::parse_xcursor(&std::fs::read(path).ok()?)?;
    let best = images.iter().min_by_key(|i| (i.size as i64 - want as i64).abs())?;
    // Shown at the theme's logical size, whichever image size we got.
    let k = best.size as f64 / size;
    Some(CursorImage {
        seq: 0,
        width: best.width,
        height: best.height,
        rgba: best.pixels_rgba.clone(),
        hotspot: (best.xhot as i32, best.yhot as i32),
        scale: k,
    })
}
