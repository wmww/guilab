//! Drives the editor in the demo host headless and writes screenshots of each interaction
//! state, in both themes: `target/debug/examples/shots [OUT_DIR] [FILTER]` (built by
//! `cargo test --workspace`). Default OUT_DIR: `target/editor-shots`.

use egui::{Color32, Key, Modifiers, Pos2, Theme, vec2};
use guilab_editor::Tool;
use guilab_editor::demo::{DemoHost, sample_raster, sample_svg};
use guilab_image::Color;
use guilab_kit::headless::{Headless, ev};

struct Driver {
    h: Headless,
    host: DemoHost,
    dir: std::path::PathBuf,
    theme: &'static str,
    pointer: Pos2,
    cursor: egui::CursorIcon,
}

impl Driver {
    fn new(dir: &std::path::Path, theme: Theme, ppp: f32) -> Driver {
        let h = Headless::new(vec2(1000.0, 680.0), ppp);
        guilab_kit::theme::install(&h.ctx);
        h.ctx.set_theme(theme);
        let mut d = Driver {
            h,
            host: DemoHost::new([1600, 1000], 2.0),
            dir: dir.into(),
            theme: if theme == Theme::Dark { "dark" } else { "light" },
            pointer: Pos2::ZERO,
            cursor: egui::CursorIcon::Default,
        };
        // Fonts and textures settle over the first frames.
        d.run(vec![]);
        d.run(vec![]);
        d
    }

    fn run(&mut self, events: Vec<egui::Event>) {
        for e in &events {
            if let egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. } = e {
                self.pointer = *p;
            }
        }
        let host = &mut self.host;
        let out = self.h.run(events, 0.05, |ui| host.ui(ui));
        self.cursor = out.platform_output.cursor_icon;
    }

    fn mv(&mut self, p: Pos2) {
        self.run(vec![ev::moved(p)]);
    }

    /// Canvas px to screen.
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.host.xf.apply(guilab_image::pt(x, y))
    }

    fn drag(&mut self, from: Pos2, to: Pos2, release: bool) {
        self.run(vec![ev::moved(from)]);
        self.run(vec![ev::press(from)]);
        for i in 1..=8 {
            self.run(vec![ev::moved(from + (to - from) * (i as f32 / 8.0))]);
        }
        if release {
            self.run(vec![ev::release(to)]);
        }
    }

    fn tool(&mut self, t: Tool) {
        self.host.editor.tool = t;
        self.run(vec![]);
    }

    fn snap(&mut self, name: &str) {
        let host = &mut self.host;
        let (p, cursor) = (self.pointer, self.cursor);
        let out = self.h.run(vec![], 0.05, |ui| {
            host.ui(ui);
            // The real cursor isn't part of a frame: mark the pointer and name the cursor.
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::new("pointer")));
            let m = Color32::from_rgb(255, 0, 255);
            painter.line_segment([p - vec2(7.0, 0.0), p + vec2(7.0, 0.0)], (1.0, m));
            painter.line_segment([p - vec2(0.0, 7.0), p + vec2(0.0, 7.0)], (1.0, m));
            let text = format!("{cursor:?}");
            let r = painter.text(p + vec2(10.0, 10.0), egui::Align2::LEFT_TOP, &text, egui::FontId::monospace(11.0), Color32::WHITE);
            painter.rect_filled(r.expand(2.0), 2.0, Color32::from_black_alpha(200));
            painter.text(p + vec2(10.0, 10.0), egui::Align2::LEFT_TOP, text, egui::FontId::monospace(11.0), Color32::WHITE);
        });
        let img = self.h.render(&out, Color32::BLACK);
        let path = self.dir.join(format!("{name}-{}.png", self.theme));
        write_png(&path, &img);
        println!("{}", path.display());
    }
}

fn write_png(path: &std::path::Path, img: &egui::ColorImage) {
    let f = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().unwrap();
    let px: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    wr.write_image_data(&px).unwrap();
}

/// A doc with one of each mark.
fn draw_marks(d: &mut Driver) {
    d.tool(Tool::Box);
    d.drag(d.at(500.0, 180.0), d.at(900.0, 300.0), true);
    d.host.editor.color = Color::BLUE;
    d.tool(Tool::Ellipse);
    d.drag(d.at(1000.0, 600.0), d.at(1300.0, 760.0), true);
    d.host.editor.color = Color::YELLOW;
    d.tool(Tool::Arrow);
    d.drag(d.at(1250.0, 200.0), d.at(920.0, 250.0), true);
    d.host.editor.color = Color::GREEN;
    d.tool(Tool::Line);
    d.drag(d.at(500.0, 400.0), d.at(900.0, 400.0), true);
    d.host.editor.color = Color::RED;
    d.tool(Tool::Pen);
    let pts: Vec<Pos2> = (0..30).map(|i| {
        let a = i as f32 / 29.0 * 5.0;
        d.at(600.0 + a * 60.0, 700.0 + (a * 2.2).sin() * 60.0)
    }).collect();
    d.run(vec![ev::moved(pts[0])]);
    d.run(vec![ev::press(pts[0])]);
    for p in &pts[1..] {
        d.run(vec![ev::moved(*p)]);
    }
    d.run(vec![ev::release(*pts.last().unwrap())]);
    d.tool(Tool::Text);
    d.run(vec![ev::moved(d.at(1000.0, 420.0))]);
    d.run(vec![ev::press(d.at(1000.0, 420.0))]);
    d.run(vec![ev::release(d.at(1000.0, 420.0))]);
    d.run(vec![]);
    d.run(vec![ev::text("Spacing here should be 16px, not 30")]);
    d.run(vec![ev::key(Key::Enter, Modifiers::NONE)]);
}

fn scenarios(d: &mut Driver, filter: &str) {
    let want = |n: &str| filter.is_empty() || n.contains(filter);
    if want("live") {
        d.tool(Tool::Pen);
        d.mv(d.at(700.0, 500.0));
        d.snap("01-live-pen");
    }
    draw_marks(d);
    if want("marks") {
        d.tool(Tool::Arrow);
        d.drag(d.at(300.0, 900.0), d.at(480.0, 760.0), false);
        d.snap("02-marks-drawing-arrow");
        d.run(vec![ev::release(d.at(480.0, 760.0))]);
        d.host.undo();
    }
    if want("move") {
        d.tool(Tool::Move);
        d.mv(d.at(700.0, 150.0));
        d.snap("03-move-empty");
        d.mv(d.at(700.0, 183.0));
        d.snap("04-move-hover-box");
        d.mv(d.at(900.0, 300.0));
        d.snap("05-move-hover-key");
        d.drag(d.at(700.0, 403.0), d.at(760.0, 470.0), false);
        d.snap("06-move-dragging-line");
        d.run(vec![ev::release(d.at(760.0, 470.0))]);
        d.drag(d.at(1300.0, 760.0), d.at(1400.0, 860.0), false);
        d.snap("07-move-reshape-ellipse");
        d.run(vec![ev::release(d.at(1400.0, 860.0))]);
        d.mv(d.at(1100.0, 432.0));
        d.snap("08-move-hover-text");
    }
    if want("text") {
        d.tool(Tool::Text);
        d.run(vec![ev::moved(d.at(300.0, 860.0)), ev::press(d.at(300.0, 860.0))]);
        d.run(vec![ev::release(d.at(300.0, 860.0))]);
        d.run(vec![]);
        d.run(vec![ev::text("A new note that is long enough to wrap onto a second line")]);
        d.snap("09-text-typing");
        d.run(vec![ev::key(Key::Escape, Modifiers::NONE)]);
        d.tool(Tool::Move);
        let p = d.at(1100.0, 432.0);
        d.run(vec![ev::moved(p), ev::press(p), ev::release(p)]);
        d.run(vec![ev::press(p), ev::release(p)]);
        d.run(vec![]);
        d.snap("10-text-editing-existing");
        d.run(vec![ev::key(Key::Escape, Modifiers::NONE)]);
    }
    if want("insert") {
        d.host.insert(sample_raster(300, 200));
        d.host.insert(sample_svg());
        d.tool(Tool::Move);
        // Move the SVG off the raster.
        d.drag(d.at(800.0, 500.0), d.at(800.0, 300.0), true);
        d.mv(d.at(800.0, 520.0));
        d.snap("11-insert-hover-picture");
        let c = d.host.xf.apply(guilab_image::pt(950.0, 600.0));
        d.drag(c, c + vec2(60.0, 40.0), false);
        d.snap("12-insert-resize-picture");
        d.run(vec![ev::release(c + vec2(60.0, 40.0))]);
    }
    if want("crop") {
        d.tool(Tool::Crop);
        d.run(vec![]);
        d.mv(d.at(800.0, 500.0));
        d.snap("13-crop-tool");
        d.drag(d.at(400.0, 120.0), d.at(1450.0, 900.0), false);
        d.snap("14-crop-dragging");
        d.run(vec![ev::release(d.at(1450.0, 900.0))]);
        d.run(vec![]);
        d.host.notice = Some("A host toast stacks above the editor's".into());
        for _ in 0..4 {
            d.run(vec![]);
        }
        d.snap("15-crop-done");
        d.host.notice = None;
        d.tool(Tool::Move);
        d.run(vec![]);
        d.mv(d.at(800.0, 500.0));
        d.snap("16-cropped-view");
    }
    if want("zoom") {
        d.tool(Tool::Move);
        let p = d.at(900.0, 300.0);
        for _ in 0..6 {
            d.run(vec![ev::moved(p), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, 60.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE }]);
        }
        d.mv(p);
        d.snap("17-zoomed");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "target/editor-shots".into()));
    let filter = args.get(1).cloned().unwrap_or_default();
    std::fs::create_dir_all(&dir).unwrap();
    for theme in [Theme::Dark, Theme::Light] {
        let mut d = Driver::new(&dir, theme, 1.0);
        scenarios(&mut d, &filter);
    }
    // HiDPI: crispness of marks, text and key points at 2x.
    if filter.is_empty() || filter == "hidpi" {
        let mut d = Driver::new(&dir, Theme::Dark, 2.0);
        d.theme = "dark-2x";
        draw_marks(&mut d);
        d.tool(Tool::Move);
        d.mv(d.at(700.0, 183.0));
        d.snap("18-hidpi");
    }
}
