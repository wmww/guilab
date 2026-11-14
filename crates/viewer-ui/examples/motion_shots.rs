//! Frame sequences of the viewer's motion (flights, glow, slot fades), painted headless:
//! `target/debug/examples/motion_shots [DIR] [FILTER]` (default `target/motion-shots`).
//! Each scenario writes a contact sheet per theme: every `STEP`th frame at 60 Hz.

use egui::{Event, Pos2, pos2, vec2};
use guilab_chat::{Item, SlotId};
use guilab_editor::Tool;
use guilab_viewer_ui::demo::{Demo, contact_sheet, press};
use guilab_viewer_ui::input::HostInput;
use guilab_viewer_ui::workspace::Att;

const STEP: usize = 2;
const FRAMES: usize = 18;

struct Run {
    d: Demo,
    frames: Vec<egui::ColorImage>,
}

impl Run {
    fn new(dark: bool) -> Run {
        let mut d = Demo::new(vec2(1000.0, 700.0), 1.0, [1200, 750], dark);
        d.idle(3);
        Run { d, frames: Vec::new() }
    }

    fn ev(&mut self, evs: Vec<Event>) {
        self.d.frame(evs.into_iter().map(HostInput::Egui).collect(), 1.0 / 60.0);
    }

    fn tool(&mut self, t: Tool) {
        self.d.ui.workspace_mut().unwrap().editor.tool = t;
    }

    /// Draws a box by dragging from `a` to `b`.
    fn drag(&mut self, a: Pos2, b: Pos2, record: bool) {
        self.ev(vec![Event::PointerMoved(a)]);
        self.ev(vec![press(a, true)]);
        for i in 1..=6 {
            let p = a + (b - a) * (i as f32 / 6.0);
            self.ev(vec![Event::PointerMoved(p)]);
            if record {
                self.snap();
            }
        }
        self.ev(vec![press(b, false)]);
    }

    fn slot(&self, slot: SlotId, att: Att) -> Pos2 {
        self.d.ui.workspace().unwrap().slot_rect(slot, att).expect("slot laid out").center()
    }

    fn snap(&mut self) {
        let out = self.d.frame(vec![], 0.0);
        let img = self.d.render(&out);
        self.frames.push(img);
    }

    /// Frames after an action: every STEP-th of FRAMES at 60 Hz.
    fn record(&mut self, evs: Vec<Event>) {
        self.ev(evs);
        self.snap();
        for i in 1..FRAMES {
            self.d.frame(vec![], 1.0 / 60.0);
            if i % STEP == 0 {
                self.snap();
            }
        }
    }

    fn click_rec(&mut self, p: Pos2) {
        self.ev(vec![Event::PointerMoved(p)]);
        self.record(vec![press(p, true), press(p, false)]);
    }

    fn draft(&self, i: usize) -> Att {
        Att::Draft(self.d.ui.workspace().unwrap().pending[i].key)
    }
}

fn scenario(name: &str, dark: bool) -> Option<Run> {
    let mut r = Run::new(dark);
    match name {
        // Drawing on the live app: the draft's slot grows in, the glow fades in.
        "draw" => {
            r.tool(Tool::Box);
            r.snap();
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0), true);
            r.record(vec![]);
        }
        // Closing the editor: the image flies home into its slot.
        "close" => {
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0), false);
            r.d.idle(30);
            r.snap();
            r.tool(Tool::Interact);
            r.record(vec![]);
        }
        // Opening a pending image: it flies out of its slot.
        "open" => {
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0), false);
            r.tool(Tool::Interact);
            r.d.idle(30);
            r.snap();
            let p = r.slot(SlotId::Composer, r.draft(0));
            r.click_rec(p);
        }
        // Switching between two pending images: one flies home, the other out.
        "switch" => {
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0), false);
            r.tool(Tool::Interact);
            r.d.idle(30);
            r.tool(Tool::Arrow);
            r.drag(pos2(300.0, 300.0), pos2(500.0, 200.0), false);
            r.d.idle(30);
            r.snap();
            let p = r.slot(SlotId::Composer, r.draft(0));
            r.click_rec(p);
        }
        // A past image: flies out of the feed; its first edit fades the feed slot back and
        // the copy grows into the composer, the glow sliding over.
        "past" => {
            let id = r.d.screenshot().unwrap();
            r.d.push(Item::them("Agent", "12:00", "Here's the dialog now.").with([id]));
            r.d.idle(30);
            r.snap();
            let feed = r.d.ui.workspace().unwrap().slots.iter().find(|(s, _, _)| matches!(s, SlotId::Feed { .. })).map(|(s, a, _)| (*s, *a)).unwrap();
            let p = r.slot(feed.0, feed.1);
            r.click_rec(p);
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0), false);
            r.record(vec![]);
        }
        // The glow follows the open image as it zooms and pans.
        "glow" => {
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0), false);
            r.d.idle(30);
            r.snap();
            let at = pos2(700.0, 200.0);
            r.ev(vec![Event::PointerMoved(at)]);
            for _ in 0..3 {
                r.ev(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, -2.0), phase: egui::TouchPhase::Move, modifiers: Default::default() }]);
                r.snap();
            }
        }
        // Chat on the right: same moves sideways.
        "side" => {
            r.d.ui.chat_place = guilab_viewer_ui::ChatPlace::Right;
            r.d.idle(3);
            r.tool(Tool::Box);
            r.drag(pos2(200.0, 300.0), pos2(400.0, 400.0), false);
            r.d.idle(30);
            r.snap();
            r.tool(Tool::Interact);
            r.record(vec![]);
            let p = r.slot(SlotId::Composer, r.draft(0));
            r.click_rec(p);
        }
        // A past image whose slot scrolled out of the feed: the feed's edge glows, and
        // closing flies to that edge, fading.
        "scrolled" => {
            let id = r.d.screenshot().unwrap();
            r.d.push(Item::them("Agent", "12:00", "Here's the dialog now.").with([id]));
            r.d.idle(30);
            let feed = r.d.ui.workspace().unwrap().slots.iter().find(|(s, _, _)| matches!(s, SlotId::Feed { .. })).map(|(s, a, _)| (*s, *a)).unwrap();
            let p = r.slot(feed.0, feed.1);
            r.ev(vec![Event::PointerMoved(p)]);
            r.ev(vec![press(p, true), press(p, false)]);
            for i in 0..8 {
                r.d.push(Item::activity("12:01", format!("guilab input click {i}")));
            }
            r.d.idle(30);
            r.snap();
            r.tool(Tool::Interact);
            r.record(vec![]);
        }
        _ => return None,
    }
    Some(r)
}

const ALL: [&str; 8] = ["draw", "close", "open", "switch", "past", "glow", "side", "scrolled"];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).cloned().unwrap_or("target/motion-shots".into()));
    let filter = args.get(2).cloned().unwrap_or_default();
    std::fs::create_dir_all(&dir).unwrap();
    for name in ALL.iter().filter(|n| n.contains(&filter)) {
        for dark in [true, false] {
            let r = scenario(name, dark).unwrap();
            let sheet = contact_sheet(&r.frames, 4, 2);
            let path = dir.join(format!("{name}-{}.png", if dark { "dark" } else { "light" }));
            std::fs::write(&path, guilab_viewer_ui::demo::png(&sheet)).unwrap();
            // Every frame at full size too, for a closer look.
            let fdir = dir.join(format!("{name}-{}", if dark { "dark" } else { "light" }));
            std::fs::create_dir_all(&fdir).unwrap();
            for (i, f) in r.frames.iter().enumerate() {
                std::fs::write(fdir.join(format!("{i:02}.png")), guilab_viewer_ui::demo::png(f)).unwrap();
            }
            println!("{}", path.display());
        }
    }
}
