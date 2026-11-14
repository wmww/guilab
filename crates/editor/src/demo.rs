//! A complete host for the editor without guilab: owns the doc, undo history and the view
//! (fit, wheel zoom, pan). The example and tests use it; real hosts do the same.

use std::sync::Arc;

use egui::{Key, Modifiers, PointerButton, Ui, Vec2};
use guilab_image::{Asset, AssetId, Body, Doc, Edit, Fonts, Group, IRect, Limits, Origin, Raster, Store, SvgDoc};
use guilab_kit::History;

use crate::{Canvas, CanvasOut, Editor, Xf};

pub struct DemoHost {
    pub editor: Editor,
    /// Issues asset ids (it may evict the assets: the doc holds what it uses).
    pub store: Store,
    /// `None` = "live": the host shows `live` itself and a drawing press makes a doc.
    pub doc: Option<Doc>,
    /// The screenshot a new doc is made from.
    pub live: (AssetId, Arc<Asset>),
    pub history: History<Edit>,
    pub xf: Xf,
    /// The view region `xf` was last fitted to (refit when it changes).
    fitted: Option<(IRect, egui::Rect)>,
    pub last: CanvasOut,
    /// Every edit the canvas reported, in order (tests read it).
    pub log: Vec<Edit>,
    pub scale: f32,
}

impl DemoHost {
    /// A host with a synthetic `size` "app screenshot" at UI `scale`, no doc open yet.
    pub fn new(size: [u32; 2], scale: f32) -> DemoHost {
        let mut store = Store::new(Limits::default());
        let id = store.add_asset(Asset::Raster(fake_app(size, scale)), Group(0));
        let live = (id, store.get_asset(id).unwrap());
        DemoHost {
            editor: Editor::new("demo-editor", Arc::new(Fonts::new())),
            store,
            doc: None,
            live,
            history: History::new(),
            xf: Xf { offset: Vec2::ZERO, zoom: 1.0 },
            fitted: None,
            last: CanvasOut::default(),
            log: Vec::new(),
            scale,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.live.1.as_raster().unwrap().size()
    }

    /// Opens a doc on the screenshot now (instead of on the first drawing press).
    pub fn open(&mut self) {
        let size = self.size();
        let live: std::collections::HashMap<_, _> = [self.live.clone()].into();
        self.doc = Some(Doc::open(Body::screenshot(self.live.0, size, self.scale), &live).unwrap());
    }

    pub fn undo(&mut self) {
        if let (Some(ops), Some(d)) = (self.history.undo(), &mut self.doc) {
            ops.iter().for_each(|e| d.revert(e));
        }
    }

    pub fn redo(&mut self) {
        if let (Some(ops), Some(d)) = (self.history.redo(), &mut self.doc) {
            ops.iter().for_each(|e| d.apply(e));
        }
    }

    /// Inserts a picture asset at the center of the view.
    pub fn insert(&mut self, a: Asset) {
        let id = self.store.add_asset(a, Group(0));
        let a = self.store.get_asset(id).unwrap();
        if self.doc.is_none() {
            self.open();
        }
        let d = self.doc.as_mut().unwrap();
        let region = self.editor.view_region(d.body().canvas, d.body().crop);
        let c = region.to_rect().center();
        let e = self.editor.insert(d, id, a, c, region).expect("demo picture");
        self.log.push(e.clone());
        self.history.push(e);
    }

    /// Toolbar on the left, canvas in the rest.
    pub fn ui(&mut self, ui: &mut Ui) {
        let t = guilab_kit::Theme::get(ui.ctx());
        egui::Panel::left("demo-tools").exact_size(t.size.toolbar_w).resizable(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(t.space.md);
                self.editor.toolbar(ui);
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.colors.view_bg)).show(ui, |ui| self.canvas(ui));
    }

    fn canvas(&mut self, ui: &mut Ui) {
        let clip = ui.max_rect();
        let size = self.size();
        let (canvas, crop) = match &self.doc {
            Some(d) => (d.body().canvas, d.body().crop),
            None => (size, None),
        };
        let region = self.editor.view_region(canvas, crop);
        if self.fitted != Some((region, clip)) {
            self.xf = Xf::fit(region, clip.shrink(16.0), 1.0);
            self.fitted = Some((region, clip));
        }
        // Host navigation: wheel zooms at the pointer, middle-drag pans.
        let (scroll, hover, mid) = ui.input(|i| (i.smooth_scroll_delta().y, i.pointer.hover_pos(), i.pointer.button_down(PointerButton::Middle).then_some(i.pointer.delta())));
        if let Some(p) = hover.filter(|p| clip.contains(*p))
            && scroll != 0.0
        {
            let k = (scroll / 200.0).exp();
            let q = self.xf.invert(p);
            self.xf.zoom = (self.xf.zoom * k).clamp(0.05, 32.0);
            self.xf.offset = p.to_vec2() - egui::vec2(q.x, q.y) * self.xf.zoom;
        }
        if let Some(d) = mid {
            self.xf.offset += d;
        }
        // Undo/redo, unless a text field has the keys.
        if !ui.ctx().egui_wants_keyboard_input() {
            let (redo, undo) = ui.input_mut(|i| {
                let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
                (redo, i.consume_key(Modifiers::COMMAND, Key::Z))
            });
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
        }

        // Live view: the host paints the screenshot itself.
        if self.doc.is_none() {
            let tex = ui.ctx().data(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("demo-live")));
            let tex = tex.unwrap_or_else(|| {
                let r = self.live.1.as_raster().unwrap();
                let img = egui::ColorImage::from_rgba_premultiplied([r.size()[0] as usize, r.size()[1] as usize], r.premultiplied());
                let h = ui.ctx().load_texture("demo-live", img, egui::TextureOptions::LINEAR);
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("demo-live"), h.clone()));
                h
            });
            let r = self.xf.rect(IRect::of_size(size).to_rect());
            ui.painter_at(clip).image(tex.id(), r, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }

        let out = self.editor.canvas(
            ui,
            Canvas { doc: self.doc.as_mut(), xf: self.xf, clip, new_doc: (size, self.scale), visible: true },
        );
        if out.begin {
            self.open();
        }
        if out.abandon {
            self.doc = None;
        }
        self.xf.offset += out.pan;
        if !out.edits.is_empty() {
            self.history.begin_group();
            for e in &out.edits {
                self.history.push(e.clone());
            }
            self.history.end_group();
            self.log.extend(out.edits.iter().cloned());
        }
        self.last = out;
    }
}

/// A synthetic app screenshot: window chrome, a sidebar, buttons and lines of "text".
pub fn fake_app(size: [u32; 2], scale: f32) -> Raster {
    let [w, h] = size;
    let mut px = vec![0u8; (w * h * 4) as usize];
    let s = scale;
    let mut fill = |x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 3]| {
        let (x0, y0) = ((x0 * s) as u32, (y0 * s) as u32);
        let (x1, y1) = (((x1 * s) as u32).min(w), ((y1 * s) as u32).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * w + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
    };
    let (lw, lh) = (w as f32 / s, h as f32 / s);
    fill(0.0, 0.0, lw, lh, [250, 250, 250]);
    fill(0.0, 0.0, lw, 46.0, [235, 235, 237]);
    fill(0.0, 46.0, lw, 47.0, [210, 210, 214]);
    fill(0.0, 47.0, 200.0, lh, [242, 242, 244]);
    fill(200.0, 47.0, 201.0, lh, [215, 215, 220]);
    for i in 0..6 {
        let y = 70.0 + i as f32 * 36.0;
        fill(20.0, y, 20.0 + 110.0 + (i * 17 % 50) as f32, y + 10.0, [150, 150, 158]);
    }
    fill(20.0, 66.0 + 2.0 * 36.0 - 8.0, 190.0, 66.0 + 2.0 * 36.0 + 18.0, [53, 132, 228]);
    for i in 0..9 {
        let y = 80.0 + i as f32 * 28.0;
        let len = 240.0 + (i * 53 % 230) as f32;
        fill(230.0, y, (230.0 + len).min(lw - 30.0), y + 9.0, [120, 120, 128]);
    }
    fill(lw - 230.0, lh - 64.0, lw - 130.0, lh - 30.0, [222, 222, 226]);
    fill(lw - 120.0, lh - 64.0, lw - 20.0, lh - 30.0, [53, 132, 228]);
    fill(12.0, 14.0, 26.0, 28.0, [237, 106, 94]);
    fill(34.0, 14.0, 48.0, 28.0, [245, 191, 79]);
    fill(56.0, 14.0, 70.0, 28.0, [97, 197, 84]);
    Raster::from_capture(w, h, px, 0)
}

/// A small SVG picture (a mockup button) for inserting.
pub fn sample_svg() -> Asset {
    let text = r##"<svg width="160" height="56" viewBox="0 0 160 56"><rect x="2" y="2" width="156" height="52" rx="10" fill="#2e7d32"/><circle cx="30" cy="28" r="12" fill="#fff"/><rect x="54" y="20" width="84" height="16" rx="4" fill="#c8e6c9"/></svg>"##;
    Asset::Svg(Box::new(SvgDoc::parse(text, Origin::File { name: "button.svg".into() }, None).unwrap()))
}

/// A gradient raster picture for inserting.
pub fn sample_raster(w: u32, h: u32) -> Asset {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(255 * x / w) as u8, (200 * y / h) as u8, 180, 255]);
        }
    }
    Asset::Raster(Raster::from_capture(w, h, px, 0))
}
