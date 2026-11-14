//! Still frames of the whole viewer UI in its interaction states (menus, hover, tooltips,
//! toasts, tabs, title bar, editor tools, chat), painted headless in both themes at 1x and
//! 2x: `target/debug/examples/ui_shots [DIR] [FILTER]` (default `target/ui-shots`). The
//! pointer is marked with a crosshair and the cursor's name.

use egui::{Event, Key, Modifiers, Pos2, pos2, vec2};
use guilab_chat::{Item, SlotId, Status};
use guilab_editor::Tool;
use guilab_viewer_ui::demo::{Demo, press};
use guilab_viewer_ui::input::HostInput;
use guilab_viewer_ui::{ChatPlace, workspace::Att};

/// Toolbar button centers (one session, no tab bar).
const TOOL_X: f32 = 20.0;
fn tool_y(i: usize) -> f32 {
    42.0 + 32.0 * i as f32
}

struct Run {
    d: Demo,
}

impl Run {
    fn new(dark: bool, ppp: f32) -> Run {
        let mut d = Demo::new(vec2(1000.0, 700.0), ppp, [1200, 750], dark);
        d.mark_pointer = true;
        d.idle(3);
        Run { d }
    }

    fn ev(&mut self, evs: Vec<Event>) {
        self.d.frame(evs.into_iter().map(HostInput::Egui).collect(), 1.0 / 60.0);
    }

    fn mv(&mut self, p: Pos2) {
        self.ev(vec![Event::PointerMoved(p)]);
    }

    fn click(&mut self, p: Pos2) {
        self.mv(p);
        self.ev(vec![press(p, true), press(p, false)]);
    }

    /// Holds the pointer still long enough for tooltips.
    fn rest(&mut self) {
        for _ in 0..4 {
            self.d.frame(vec![], 0.3);
        }
    }

    fn tool(&mut self, t: Tool) {
        self.d.ui.workspace_mut().unwrap().editor.tool = t;
        self.d.idle(2);
    }

    fn drag(&mut self, a: Pos2, b: Pos2) {
        self.mv(a);
        self.ev(vec![press(a, true)]);
        for i in 1..=6 {
            self.mv(a + (b - a) * (i as f32 / 6.0));
        }
        self.ev(vec![press(b, false)]);
        self.d.idle(30);
    }

    fn typing(&mut self, text: &str) {
        self.ev(vec![Event::Text(text.into())]);
    }

    /// A feed with each kind of item.
    fn feed(&mut self) {
        let shot = self.d.screenshot().unwrap();
        self.d.push(Item::me("11:58", "The save button is hard to find. Can you make it stand out?"));
        self.d.push(Item::activity("11:58", "work \"making the save button stand out\""));
        self.d.push(Item::activity("11:59", "run -- cargo run --example editor"));
        self.d.push(Item::activity("11:59", "screenshot → s7kq").with([shot]).faint());
        self.d.push(Item::them("Agent", "12:00", "Here's the dialog now: the button is blue and moved to the right.").with([shot]));
        self.d.idle(30);
    }

    fn feed_slot(&self) -> Pos2 {
        let ws = self.d.ui.workspace().unwrap();
        ws.slots.iter().filter(|(s, _, _)| matches!(s, SlotId::Feed { .. })).next_back().map(|(_, _, r)| r.center()).expect("a feed slot")
    }

    fn draft_slot(&self) -> Pos2 {
        let ws = self.d.ui.workspace().unwrap();
        let att = Att::Draft(ws.pending[0].key);
        ws.slot_rect(SlotId::Composer, att).expect("composer slot").center()
    }
}

fn scene(name: &str, dark: bool, ppp: f32) -> Option<Run> {
    let mut r = Run::new(dark, ppp);
    match name {
        // A fresh session: the empty chat, "Use the app", the pointer over the app.
        "empty" => r.mv(pos2(500.0, 250.0)),
        // A conversation in progress: the agent works, a drawing is open and pending.
        "chat" => {
            r.feed();
            r.d.agent = Some(Status { text: "making the save button stand out".into(), elapsed: 75, note: None });
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0));
            r.mv(pos2(TOOL_X, tool_y(4)));
        }
        "menu-file" => {
            r.click(pos2(20.0, 12.0));
            r.mv(pos2(60.0, 40.0));
        }
        "menu-edit" => {
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0));
            r.click(pos2(62.0, 12.0));
            r.mv(pos2(100.0, 40.0));
        }
        "menu-view" => {
            r.click(pos2(108.0, 12.0));
            r.mv(pos2(150.0, 72.0));
        }
        // A tool button's hover and tooltip.
        "tooltip" => {
            r.mv(pos2(TOOL_X, tool_y(2)));
            r.rest();
        }
        // Several sessions, the agent holding this one: tabs and the lock toast.
        "tabs" => {
            r.d.extra = vec!["checkout-flow".into(), "settings".into()];
            r.d.locked = true;
            r.d.idle(2);
            r.click(pos2(500.0, 250.0));
            r.d.idle(2);
            r.mv(pos2(700.0, 50.0));
        }
        // Client-side decorations: the menu bar is the title bar.
        "csd" => {
            r.d.ui.csd = true;
            r.d.idle(2);
            r.mv(pos2(985.0, 12.0));
        }
        // The move tool over a mark: glow and key points.
        "move" => {
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0));
            r.tool(Tool::Arrow);
            r.drag(pos2(300.0, 330.0), pos2(390.0, 220.0));
            r.tool(Tool::Move);
            r.mv(pos2(500.0, 151.0));
        }
        "crop" => {
            r.tool(Tool::Box);
            r.drag(pos2(400.0, 150.0), pos2(600.0, 260.0));
            r.tool(Tool::Crop);
            r.drag(pos2(300.0, 100.0), pos2(700.0, 350.0));
            r.mv(pos2(700.0, 200.0));
        }
        "text" => {
            r.tool(Tool::Text);
            r.click(pos2(450.0, 200.0));
            r.d.idle(3);
            r.typing("Make this blue");
            r.d.idle(3);
        }
        "side" => {
            r.d.ui.chat_place = ChatPlace::Right;
            r.feed();
            r.tool(Tool::Arrow);
            r.drag(pos2(200.0, 400.0), pos2(300.0, 300.0));
            r.tool(Tool::Interact);
            r.d.idle(30);
            let p = r.draft_slot();
            r.mv(p);
        }
        // A feed thumbnail hovered: it grows.
        "slot-hover" => {
            r.feed();
            let p = r.feed_slot();
            r.mv(p);
            r.d.idle(20);
        }
        // The composer focused with text, the send button hovered.
        "composer" => {
            r.feed();
            r.click(pos2(500.0, 672.0));
            r.typing("Also the cancel button");
            r.mv(pos2(968.0, 672.0));
            r.rest();
        }
        // Resize handles around a small app, one hovered.
        "handles" => {
            r.d.ui.chat_place = ChatPlace::Off;
            r.d.idle(2);
            r.d = {
                let mut d = Demo::new(vec2(1000.0, 700.0), ppp, [600, 400], dark);
                d.mark_pointer = true;
                d.ui.chat_place = ChatPlace::Off;
                d.idle(3);
                d
            };
            r.mv(pos2(820.0, 362.0));
            r.d.idle(2);
        }
        // The feed scrolled up: the jump-to-latest button.
        "scrolled" => {
            r.feed();
            for i in 0..30 {
                r.d.push(Item::activity("12:01", format!("input click {} 300", 100 + i)));
            }
            r.d.idle(5);
            r.mv(pos2(500.0, 560.0));
            for _ in 0..6 {
                r.ev(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, 3.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE }]);
            }
            r.d.push(Item::them("Agent", "12:02", "Done."));
            r.d.idle(5);
        }
        // Keyboard: Esc closes a menu opened by a click, arrows move the highlight.
        "menu-keys" => {
            r.click(pos2(108.0, 12.0));
            r.ev(vec![Event::Key { key: Key::ArrowDown, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
            r.ev(vec![Event::Key { key: Key::ArrowDown, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
            r.d.idle(2);
        }
        _ => return None,
    }
    Some(r)
}

const ALL: [&str; 17] = ["empty", "chat", "menu-file", "menu-edit", "menu-view", "menu-keys", "tooltip", "tabs", "csd", "move", "crop", "text", "side", "slot-hover", "composer", "handles", "scrolled"];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).cloned().unwrap_or("target/ui-shots".into()));
    let filter = args.get(2).cloned().unwrap_or_default();
    std::fs::create_dir_all(&dir).unwrap();
    for name in ALL.iter().filter(|n| n.contains(&filter)) {
        for ppp in [1.0, 2.0] {
            for dark in [true, false] {
                let mut r = scene(name, dark, ppp).unwrap();
                // Popups fade in, thumbnails grow: let it settle.
                r.d.idle(10);
                let out = r.d.frame(vec![], 0.0);
                let img = r.d.render(&out);
                let path = dir.join(format!("{name}-{}{}.png", if dark { "dark" } else { "light" }, if ppp > 1.0 { "@2x" } else { "" }));
                std::fs::write(&path, guilab_viewer_ui::demo::png(&img)).unwrap();
                println!("{}", path.display());
            }
        }
    }
}
