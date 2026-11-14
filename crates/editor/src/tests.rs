//! Interaction tests: the demo host in a headless egui context, driven by synthetic input.

use egui::{CursorIcon, Event, Key, Modifiers, Pos2, vec2};
use guilab_image::geom::Key as KeyPt;
use guilab_image::{Color, Edit, Element, IRect, Kind, Op, Rect, pt};
use guilab_kit::headless::{Headless, ev};

use crate::demo::{DemoHost, sample_svg};
use crate::{Hit, Tool};

struct T {
    h: Headless,
    host: DemoHost,
    cursor: CursorIcon,
}

impl T {
    /// A 600x400 scale-1 screenshot shown at 1:1.
    fn new() -> T {
        let h = Headless::new(vec2(1000.0, 700.0), 1.0);
        guilab_kit::theme::install(&h.ctx);
        let mut t = T { h, host: DemoHost::new([600, 400], 1.0), cursor: CursorIcon::Default };
        t.run(vec![]);
        t.run(vec![]);
        assert_eq!(t.host.xf.zoom, 1.0);
        t
    }

    fn run(&mut self, events: Vec<Event>) {
        let host = &mut self.host;
        let out = self.h.run(events, 0.1, |ui| host.ui(ui));
        self.cursor = out.platform_output.cursor_icon;
    }

    /// Canvas px to screen.
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.host.xf.apply(pt(x, y))
    }

    fn mv(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.run(vec![ev::moved(p)]);
    }

    fn drag_mods(&mut self, from: (f32, f32), to: (f32, f32), m: Modifiers) {
        let (a, b) = (self.at(from.0, from.1), self.at(to.0, to.1));
        self.run(vec![ev::moved(a)]);
        self.run(vec![ev::button(a, true, m)]);
        self.h.modifiers = m;
        self.run(vec![ev::moved(a + (b - a) * 0.5)]);
        self.run(vec![ev::moved(b)]);
        self.run(vec![ev::button(b, false, m)]);
        self.h.modifiers = Modifiers::NONE;
    }

    fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.drag_mods(from, to, Modifiers::NONE);
    }

    fn click(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.run(vec![ev::moved(p), ev::press(p), ev::release(p)]);
    }

    fn key(&mut self, k: Key, m: Modifiers) {
        self.run(vec![ev::key(k, m)]);
    }

    fn elements(&self) -> Vec<Element> {
        self.host.doc.as_ref().map(|d| d.body().elements.clone()).unwrap_or_default()
    }

    /// Adds marks straight into a fresh doc (a box, a line, a text), recorded for undo.
    fn with_marks(&mut self) {
        self.host.open();
        let d = self.host.doc.as_mut().unwrap();
        for el in [
            Element::new(Color::RED, Kind::Box(Rect::new(100.0, 100.0, 200.0, 100.0))),
            Element::new(Color::BLUE, Kind::Line(pt(100.0, 300.0), pt(300.0, 300.0))),
            Element::new(Color::RED, Kind::Text { at: pt(400.0, 100.0), size: 18.0, width: None, text: "hello".into() }),
        ] {
            let e = d.add_edit(el).unwrap();
            d.apply(&e);
        }
        self.run(vec![]);
    }
}

#[test]
fn draw_without_doc_begins_and_commits() {
    let mut t = T::new();
    t.host.editor.tool = Tool::Box;
    t.mv(100.0, 100.0);
    assert_eq!(t.cursor, CursorIcon::Crosshair);
    assert!(t.host.doc.is_none());
    t.drag((100.0, 100.0), (200.0, 150.0));
    let els = t.elements();
    assert_eq!(els.len(), 2, "base + box");
    assert_eq!(els[1].kind, Kind::Box(Rect::new(100.0, 100.0, 100.0, 50.0)));
    assert!(matches!(t.host.log.as_slice(), [e] if matches!(e.op(), Op::Add { .. })));

    // A stray click on a fresh live view makes no doc.
    let mut t = T::new();
    t.host.editor.tool = Tool::Pen;
    t.click(50.0, 50.0);
    assert!(t.host.doc.is_none(), "abandoned");
    assert!(t.host.log.is_empty());
}

#[test]
fn shift_constrains_shapes() {
    let mut t = T::new();
    t.host.open();
    t.host.editor.tool = Tool::Ellipse;
    t.drag_mods((100.0, 100.0), (180.0, 130.0), Modifiers::SHIFT);
    assert_eq!(t.elements()[1].kind, Kind::Ellipse(Rect::new(100.0, 100.0, 80.0, 80.0)), "a circle");
    t.host.editor.tool = Tool::Line;
    t.drag_mods((100.0, 300.0), (200.0, 310.0), Modifiers::SHIFT);
    let Kind::Line(a, b) = t.elements()[2].kind else { panic!() };
    assert_eq!(a, pt(100.0, 300.0));
    assert!((b.y - 300.0).abs() < 0.01, "snapped flat: {b:?}");
}

#[test]
fn hit_priorities() {
    let mut t = T::new();
    t.with_marks();
    t.host.editor.tool = Tool::Move;
    let d = t.host.doc.as_ref().unwrap();
    let k = d.keys().to_vec();
    // Empty space and the locked base: pan.
    t.mv(200.0, 150.0);
    assert_eq!(t.host.editor.hovered(), Hit::None, "inside the box isn't on it");
    assert_eq!(t.cursor, CursorIcon::Grab);
    // On the box's edge, and near it (thin lines are hard to hit).
    t.mv(200.0, 101.0);
    assert_eq!(t.host.editor.hovered(), Hit::Item(k[1]));
    assert_eq!(t.cursor, CursorIcon::AllScroll);
    t.mv(200.0, 100.0 - 1.5 - 5.5);
    assert_eq!(t.host.editor.hovered(), Hit::Item(k[1]), "within the slop");
    t.mv(200.0, 100.0 - 1.5 - 7.0);
    assert_eq!(t.host.editor.hovered(), Hit::None, "past the slop");
    // A key point wins over its item.
    t.mv(301.0, 201.0);
    assert_eq!(t.host.editor.hovered(), Hit::Key(k[1], KeyPt::Corner(2)));
    assert_eq!(t.cursor, CursorIcon::ResizeNwSe);
    t.mv(299.0, 99.0);
    assert_eq!(t.cursor, CursorIcon::ResizeNeSw);
    t.mv(100.0, 300.0);
    assert_eq!(t.host.editor.hovered(), Hit::Key(k[2], KeyPt::End(0)));
    // Topmost wins: a second box on top of the first's edge.
    let d = t.host.doc.as_mut().unwrap();
    let e = d.add_edit(Element::new(Color::GREEN, Kind::Box(Rect::new(150.0, 50.0, 100.0, 100.0)))).unwrap();
    d.apply(&e);
    let top = d.keys()[4];
    t.mv(150.0, 101.0);
    assert_eq!(t.host.editor.hovered(), Hit::Item(top), "the newer box covers this spot of both");
    // Text: anywhere in its backing.
    t.mv(410.0, 110.0);
    assert_eq!(t.host.editor.hovered(), Hit::Item(k[3]));
}

#[test]
fn move_reshape_pan_and_undo() {
    let mut t = T::new();
    t.with_marks();
    t.host.editor.tool = Tool::Move;
    let before = t.elements();
    // Move the line down by 50: one Replace at release.
    t.drag((200.0, 300.0), (200.0, 350.0));
    assert_eq!(t.elements()[2].kind, Kind::Line(pt(100.0, 350.0), pt(300.0, 350.0)));
    assert_eq!(t.host.log.len(), 1);
    assert!(matches!(t.host.log[0].op(), Op::Replace { .. }));
    // Reshape: the box's bottom-right corner, grabbed slightly off the point.
    t.drag((302.0, 202.0), (352.0, 252.0));
    assert_eq!(t.elements()[1].kind, Kind::Box(Rect::new(100.0, 100.0, 250.0, 150.0)), "no jump by the grab offset");
    // A line end.
    t.drag((300.0, 350.0), (320.0, 380.0));
    assert_eq!(t.elements()[2].kind, Kind::Line(pt(100.0, 350.0), pt(320.0, 380.0)));
    // Text corner: scales the font.
    let Kind::Text { size: s0, .. } = t.elements()[3].kind else { panic!() };
    let f = t.elements()[3].frame(1.0, t.host.editor.fonts());
    t.drag((f.max().x, f.max().y), (f.max().x + 40.0, f.max().y + f.size.h));
    let Kind::Text { size: s1, at, .. } = t.elements()[3].kind else { panic!() };
    assert!(s1 > s0 * 1.8, "{s0} -> {s1}");
    assert_eq!(at, pt(400.0, 100.0), "anchored at the opposite corner");
    // Pan on empty space: no edit, the view moves.
    let x0 = t.host.xf.offset;
    t.drag((500.0, 350.0), (450.0, 330.0));
    assert_eq!(t.host.xf.offset - x0, vec2(-50.0, -20.0));
    assert_eq!(t.host.log.len(), 4);
    // Undo everything back.
    for _ in 0..4 {
        t.key(Key::Z, Modifiers::COMMAND);
    }
    assert_eq!(t.elements(), before);
    t.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(t.elements()[2].kind, Kind::Line(pt(100.0, 350.0), pt(300.0, 350.0)), "redo");
}

#[test]
fn escape_and_undo_mid_drag() {
    let mut t = T::new();
    t.with_marks();
    t.host.editor.tool = Tool::Move;
    let before = t.elements();
    let (a, b) = (t.at(200.0, 300.0), t.at(200.0, 360.0));
    t.run(vec![ev::moved(a), ev::press(a)]);
    t.run(vec![ev::moved(b)]);
    assert_eq!(t.cursor, CursorIcon::AllScroll);
    t.key(Key::Escape, Modifiers::NONE);
    t.run(vec![ev::release(b)]);
    assert_eq!(t.elements(), before, "Esc cancels the drag");
    assert!(t.host.log.is_empty());

    // The element disappears mid-drag (host undo): the drag just ends.
    t.host.editor.tool = Tool::Box;
    t.drag((400.0, 250.0), (500.0, 350.0));
    t.host.editor.tool = Tool::Move;
    let (a, b) = (t.at(450.0, 250.0), t.at(450.0, 280.0));
    t.run(vec![ev::moved(a), ev::press(a)]);
    t.run(vec![ev::moved(b)]);
    t.host.undo();
    t.run(vec![ev::moved(b + vec2(5.0, 5.0))]);
    t.run(vec![ev::release(b)]);
    assert_eq!(t.elements(), before);
    assert!(!t.host.editor.busy());
}

#[test]
fn delete_hovered() {
    let mut t = T::new();
    t.with_marks();
    t.host.editor.tool = Tool::Move;
    t.mv(200.0, 300.0);
    t.key(Key::Delete, Modifiers::NONE);
    assert_eq!(t.elements().len(), 3);
    assert!(matches!(t.host.log[0].op(), Op::Remove { at: 2, .. }));
    // The base can't be deleted.
    t.mv(500.0, 350.0);
    t.key(Key::Backspace, Modifiers::NONE);
    assert_eq!(t.elements().len(), 3);
    t.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(t.elements().len(), 4, "undo brings it back");
}

#[test]
fn text_new_edit_and_clear() {
    let mut t = T::new();
    t.host.editor.tool = Tool::Text;
    t.click(100.0, 100.0);
    assert!(t.host.doc.is_some(), "text on the live view begins a doc");
    t.run(vec![]);
    t.run(vec![ev::text("hi there")]);
    t.key(Key::Enter, Modifiers::SHIFT);
    t.run(vec![ev::text("two")]);
    t.key(Key::Enter, Modifiers::NONE);
    let Kind::Text { text, width, size, .. } = &t.elements()[1].kind else { panic!("{:?}", t.elements()) };
    assert_eq!(text, "hi there\ntwo");
    assert_eq!(*width, None, "short text stores no width");
    assert_eq!(*size, 18.0);

    // Esc on new text: nothing, and the doc it began is dropped.
    let mut t2 = T::new();
    t2.host.editor.tool = Tool::Text;
    t2.click(100.0, 100.0);
    t2.run(vec![]);
    t2.run(vec![ev::text("nope")]);
    t2.key(Key::Escape, Modifiers::NONE);
    assert!(t2.host.doc.is_none() && t2.host.log.is_empty(), "{:?}", t2.host.log);

    // Double-click with Move edits in place: the word under the pointer is selected.
    t.host.editor.tool = Tool::Move;
    let Kind::Text { at, .. } = t.elements()[1].kind else { panic!() };
    let p = t.at(at.x + 5.0, at.y + 8.0);
    t.run(vec![ev::moved(p), ev::press(p), ev::release(p)]);
    t.run(vec![ev::press(p), ev::release(p)]);
    t.run(vec![]);
    t.run(vec![]);
    t.run(vec![ev::text("bye")]);
    t.key(Key::Enter, Modifiers::NONE);
    let Kind::Text { text, .. } = &t.elements()[1].kind else { panic!() };
    assert_eq!(text, "bye there\ntwo");
    assert!(matches!(t.host.log.last().map(|e| e.op()), Some(Op::Replace { .. })));

    // Long text wraps and stores its width.
    t.host.editor.tool = Tool::Text;
    t.click(100.0, 250.0);
    t.run(vec![]);
    t.run(vec![ev::text(&"word ".repeat(40))]);
    t.key(Key::Enter, Modifiers::NONE);
    let Kind::Text { width, at, .. } = t.elements()[2].kind.clone() else { panic!() };
    assert_eq!(width, Some(16.0 * 18.0), "wraps at 16 ems");
    assert_eq!(at.x, 100.0);

    // Clearing a text removes it.
    t.click(at.x + 5.0, at.y + 5.0);
    t.run(vec![]);
    t.run(vec![]);
    t.key(Key::A, Modifiers::COMMAND);
    t.key(Key::Backspace, Modifiers::NONE);
    t.key(Key::Enter, Modifiers::NONE);
    assert_eq!(t.elements().len(), 2);
    assert!(matches!(t.host.log.last().map(|e| e.op()), Some(Op::Remove { .. })));
}

#[test]
fn crop_and_show_all() {
    let mut t = T::new();
    t.with_marks();
    t.host.editor.tool = Tool::Crop;
    t.run(vec![]);
    t.mv(300.0, 200.0);
    assert_eq!(t.cursor, CursorIcon::Crosshair, "no crop yet: a drag inside makes one");
    t.drag((100.4, 50.0), (400.0, 300.6));
    assert_eq!(t.host.doc.as_ref().unwrap().body().crop, Some(IRect::new(100, 50, 300, 251)), "whole pixels");
    // Drag the left edge, then move the crop.
    t.mv(100.0, 200.0);
    assert_eq!(t.cursor, CursorIcon::ResizeHorizontal);
    t.drag((100.0, 200.0), (50.0, 200.0));
    assert_eq!(t.host.doc.as_ref().unwrap().body().crop, Some(IRect::new(50, 50, 350, 251)));
    t.drag((200.0, 200.0), (210.0, 1000.0));
    assert_eq!(t.host.doc.as_ref().unwrap().body().crop, Some(IRect::new(60, 149, 350, 251)), "kept inside the canvas");
    // Other tools show (and fit) the crop only.
    t.host.editor.tool = Tool::Move;
    t.run(vec![]);
    t.run(vec![]);
    let shown = t.host.last.shown.unwrap();
    assert_eq!(shown.size(), vec2(350.0, 251.0));
    t.host.editor.tool = Tool::Crop;
    t.run(vec![]);
    t.run(vec![]);
    // Show all, in the toast at the bottom of the view.
    let clip = t.h.ctx.content_rect();
    let mut found = None;
    for y in (clip.max.y as i32 - 80..clip.max.y as i32).step_by(2) {
        for x in (400..700).step_by(4) {
            let p = egui::pos2(x as f32, y as f32);
            if t.h.ctx.layer_id_at(p).is_some_and(|l| l.order == egui::Order::Middle) {
                found = Some(p);
            }
        }
    }
    assert!(found.is_some(), "the crop toast is up");
    for _ in 0..3 {
        t.key(Key::Z, Modifiers::COMMAND);
    }
    assert_eq!(t.host.doc.as_ref().unwrap().body().crop, None, "crops undo");
}

#[test]
fn insert_picture_moves_and_resizes_keeping_aspect() {
    let mut t = T::new();
    t.host.insert(sample_svg());
    let Kind::Svg { dst, .. } = t.elements()[1].kind else { panic!() };
    assert_eq!(dst, Rect::new(220.0, 172.0, 160.0, 56.0), "natural size, centered");
    t.host.editor.tool = Tool::Move;
    t.drag((300.0, 200.0), (310.0, 210.0));
    t.drag((390.0, 238.0), (470.0, 238.0));
    let Kind::Svg { dst, .. } = t.elements()[1].kind else { panic!() };
    assert_eq!(dst.min, pt(230.0, 182.0));
    assert_eq!(dst.size.w, 240.0);
    assert!((dst.size.h - 84.0).abs() < 0.01, "aspect kept: {dst:?}");
    t.drag_mods((470.0, 266.0), (470.0, 300.0), Modifiers::SHIFT);
    let Kind::Svg { dst, .. } = t.elements()[1].kind else { panic!() };
    assert_eq!((dst.size.w, dst.size.h), (240.0, 118.0), "Shift frees the aspect");
}

#[test]
fn tool_keys_and_cursors() {
    let mut t = T::new();
    t.host.open();
    t.run(vec![]);
    t.host.editor.tool = Tool::Move;
    for (k, tool) in [(Key::P, Tool::Pen), (Key::T, Tool::Text), (Key::C, Tool::Crop), (Key::V, Tool::Move)] {
        t.key(k, Modifiers::NONE);
        assert_eq!(t.host.editor.tool, tool);
    }
    for (tool, c) in [(Tool::Pen, CursorIcon::Crosshair), (Tool::Text, CursorIcon::Text), (Tool::Move, CursorIcon::Grab), (Tool::Crop, CursorIcon::Crosshair)] {
        t.host.editor.tool = tool;
        t.mv(300.0, 200.0);
        assert_eq!(t.cursor, c, "{tool:?}");
    }
    t.host.editor.tool = Tool::Interact;
    t.mv(300.0, 200.0);
    assert_eq!(t.host.last.cursor, None, "Interact leaves the cursor to the host");
}

/// The editor paints text where the CPU renderer does (same layout, same fonts).
#[test]
fn text_matches_cpu_render() {
    let mut t = T::new();
    t.host.open();
    let d = t.host.doc.as_mut().unwrap();
    let e = d.add_edit(Element::new(Color::BLACK, Kind::Text { at: pt(50.0, 50.0), size: 30.0, width: Some(200.0), text: "Wrapped words here and there".into() })).unwrap();
    d.apply(&e);
    t.run(vec![]);
    t.run(vec![]);
    let host = &mut t.host;
    let out = t.h.run(vec![], 0.1, |ui| host.ui(ui));
    let screen = t.h.render(&out, egui::Color32::BLACK);
    let body = t.host.doc.as_ref().unwrap().body().clone();
    let cpu = guilab_image::render(&body, &t.host.store, t.host.editor.fonts(), &Default::default()).unwrap();
    // Bounding box of black glyph pixels inside the text box, in canvas px, both ways.
    let dark = |c: [u8; 4]| c[0] < 60 && c[1] < 60 && c[2] < 60;
    let bbox = |f: &dyn Fn(u32, u32) -> [u8; 4]| {
        let (mut a, mut b) = ((u32::MAX, u32::MAX), (0, 0));
        for y in 40..200 {
            for x in 40..300 {
                if dark(f(x, y)) {
                    a = (a.0.min(x), a.1.min(y));
                    b = (b.0.max(x), b.1.max(y));
                }
            }
        }
        (a, b)
    };
    let off = t.host.xf.offset;
    let w = screen.size[0];
    let on_screen = bbox(&|x, y| screen.pixels[(y as f32 + off.y) as usize * w + (x as f32 + off.x) as usize].to_array());
    let on_cpu = bbox(&|x, y| cpu.pixel(x, y));
    let close = |a: u32, b: u32| a.abs_diff(b) <= 2;
    assert!(
        close(on_screen.0.0, on_cpu.0.0) && close(on_screen.0.1, on_cpu.0.1) && close(on_screen.1.0, on_cpu.1.0) && close(on_screen.1.1, on_cpu.1.1),
        "screen {on_screen:?} vs cpu {on_cpu:?}"
    );
}

/// A host that makes the doc only after the drag ended (a slow capture): the mark waits
/// in the editor and is committed into the doc when it arrives.
#[test]
fn late_doc_gets_pending_mark() {
    use crate::{Canvas, Editor, Xf};
    use guilab_image::{Body, Doc};
    let mut h = Headless::new(vec2(800.0, 600.0), 1.0);
    guilab_kit::theme::install(&h.ctx);
    let host = DemoHost::new([400, 300], 1.0);
    let mut ed = Editor::new("e", host.editor.fonts().clone());
    ed.tool = Tool::Arrow;
    let xf = Xf { offset: vec2(0.0, 0.0), zoom: 1.0 };
    let mut doc: Option<Doc> = None;
    let mut begun = 0;
    let mut edits = Vec::new();
    let frame = |h: &mut Headless, ed: &mut Editor, doc: &mut Option<Doc>, events: Vec<Event>, begun: &mut u32, edits: &mut Vec<Edit>| {
        h.run(events, 0.1, |ui| {
            let out = ed.canvas(ui, Canvas { doc: doc.as_mut(), xf, clip: ui.max_rect(), new_doc: ([400, 300], 1.0), visible: true, input: true });
            *begun += out.begin as u32;
            edits.extend(out.edits);
        });
    };
    let (a, b) = (egui::pos2(50.0, 50.0), egui::pos2(150.0, 120.0));
    frame(&mut h, &mut ed, &mut doc, vec![ev::moved(a), ev::press(a)], &mut begun, &mut edits);
    frame(&mut h, &mut ed, &mut doc, vec![ev::moved(b)], &mut begun, &mut edits);
    frame(&mut h, &mut ed, &mut doc, vec![ev::release(b)], &mut begun, &mut edits);
    frame(&mut h, &mut ed, &mut doc, vec![], &mut begun, &mut edits);
    assert!(edits.is_empty(), "nothing to commit into yet");
    doc = Some(Doc::open(Body::screenshot(host.live.0, [400, 300], 1.0), &[host.live.clone()].into_iter().collect::<std::collections::HashMap<_, _>>()).unwrap());
    frame(&mut h, &mut ed, &mut doc, vec![], &mut begun, &mut edits);
    assert_eq!(begun, 1);
    assert_eq!(edits.len(), 1);
    assert_eq!(doc.unwrap().body().elements[1].kind, Kind::Arrow(pt(50.0, 50.0), pt(150.0, 120.0)));
}

#[test]
fn second_insert_cascades() {
    let mut t = T::new();
    t.host.insert(sample_svg());
    t.host.insert(sample_svg());
    let dsts: Vec<_> = t.elements().iter().filter_map(|e| if let Kind::Svg { dst, .. } = e.kind { Some(dst.min) } else { None }).collect();
    assert_eq!(dsts, vec![pt(220.0, 172.0), pt(240.0, 192.0)]);
}

/// A host that switches docs mid-drag without `reset()`: the drag must not land on the
/// element with the same key in the other doc.
#[test]
fn doc_switch_drops_gesture() {
    use crate::{Canvas, Editor, Xf};
    use guilab_image::{Body, Doc};
    let mut h = Headless::new(vec2(800.0, 600.0), 1.0);
    guilab_kit::theme::install(&h.ctx);
    let host = DemoHost::new([400, 300], 1.0);
    let mut ed = Editor::new("e", host.editor.fonts().clone());
    ed.tool = Tool::Move;
    let xf = Xf { offset: vec2(0.0, 0.0), zoom: 1.0 };
    let with_box = |x: f32| {
        let mut d = Doc::open(Body::screenshot(host.live.0, [400, 300], 1.0), &[host.live.clone()].into_iter().collect::<std::collections::HashMap<_, _>>()).unwrap();
        let e = d.add_edit(Element::new(Color::RED, Kind::Box(Rect::new(x, 50.0, 50.0, 50.0)))).unwrap();
        d.apply(&e);
        d
    };
    let (mut a, mut b) = (with_box(50.0), with_box(200.0));
    let mut edits = Vec::new();
    let mut frame = |h: &mut Headless, doc: &mut Doc, events: Vec<Event>| {
        h.run(events, 0.1, |ui| {
            edits.extend(ed.canvas(ui, Canvas { doc: Some(doc), xf, clip: ui.max_rect(), new_doc: ([400, 300], 1.0), visible: true, input: true }).edits);
        });
    };
    let (p, q) = (egui::pos2(50.0, 75.0), egui::pos2(80.0, 75.0));
    frame(&mut h, &mut a, vec![ev::moved(p), ev::press(p)]);
    frame(&mut h, &mut a, vec![ev::moved(q)]);
    frame(&mut h, &mut b, vec![ev::release(q)]);
    assert!(edits.is_empty(), "{edits:?}");
}

/// An inserted picture no store image uses yet survives the store's sweep: the doc holds it.
#[test]
fn inserted_picture_survives_sweep() {
    let mut t = T::new();
    t.host.insert(crate::demo::sample_raster(200, 100));
    t.host.store.sweep(std::time::Instant::now() + std::time::Duration::from_secs(1));
    let host = &mut t.host;
    let out = t.h.run(vec![], 0.1, |ui| host.ui(ui));
    let screen = t.h.render(&out, egui::Color32::BLACK);
    let c = t.host.xf.apply(pt(300.0, 200.0));
    let px = screen.pixels[c.y as usize * screen.size[0] + c.x as usize].to_array();
    assert!(px[0].abs_diff(127) < 8 && px[2].abs_diff(180) < 8, "{px:?}");
}

fn whole(v: f32) -> bool {
    v == v.round()
}

fn all_whole(k: &Kind) -> bool {
    match k {
        Kind::Pen(p) => p.iter().all(|p| whole(p.x) && whole(p.y)),
        Kind::Box(r) | Kind::Ellipse(r) => [r.min.x, r.min.y, r.size.w, r.size.h].into_iter().all(whole),
        Kind::Line(a, b) | Kind::Arrow(a, b) => [a.x, a.y, b.x, b.y].into_iter().all(whole),
        Kind::Text { at, size, .. } => whole(at.x) && whole(at.y) && whole(*size),
        _ => true,
    }
}

/// At a fractional zoom, committed geometry is still whole canvas pixels.
#[test]
fn committed_geometry_is_whole_pixels() {
    let mut t = T::new();
    t.host.xf = crate::Xf { offset: vec2(13.3, 7.7), zoom: 1.37 };
    for tool in [Tool::Box, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Pen] {
        t.host.editor.tool = tool;
        t.drag((100.3, 100.6), (200.45, 150.2));
    }
    t.host.editor.tool = Tool::Move;
    t.drag((100.4, 125.2), (130.1, 140.7));
    // (That moved the ellipse, which is above the box.) Reshape it by its top-left corner.
    t.drag((130.0, 117.0), (120.3, 96.7));
    t.host.editor.tool = Tool::Text;
    t.click(300.3, 300.7);
    t.run(vec![Event::Text("hi".into())]);
    t.key(Key::Enter, Modifiers::NONE);
    let els = t.elements();
    assert_eq!(els.len(), 7);
    assert_eq!(els[2].kind, Kind::Ellipse(Rect::new(120.0, 96.0, 110.0, 69.0)));
    for e in &els {
        assert!(all_whole(&e.kind), "{:?}", e.kind);
    }
}

/// A straight pen stroke drawn with many points keeps only what its shape needs.
#[test]
fn pen_is_simplified() {
    let mut t = T::new();
    t.host.editor.tool = Tool::Pen;
    let mut evs = vec![ev::moved(t.at(100.0, 100.0)), ev::press(t.at(100.0, 100.0))];
    for i in 1..=100 {
        evs.push(ev::moved(t.at(100.0 + i as f32 * 2.0, 100.0)));
    }
    evs.push(ev::release(t.at(300.0, 100.0)));
    t.run(evs);
    t.run(vec![]);
    assert_eq!(t.elements()[1].kind, Kind::Pen(vec![pt(100.0, 100.0), pt(300.0, 100.0)]));
}

/// An element dragged far off the image stays within reach.
#[test]
fn move_keeps_element_reachable() {
    let mut t = T::new();
    t.with_marks();
    t.host.editor.tool = Tool::Move;
    t.drag((100.0, 150.0), (-900.0, 150.0));
    let b = t.elements()[1].bounds(1.0, t.host.editor.fonts());
    assert!(b.max().x > 0.0, "{b:?}");
}

/// A bare host around one editor: canvas over `clip`, optional host widgets before and after it.
struct Bare {
    h: Headless,
    ed: crate::Editor,
    live: (guilab_image::AssetId, std::sync::Arc<guilab_image::Asset>),
    doc: Option<guilab_image::Doc>,
    xf: crate::Xf,
    clip: egui::Rect,
    visible: bool,
    input: bool,
    /// A drag widget registered after the canvas (like the viewer's resize handles).
    after: Option<egui::Rect>,
    /// A click widget registered before the canvas.
    before: Option<egui::Rect>,
    after_dragged: bool,
    before_clicked: bool,
    out: crate::CanvasOut,
    edits: Vec<Edit>,
    pan: egui::Vec2,
}

fn bare_doc(live: &(guilab_image::AssetId, std::sync::Arc<guilab_image::Asset>)) -> guilab_image::Doc {
    let assets: std::collections::HashMap<_, _> = [live.clone()].into();
    guilab_image::Doc::open(guilab_image::Body::screenshot(live.0, [400, 300], 1.0), &assets).unwrap()
}

impl Bare {
    fn new() -> Bare {
        let h = Headless::new(vec2(800.0, 600.0), 1.0);
        guilab_kit::theme::install(&h.ctx);
        let host = DemoHost::new([400, 300], 1.0);
        let ed = crate::Editor::new("e", host.editor.fonts().clone());
        let mut b = Bare {
            h,
            ed,
            live: host.live,
            doc: None,
            xf: crate::Xf { offset: vec2(0.0, 0.0), zoom: 1.0 },
            clip: egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0)),
            visible: true,
            input: true,
            after: None,
            before: None,
            after_dragged: false,
            before_clicked: false,
            out: Default::default(),
            edits: Vec::new(),
            pan: egui::Vec2::ZERO,
        };
        b.run(vec![]);
        b.run(vec![]);
        b
    }

    fn open(&mut self) {
        self.doc = Some(bare_doc(&self.live));
    }

    fn run(&mut self, events: Vec<Event>) -> egui::FullOutput {
        let Bare { h, ed, live, doc, xf, clip, visible, input, after, before, after_dragged, before_clicked, out, edits, pan } = self;
        h.run(events, 0.1, |ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                if let Some(r) = *before {
                    *before_clicked |= ui.interact(r, egui::Id::new("before"), egui::Sense::click()).clicked();
                }
                let o = ed.canvas(ui, crate::Canvas { doc: doc.as_mut(), xf: *xf, clip: *clip, new_doc: ([400, 300], 1.0), visible: *visible, input: *input });
                if let Some(r) = *after {
                    *after_dragged |= ui.interact(r, egui::Id::new("after"), egui::Sense::drag()).dragged();
                }
                if o.begin && doc.is_none() {
                    *doc = Some(bare_doc(live));
                }
                edits.extend(o.edits.iter().cloned());
                *pan += o.pan;
                *out = o;
            });
        })
    }

    fn mv(&mut self, p: Pos2) {
        self.run(vec![ev::moved(p)]);
    }

    fn drag(&mut self, a: Pos2, b: Pos2) {
        self.run(vec![ev::moved(a)]);
        self.run(vec![ev::press(a)]);
        self.run(vec![ev::moved(a + (b - a) * 0.5)]);
        self.run(vec![ev::moved(b)]);
        self.run(vec![ev::release(b)]);
    }
}

/// `input: false` paints the doc but takes no presses (a press that closes a menu).
#[test]
fn input_off_paints_but_ignores_presses() {
    let mut b = Bare::new();
    b.open();
    let d = b.doc.as_mut().unwrap();
    let e = d.add_edit(Element::new(Color::RED, Kind::Box(Rect::new(100.0, 100.0, 100.0, 100.0)))).unwrap();
    d.apply(&e);
    b.ed.tool = Tool::Box;
    b.input = false;
    b.run(vec![]);
    b.drag(egui::pos2(250.0, 50.0), egui::pos2(350.0, 150.0));
    assert!(b.edits.is_empty(), "no drawing while input is off");
    let out = b.run(vec![]);
    let img = b.h.render(&out, egui::Color32::BLACK);
    let px = img.pixels[100 * img.size[0] + 150].to_array();
    assert!(px[0] > 150 && px[1] < 80, "the box is painted: {px:?}");
    assert_eq!(b.out.cursor, None, "no tool cursor while input is off");
    // Back on: the same drag draws.
    b.input = true;
    b.run(vec![]);
    b.drag(egui::pos2(250.0, 50.0), egui::pos2(350.0, 150.0));
    assert_eq!(b.edits.len(), 1);
}

/// Hiding the canvas (or turning input off) mid-drag cancels the drag; the next press works.
#[test]
fn hiding_mid_drag_cancels() {
    for hide_by_input in [false, true] {
        let mut b = Bare::new();
        b.ed.tool = Tool::Box;
        let (a, m) = (egui::pos2(50.0, 50.0), egui::pos2(120.0, 120.0));
        b.run(vec![ev::moved(a)]);
        b.run(vec![ev::press(a)]);
        assert!(b.out.begin);
        b.run(vec![ev::moved(m)]);
        if hide_by_input {
            b.input = false;
        } else {
            b.visible = false;
        }
        b.run(vec![]);
        assert!(b.out.abandon, "the begun doc is abandoned");
        b.run(vec![ev::release(m)]);
        b.doc = None;
        b.input = true;
        b.visible = true;
        b.run(vec![]);
        assert!(!b.ed.busy());
        b.drag(egui::pos2(200.0, 200.0), egui::pos2(260.0, 240.0));
        assert_eq!(b.edits.len(), 1, "the next drag draws (by input: {hide_by_input})");
    }
}

/// A host widget drawn after the canvas (a resize handle) gets its presses alone; one drawn
/// before it still gets clicks while the tool is Interact.
#[test]
fn host_widgets_keep_their_presses() {
    let mut b = Bare::new();
    b.open();
    b.ed.tool = Tool::Move;
    b.after = Some(egui::Rect::from_min_size(egui::pos2(500.0, 100.0), vec2(20.0, 20.0)));
    b.run(vec![]);
    b.mv(egui::pos2(510.0, 110.0));
    assert_eq!(b.out.cursor, None, "the handle's cursor, not the canvas's");
    b.drag(egui::pos2(510.0, 110.0), egui::pos2(560.0, 110.0));
    assert!(b.after_dragged, "the handle dragged");
    assert_eq!(b.pan, vec2(0.0, 0.0), "and the view didn't pan");
    b.drag(egui::pos2(300.0, 300.0), egui::pos2(320.0, 300.0));
    assert_eq!(b.pan, vec2(20.0, 0.0), "elsewhere the move tool still pans");

    let mut b = Bare::new();
    b.before = Some(egui::Rect::from_min_size(egui::pos2(100.0, 100.0), vec2(40.0, 40.0)));
    b.run(vec![]);
    let p = egui::pos2(120.0, 120.0);
    b.run(vec![ev::moved(p)]);
    b.run(vec![ev::press(p), ev::release(p)]);
    assert!(b.before_clicked, "Interact leaves clicks to widgets under the canvas");
}

/// The text field stays inside the canvas clip, below menus.
#[test]
fn text_field_is_clipped() {
    let mut b = Bare::new();
    b.open();
    b.clip = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 600.0));
    b.ed.tool = Tool::Text;
    b.run(vec![]);
    let p = egui::pos2(300.0, 100.0);
    b.run(vec![ev::moved(p)]);
    b.run(vec![ev::press(p), ev::release(p)]);
    b.run(vec![]);
    b.run(vec![ev::text("some words that run past the edge")]);
    // Pan so the field sticks out of the clip.
    b.xf.offset = vec2(150.0, 0.0);
    b.run(vec![]);
    let out = b.run(vec![]);
    let img = b.h.render(&out, egui::Color32::from_rgb(0, 0, 255));
    let w = img.size[0];
    let leaks = (0..600).flat_map(|y| (402..800).map(move |x| (x, y))).filter(|&(x, y)| img.pixels[y * w + x] != egui::Color32::from_rgb(0, 0, 255)).count();
    assert_eq!(leaks, 0, "nothing painted outside the clip");
    let order = b.h.ctx.memory(|m| m.areas().visible_layer_ids().into_iter().find(|l| l.id == egui::Id::new("e").with("text-area")).map(|l| l.order));
    assert_eq!(order, Some(egui::Order::Middle));
}

/// An empty view (a collapsed panel) never gives a zero zoom or NaN points.
#[test]
fn empty_view_is_harmless() {
    let xf = crate::Xf::fit(IRect::new(0, 0, 400, 300), egui::Rect::from_min_size(egui::pos2(10.0, 10.0), vec2(0.0, 0.0)), 1.0);
    assert!(xf.zoom > 0.0 && xf.zoom.is_finite(), "{xf:?}");
    let mut b = Bare::new();
    b.ed.tool = Tool::Pen;
    b.clip = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), vec2(0.0, 0.0));
    b.xf = crate::Xf { offset: vec2(10.0, 10.0), zoom: 0.0 };
    b.drag(egui::pos2(10.0, 10.0), egui::pos2(10.0, 10.0));
    assert!(!b.out.begin && b.edits.is_empty() && b.doc.is_none());
}
