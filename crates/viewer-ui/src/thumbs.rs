//! Thumbnail textures for the chat: store images by id, drafts by key and revision,
//! rendered on the CPU (`guilab_image::render`) at the height the chat asks for.

use std::collections::HashMap;

use egui::{ColorImage, Context, TextureHandle, TextureId, TextureOptions};
use guilab_chat::{SlotId, SlotState, Thumbs};
use guilab_image::{Assets, Body, Fonts, RenderOpts, Store};

use super::workspace::{Att, Draft, Open};

/// Seconds an unused texture is kept (a removed slot shrinks out with its last texture).
const KEEP_SECS: f64 = 3.0;

struct Entry {
    tex: TextureHandle,
    /// Draft revision it shows (0 for store images: they never change).
    rev: u64,
    h: u32,
    used: f64,
}

#[derive(Default)]
pub struct ThumbCache {
    map: HashMap<Att, Entry>,
}

impl ThumbCache {
    /// Drops textures unused for a while.
    pub fn tick(&mut self, now: f64) {
        self.map.retain(|_, e| now - e.used < KEEP_SECS);
    }

    /// Drops everything (a new egui context).
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// What the chat reads thumbnails from, for one frame.
pub struct ThumbSource<'a> {
    pub store: &'a Store,
    pub fonts: &'a Fonts,
    pub drafts: &'a [Draft],
    pub shelf: &'a [Draft],
    pub open: &'a Open,
    pub flights: &'a crate::flight::Flights,
    pub cache: &'a mut ThumbCache,
    pub now: f64,
}

impl ThumbSource<'_> {
    fn draft(&self, key: u64) -> Option<&Draft> {
        self.drafts.iter().chain(self.shelf).find(|d| d.key == key)
    }

    /// Renders `att` at most `h` px tall, with the revision it shows. None if it's gone.
    fn render(&self, att: &Att, h: u32) -> Option<(Result<guilab_image::Rendered, guilab_image::RenderError>, u64)> {
        let opts = RenderOpts { max: Some([16384, h]), ..Default::default() };
        match att {
            Att::Draft(k) => self.draft(*k).map(|d| (guilab_image::render(d.doc.body(), &d.doc as &dyn Assets, self.fonts, &opts), d.doc.rev())),
            Att::Image(id) => self.store.image(*id).ok().map(|i| (guilab_image::render(&i.body, self.store, self.fonts, &opts), 0)),
        }
    }
}

fn aspect_of(b: &Body) -> f32 {
    let r = b.shown();
    r.w.max(1) as f32 / r.h.max(1) as f32
}

impl Thumbs<Att> for ThumbSource<'_> {
    fn aspect(&mut self, key: &Att) -> f32 {
        match key {
            Att::Draft(k) => self.draft(*k).map_or(1.6, |d| aspect_of(d.doc.body())),
            Att::Image(id) => self.store.image(*id).map_or(1.6, |i| aspect_of(&i.body)),
        }
    }

    fn texture(&mut self, ctx: &Context, key: &Att, height_px: f32) -> Option<TextureId> {
        let want = height_px.ceil().max(8.0) as u32;
        let rev = match key {
            Att::Draft(k) => self.draft(*k).map(|d| d.doc.rev()),
            Att::Image(_) => Some(0),
        };
        // Reuse a texture of this revision that's big enough and not wastefully big.
        if let Some(e) = self.cache.map.get_mut(key)
            && (rev.is_none() || Some(e.rev) == rev)
            && e.h * 4 >= want * 3
            && e.h <= want * 2
        {
            e.used = self.now;
            return Some(e.tex.id());
        }
        // Gone (removed, shrinking out): keep the last texture.
        let Some((rendered, rev)) = self.render(key, want) else { return self.cache.map.get(key).map(|e| e.tex.id()) };
        let r = rendered.map_err(|e| tracing::warn!("thumbnail: {e}")).ok()?;
        let [w, h] = r.size();
        let img = ColorImage::from_rgba_premultiplied([w as usize, h as usize], r.premultiplied());
        let tex = ctx.load_texture(format!("thumb-{key:?}"), img, TextureOptions::LINEAR);
        let id = tex.id();
        self.cache.map.insert(*key, Entry { tex, rev, h, used: self.now });
        Some(id)
    }

    fn state(&self, slot: SlotId, key: &Att) -> SlotState {
        let open = match (self.open, key) {
            (Open::Draft(k), Att::Draft(a)) => k == a && slot == SlotId::Composer,
            (Open::Past { image, slot: s, .. }, Att::Image(id)) => image == id && *s == slot,
            _ => false,
        };
        // A slot an image is flying to or from is empty meanwhile.
        if self.flights.touching(slot, *key) {
            SlotState::Covered { outlined: open }
        } else if open {
            SlotState::Away { outlined: true }
        } else {
            SlotState::Shown
        }
    }
}
