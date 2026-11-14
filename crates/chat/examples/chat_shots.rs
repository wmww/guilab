//! Drives the chat in the demo host headless and writes screenshots of each state, in both
//! themes: `target/debug/examples/chat_shots [OUT_DIR] [FILTER]` (built by `cargo test
//! --workspace`). Default OUT_DIR: `target/chat-shots`.

use egui::{Color32, Key, Modifiers, Pos2, Theme, vec2};
use guilab_chat::SlotId;
use guilab_chat::demo::DemoHost;
use guilab_kit::headless::{Headless, ev};

struct Driver {
    h: Headless,
    host: DemoHost,
    dir: std::path::PathBuf,
    theme: String,
    pointer: Pos2,
    cursor: egui::CursorIcon,
}

impl Driver {
    fn new(dir: &std::path::Path, theme: Theme, ppp: f32) -> Driver {
        let h = Headless::new(vec2(460.0, 760.0), ppp);
        guilab_kit::theme::install(&h.ctx);
        h.ctx.set_theme(theme);
        let mut d = Driver {
            h,
            host: DemoHost::new(),
            dir: dir.into(),
            theme: if theme == Theme::Dark { "dark".into() } else { "light".into() },
            pointer: Pos2::new(-10.0, -10.0),
            cursor: egui::CursorIcon::Default,
        };
        d.settle();
        d
    }

    fn run_dt(&mut self, events: Vec<egui::Event>, dt: f64) {
        for e in &events {
            if let egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. } = e {
                self.pointer = *p;
            }
        }
        let host = &mut self.host;
        let out = self.h.run(events, dt, |ui| ui_frame(ui, host));
        self.cursor = out.platform_output.cursor_icon;
    }

    fn run(&mut self, events: Vec<egui::Event>) {
        self.run_dt(events, 0.02);
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            self.run_dt(vec![], 0.4);
        }
    }

    fn mv(&mut self, p: Pos2) {
        self.run(vec![ev::moved(p)]);
        self.settle();
    }

    fn click(&mut self, p: Pos2) {
        self.run(vec![ev::moved(p)]);
        self.run(vec![ev::press(p), ev::release(p)]);
        self.settle();
    }

    fn snap(&mut self, name: &str) {
        let host = &mut self.host;
        let (p, cursor) = (self.pointer, self.cursor);
        let out = self.h.run(vec![], 0.0, |ui| {
            ui_frame(ui, host);
            // The real cursor isn't part of a frame: mark the pointer and name the cursor.
            if p.x >= 0.0 {
                let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::new("pointer")));
                let m = Color32::from_rgb(255, 0, 255);
                painter.line_segment([p - vec2(7.0, 0.0), p + vec2(7.0, 0.0)], (1.0, m));
                painter.line_segment([p - vec2(0.0, 7.0), p + vec2(0.0, 7.0)], (1.0, m));
                let text = format!("{cursor:?}");
                let r = painter.text(p + vec2(10.0, 10.0), egui::Align2::LEFT_TOP, &text, egui::FontId::monospace(11.0), Color32::WHITE);
                painter.rect_filled(r.expand(2.0), 2.0, Color32::from_black_alpha(200));
                painter.text(p + vec2(10.0, 10.0), egui::Align2::LEFT_TOP, text, egui::FontId::monospace(11.0), Color32::WHITE);
            }
        });
        let img = self.h.render(&out, Color32::BLACK);
        let path = self.dir.join(format!("{name}-{}.png", self.theme));
        write_png(&path, &img);
        println!("{}", path.display());
    }

    fn slot(&self, slot: SlotId, key: u32) -> egui::Rect {
        self.host.last.slot(slot, &key).expect("slot")
    }

    fn feed_slot(&self, item_index: usize, att: usize) -> (SlotId, u32) {
        let (id, it) = self.host.chat.items().nth(item_index).unwrap();
        (SlotId::Feed { item: id, index: att as u16 }, it.attachments[att])
    }
}

/// The chat as a side panel would show it.
fn ui_frame(ui: &mut egui::Ui, host: &mut DemoHost) {
    let t = guilab_kit::Theme::get(ui.ctx());
    egui::CentralPanel::default().frame(egui::Frame::new().fill(t.colors.panel)).show(ui, |ui| host.ui(ui));
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

fn scenarios(d: &mut Driver, filter: &str) {
    let want = |n: &str| filter.is_empty() || n.contains(filter);
    if want("empty") {
        d.snap("01-empty");
    }
    d.host.sample();
    d.settle();
    if want("feed") {
        d.snap("02-feed-working");
    }
    if want("composer") {
        d.host.attach(800, 500, 1);
        d.host.attach(300, 600, 3);
        d.settle();
        let c = d.host.last.composer;
        d.click(c.left_bottom() + vec2(30.0, -16.0));
        d.run(vec![ev::text("The dialog's buttons are misaligned, see the marks. Also the title is cut off on the right.")]);
        d.settle();
        d.snap("03-composer-typing");
        let k = d.host.pending[0];
        let r = d.slot(SlotId::Composer, k);
        d.mv(r.center());
        d.snap("04-composer-hover");
        let r = d.slot(SlotId::Composer, k);
        d.mv(r.right_top() + vec2(-12.0, 12.0));
        d.snap("05-composer-hover-x");
        d.click(d.pointer);
        d.run_dt(vec![], 0.07);
        d.snap("06-composer-removing");
        d.settle();
        d.run(vec![ev::moved(Pos2::new(200.0, 300.0))]);
        d.settle();
        d.h.ctx.memory_mut(|m| m.surrender_focus(d.host.chat.text_id()));
        d.run(vec![ev::key(Key::Z, Modifiers::COMMAND)]);
        d.settle();
        d.snap("07-composer-undone");
    }
    if want("open") {
        // A past image open: its feed slot is empty and outlined.
        let (s, k) = d.feed_slot(7, 0);
        d.run(vec![ev::moved(Pos2::new(200.0, 300.0))]);
        d.settle();
        let r = d.slot(s, k);
        d.click(r.center());
        d.mv(Pos2::new(440.0, 100.0));
        d.snap("08-open-past");
        // First edit: a pending copy appears (open), the feed slot fades back.
        d.host.edit_open();
        for _ in 0..4 {
            d.run_dt(vec![], 0.02);
        }
        d.snap("09-copy-on-edit-mid");
        d.settle();
        d.snap("10-copy-on-edit");
    }
    if want("long") {
        let c = d.host.last.composer;
        d.click(c.left_bottom() + vec2(30.0, -16.0));
        for i in 0..12 {
            d.run(vec![ev::text(&format!("line {i} of a long message")), ev::key(Key::Enter, Modifiers::SHIFT)]);
        }
        d.settle();
        d.snap("14-long-draft");
        d.h.ctx.memory_mut(|m| m.surrender_focus(d.host.chat.text_id()));
        d.host.chat.draft_mut().clear();
        d.settle();
    }
    if want("faint") {
        let (s, k) = d.feed_slot(4, 0);
        // Scroll it into view.
        d.run(vec![ev::moved(Pos2::new(200.0, 300.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, 5000.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE }]);
        d.settle();
        let r = d.slot(s, k);
        d.mv(r.center());
        d.snap("11-faint-hover");
        d.mv(Pos2::new(440.0, 300.0));
        d.snap("12-scrolled-up");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "target/chat-shots".into()));
    let filter = args.get(1).cloned().unwrap_or_default();
    std::fs::create_dir_all(&dir).unwrap();
    for theme in [Theme::Dark, Theme::Light] {
        let mut d = Driver::new(&dir, theme, 1.0);
        scenarios(&mut d, &filter);
    }
    if filter.is_empty() || filter == "hidpi" {
        let mut d = Driver::new(&dir, Theme::Dark, 2.0);
        d.theme = "dark-2x".into();
        d.host.sample();
        d.host.attach(800, 500, 1);
        d.settle();
        d.snap("13-hidpi");
    }
}
