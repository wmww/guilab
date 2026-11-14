//! A headless stand-in for the server around a [`ViewerUi`]: one session showing a
//! synthetic app frame, a store, and the server's answers to `Capture` and `Message`.
//! Tests and the `motion_shots` example drive it with synthetic input and paint frames on
//! the CPU (`guilab_kit::headless`).

use std::sync::Arc;

use egui::{Color32, ColorImage, FullOutput, Pos2, TextureHandle, TextureOptions, Vec2};
use guilab_chat::Item;
use guilab_image::{Body, Fonts, Group, ImageId, Limits, Made, NoAssets, Store};
use guilab_kit::headless::Headless;

use crate::input::{HostCursor, HostInput};
use crate::{Action, SessionView, ViewerUi};

pub const SESSION: &str = "demo";

pub struct Demo {
    pub h: Headless,
    pub ui: ViewerUi,
    pub store: Store,
    group: Group,
    /// The live app's frame and its texture.
    app: ColorImage,
    tex: Option<TextureHandle>,
    pub scale: f64,
    /// Actions the demo doesn't handle itself (inputs to the app, saves, ...).
    pub actions: Vec<Action>,
    feed: u64,
    /// More sessions (tabs) showing the same frame.
    pub extra: Vec<String>,
    /// An agent holds the session's lock.
    pub locked: bool,
    pub agent: Option<guilab_chat::Status>,
    /// Paint the pointer and the cursor's name into frames (the real cursor isn't part of one).
    pub mark_pointer: bool,
    pointer: Option<Pos2>,
    cursor: egui::CursorIcon,
}

impl Demo {
    /// A window of `size` points at `ppp` showing an app of `app` pixels.
    pub fn new(size: Vec2, ppp: f32, app: [usize; 2], dark: bool) -> Demo {
        let h = Headless::new(size, ppp);
        guilab_kit::theme::install(&h.ctx);
        h.ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
        let mut ui = ViewerUi::new(Arc::new(Fonts::new()));
        ui.theme = if dark { guilab_kit::ThemePref::Dark } else { guilab_kit::ThemePref::Light };
        Demo {
            h,
            ui,
            store: Store::new(Limits::default()),
            group: Group(1),
            app: fake_app(app[0], app[1], 0),
            tex: None, scale: ppp as f64,
            actions: Vec::new(),
            feed: 0,
            extra: Vec::new(),
            locked: false,
            agent: None,
            mark_pointer: false,
            pointer: None,
            cursor: egui::CursorIcon::Default,
        }
    }

    /// Runs one frame `dt` seconds after the last, then answers its actions like the server.
    pub fn frame(&mut self, input: Vec<HostInput>, dt: f64) -> FullOutput {
        let events: Vec<egui::Event> = input.iter().filter_map(|i| if let HostInput::Egui(e) = i { Some(e.clone()) } else { None }).collect();
        let app = &self.app;
        let tex = self.tex.get_or_insert_with(|| self.h.ctx.load_texture("demo-app", app.clone(), TextureOptions::LINEAR)).id();
        for e in &events {
            if let egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. } = e {
                self.pointer = Some(*p);
            }
        }
        let size = (self.app.size[0] as u32, self.app.size[1] as u32);
        let names: Vec<String> = std::iter::once(SESSION.to_string()).chain(self.extra.iter().cloned()).collect();
        let (scale, locked, agent) = (self.scale, self.locked, self.agent.clone());
        fn view<'a>(name: &'a str, tex: egui::TextureId, size: (u32, u32), scale: f64, locked: bool, agent: Option<guilab_chat::Status>) -> SessionView<'a> {
            SessionView { name, key: name, texture: Some(tex), size, scale, locked, cursor: HostCursor::Icon(egui::CursorIcon::Default), viz: None, under_pointer: None, agent }
        }
        let sessions: Vec<SessionView> = names.iter().map(|n| view(n, tex, size, scale, locked, agent.clone())).collect();
        let (ui, store) = (&mut self.ui, &self.store);
        let mut actions = Vec::new();
        let mut input = Some(input);
        let mark = self.mark_pointer.then_some(self.pointer).flatten().map(|p| (p, self.cursor));
        let out = self.h.run(events, dt, |eu| {
            actions = ui.show(eu, &sessions, input.take().unwrap_or_default(), false, store);
            if let Some((p, cursor)) = mark {
                mark_pointer(eu.ctx(), p, cursor);
            }
        });
        self.cursor = out.platform_output.cursor_icon;
        for a in actions {
            match a {
                Action::Capture { ticket, .. } => {
                    let img = self.screenshot().and_then(|id| self.store.image(id).map_err(|e| e.to_string()));
                    self.ui.captured(SESSION, ticket, img, &self.store);
                }
                Action::Message { text, drafts, .. } => {
                    let ids: Vec<ImageId> = drafts.iter().filter_map(|(parent, doc)| self.store.add_image(*parent, doc.body().clone(), Made::Human, self.group, doc).ok()).collect();
                    self.push(Item::me("12:00", text).with(ids));
                }
                a => self.actions.push(a),
            }
        }
        out
    }

    /// A screenshot of the app into the store.
    pub fn screenshot(&mut self) -> Result<ImageId, String> {
        let [w, h] = self.app.size;
        let asset = guilab_image::asset::load_bytes(&png(&self.app), "app.png").map_err(|e| e.to_string())?;
        let a = self.store.add_asset(asset, self.group);
        let body = Body::screenshot(a, [w as u32, h as u32], self.scale as f32);
        self.store.add_image(None, body, Made::Screenshot { at_ms: 0, by_agent: true }, self.group, &NoAssets).map_err(|e| e.to_string())
    }

    /// Adds a feed item.
    pub fn push(&mut self, item: Item<ImageId>) {
        self.feed += 1;
        self.ui.push_feed(SESSION, SESSION, vec![item], self.feed);
    }

    /// Paints a frame's output.
    pub fn render(&mut self, out: &FullOutput) -> ColorImage {
        self.h.render(out, Color32::BLACK)
    }

    /// Runs `n` frames at 60 Hz without input.
    pub fn idle(&mut self, n: usize) -> FullOutput {
        let mut out = self.frame(vec![], 1.0 / 60.0);
        for _ in 1..n {
            out = self.frame(vec![], 1.0 / 60.0);
        }
        out
    }

    /// Moves the pointer to `p`, then presses and releases there (two frames).
    pub fn click(&mut self, p: Pos2) -> FullOutput {
        self.frame(vec![HostInput::Egui(egui::Event::PointerMoved(p))], 1.0 / 60.0);
        self.frame(vec![HostInput::Egui(press(p, true)), HostInput::Egui(press(p, false))], 1.0 / 60.0)
    }
}

/// A crosshair at the pointer and the cursor's name next to it, over everything.
fn mark_pointer(ctx: &egui::Context, p: Pos2, cursor: egui::CursorIcon) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::new("demo-pointer")));
    let m = Color32::from_rgb(255, 0, 255);
    painter.line_segment([p - Vec2::new(7.0, 0.0), p + Vec2::new(7.0, 0.0)], (1.0, m));
    painter.line_segment([p - Vec2::new(0.0, 7.0), p + Vec2::new(0.0, 7.0)], (1.0, m));
    let text = format!("{cursor:?}");
    let font = egui::FontId::monospace(11.0);
    let at = p + Vec2::new(10.0, 10.0);
    let r = painter.text(at, egui::Align2::LEFT_TOP, &text, font.clone(), Color32::WHITE);
    painter.rect_filled(r.expand(2.0), 2.0, Color32::from_black_alpha(200));
    painter.text(at, egui::Align2::LEFT_TOP, text, font, Color32::WHITE);
}

pub fn press(p: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// A plain picture of an app window (title bar, sidebar, a few controls), hue from `seed`.
pub fn fake_app(w: usize, h: usize, seed: u32) -> ColorImage {
    let hue = [[66, 133, 244], [219, 68, 55], [15, 157, 88], [244, 160, 0]][seed as usize % 4];
    let mut img = ColorImage::new([w, h], vec![Color32::from_rgb(246, 246, 248); w * h]);
    let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize, c: Color32| {
        for y in y0.min(h)..y1.min(h) {
            for x in x0.min(w)..x1.min(w) {
                img.pixels[y * w + x] = c;
            }
        }
    };
    let s = h / 40;
    fill(0, 0, w, s * 3, Color32::from_rgb(hue[0], hue[1], hue[2]));
    fill(0, s * 3, w / 4, h, Color32::from_rgb(228, 230, 236));
    for i in 0..6 {
        let y = s * 5 + i * s * 3;
        fill(s, y, w / 4 - s, y + s, Color32::from_rgb(190, 194, 204));
    }
    for i in 0..8 {
        let y = s * 6 + i * s * 3;
        fill(w / 4 + s * 2, y, w - s * (3 + (i % 3) * 6), y + s, Color32::from_rgb(205, 208, 216));
    }
    fill(w - s * 12, h - s * 5, w - s * 2, h - s * 2, Color32::from_rgb(hue[0], hue[1], hue[2]));
    img
}

/// An image as PNG bytes (no render mark: it's a picture of the app, not a rendering).
pub fn png(img: &ColorImage) -> Vec<u8> {
    let mut v = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut v, img.size[0] as u32, img.size[1] as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(img.as_raw()).expect("png data");
    }
    v
}

/// Frames side by side in rows of `cols`, each shrunk by `k` (box filter), on a gray gap.
pub fn contact_sheet(frames: &[ColorImage], cols: usize, k: usize) -> ColorImage {
    let [fw, fh] = frames[0].size.map(|v| v / k);
    let gap = 6;
    let rows = frames.len().div_ceil(cols);
    let (w, h) = (cols * (fw + gap) + gap, rows * (fh + gap) + gap);
    let mut out = ColorImage::new([w, h], vec![Color32::from_gray(128); w * h]);
    for (i, f) in frames.iter().enumerate() {
        let (ox, oy) = (gap + (i % cols) * (fw + gap), gap + (i / cols) * (fh + gap));
        for y in 0..fh {
            for x in 0..fw {
                let mut acc = [0u32; 4];
                for dy in 0..k {
                    for dx in 0..k {
                        let c = f.pixels[(y * k + dy) * f.size[0] + x * k + dx].to_array();
                        for j in 0..4 {
                            acc[j] += c[j] as u32;
                        }
                    }
                }
                let n = (k * k) as u32;
                out.pixels[(oy + y) * w + ox + x] = Color32::from_rgba_premultiplied((acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, 255);
            }
        }
    }
    out
}
