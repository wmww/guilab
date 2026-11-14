//! Interaction tests: the demo host in a headless egui context, driven by synthetic input.

use egui::{Event, Key, Modifiers, Pos2, Rect, vec2};
use guilab_kit::headless::{Headless, ev};

use crate::demo::DemoHost;
use crate::{ChatEvent, SlotId};

struct T {
    h: Headless,
    host: DemoHost,
}

impl T {
    fn new() -> T {
        let h = Headless::new(vec2(420.0, 700.0), 1.0);
        guilab_kit::theme::install(&h.ctx);
        let mut t = T { h, host: DemoHost::new() };
        t.settle();
        t
    }

    fn run(&mut self, events: Vec<Event>) {
        self.run_dt(events, 0.02);
    }

    fn run_dt(&mut self, events: Vec<Event>, dt: f64) {
        let host = &mut self.host;
        self.h.run(events, dt, |ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| host.ui(ui));
        });
    }

    /// Lets animations finish.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.run_dt(vec![], 0.5);
        }
    }

    fn click(&mut self, p: Pos2) {
        self.run(vec![ev::moved(p)]);
        self.run(vec![ev::press(p), ev::release(p)]);
    }

    /// Scrolls content down by `dy` points (up the feed) with the pointer at `p`.
    fn wheel(&mut self, p: Pos2, dy: f32) {
        self.run(vec![ev::moved(p)]);
        for _ in 0..10 {
            self.run_dt(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, dy / 10.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE }], 0.1);
        }
        self.settle();
    }

    fn key(&mut self, k: Key, m: Modifiers) {
        self.run(vec![ev::key(k, m)]);
    }

    fn events(&mut self) -> Vec<ChatEvent<u32>> {
        std::mem::take(&mut self.host.log)
    }

    fn slot(&self, slot: SlotId, key: u32) -> Rect {
        self.host.last.slot(slot, &key).unwrap_or_else(|| panic!("no slot {slot:?} {key}"))
    }

    /// Focuses the text box by clicking it.
    fn focus_text(&mut self) {
        let c = self.host.last.composer;
        self.click(c.left_bottom() + vec2(30.0, -15.0));
        assert!(self.h.ctx.memory(|m| m.has_focus(self.host.chat.text_id())), "text box focused");
    }

    fn send_button(&self) -> Pos2 {
        let c = self.host.last.composer;
        c.right_bottom() + vec2(-4.0 - 14.0, -4.0 - 14.0)
    }
}

#[test]
fn send_rules() {
    let mut t = T::new();
    // Empty: neither Enter nor the button sends.
    t.focus_text();
    t.key(Key::Enter, Modifiers::NONE);
    t.click(t.send_button());
    t.focus_text();
    t.run(vec![ev::text("   ")]);
    t.key(Key::Enter, Modifiers::NONE);
    assert!(t.events().iter().all(|e| !matches!(e, ChatEvent::Send { .. })), "nothing to send");

    // Shift+Enter is a newline; Enter sends the trimmed text and clears the box.
    t.host.chat.draft_mut().clear();
    t.run(vec![ev::text("two")]);
    t.key(Key::Enter, Modifiers::SHIFT);
    t.run(vec![ev::text("lines ")]);
    assert_eq!(t.host.chat.draft(), "two\nlines ");
    t.key(Key::Enter, Modifiers::NONE);
    assert_eq!(t.events(), vec![ChatEvent::Send { text: "two\nlines".into() }]);
    assert_eq!(t.host.chat.draft(), "");
    assert!(t.h.ctx.memory(|m| m.has_focus(t.host.chat.text_id())), "focus stays after sending");

    // Text typed and Enter in one frame sends it all.
    t.run(vec![ev::text("hi"), ev::key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(t.events(), vec![ChatEvent::Send { text: "hi".into() }]);

    // An image alone can be sent, with the button too.
    let empty_h = t.host.last.composer.height();
    let k = t.host.attach(400, 250, 0);
    t.settle();
    t.click(t.send_button());
    assert_eq!(t.events(), vec![ChatEvent::Send { text: "".into() }]);
    assert!(t.host.pending.is_empty());
    let (_, last) = t.host.chat.items().last().unwrap();
    assert_eq!(last.attachments, vec![k]);
    // The sent slot leaves the composer at once (no shrinking).
    t.run(vec![]);
    t.run(vec![]);
    assert!(t.host.last.slots.iter().all(|(s, _, _)| *s != SlotId::Composer));
    t.settle();
    assert_eq!(t.host.last.composer.height(), empty_h, "the box shrinks back");
}

#[test]
fn remove_and_undo() {
    let mut t = T::new();
    let a = t.host.attach(400, 250, 0);
    let b = t.host.attach(400, 250, 1);
    t.settle();
    let ra = t.slot(SlotId::Composer, a);
    let rb = t.slot(SlotId::Composer, b);
    assert!(rb.left() > ra.right(), "in a row");
    assert!(ra.bottom() < t.host.last.composer.bottom() - 20.0, "above the text, inside the box");
    assert!(t.host.last.composer.contains_rect(ra));

    // The × (top right) removes, without opening.
    let x = ra.right_top() + vec2(-12.0, 12.0);
    t.click(x);
    assert_eq!(t.events(), vec![ChatEvent::Remove { key: a }]);
    assert_eq!(t.host.pending, vec![b]);
    // It shrinks out while b slides over, then it's gone.
    t.run(vec![]);
    t.run_dt(vec![], 0.05);
    let mid = t.slot(SlotId::Composer, b);
    assert!(mid.left() < rb.left() && mid.left() > ra.left(), "b slides left");
    t.run(vec![ev::moved(Pos2::new(5.0, 5.0))]);
    t.settle();
    assert!((t.slot(SlotId::Composer, b).left() - ra.left()).abs() < 1.0);

    // Ctrl+Z (no text focus) brings it back in its place; it grows in.
    t.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(t.host.pending, vec![a, b]);
    t.run_dt(vec![], 0.03);
    assert!(t.slot(SlotId::Composer, a).width() < ra.width(), "growing in");
    t.settle();
    assert_eq!(t.slot(SlotId::Composer, a), ra);

    // With the text box focused and untouched, undo is still the host's.
    t.click(t.slot(SlotId::Composer, a).right_top() + vec2(-12.0, 12.0));
    assert_eq!(t.host.pending, vec![b]);
    t.focus_text();
    t.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(t.host.pending, vec![a, b]);
    // After typing, undo is the text's.
    t.run(vec![ev::text("abc")]);
    t.events();
    t.key(Key::Z, Modifiers::COMMAND);
    assert!(!t.events().contains(&ChatEvent::Undo));
    assert_eq!(t.host.chat.draft(), "");
}

#[test]
fn hover_grows_click_opens() {
    let mut t = T::new();
    t.host.sample();
    t.settle();
    let (id, _) = t.host.chat.items().next().unwrap();
    let key = 1;
    let slot = SlotId::Feed { item: id, index: 0 };
    // Scroll to the top so the first image is on screen.
    t.wheel(Pos2::new(200.0, 200.0), 5000.0);
    let rest = t.slot(slot, key);
    assert!(t.host.last.feed_clip.contains_rect(rest));
    t.run(vec![ev::moved(rest.center())]);
    t.settle();
    let grown = t.slot(slot, key);
    assert!(grown.width() > rest.width() * 1.1 && (grown.center() - rest.center()).length() < 1.0, "grows in place");
    // A press on the grown edge still counts.
    t.run(vec![ev::moved(grown.right_center() - vec2(2.0, 0.0))]);
    t.run(vec![ev::press(grown.right_center() - vec2(2.0, 0.0)), ev::release(grown.right_center() - vec2(2.0, 0.0))]);
    assert_eq!(t.events(), vec![ChatEvent::Open { slot, key, rect: grown }]);
    // Now it's away (empty, outlined): no grow; a click on it again is reported too.
    t.settle();
    assert_eq!(t.slot(slot, key), rest, "empty slots don't grow");
    t.click(rest.center());
    assert!(matches!(t.events().as_slice(), [ChatEvent::Open { .. }]));
    assert_eq!(t.host.thumbs.open, None);
}

#[test]
fn faint_images_smaller() {
    let mut t = T::new();
    t.host.sample();
    t.settle();
    let th = guilab_kit::Theme::dark();
    let mut faint = 0;
    for (id, it) in t.host.chat.items().map(|(i, it)| (i, it.clone())).collect::<Vec<_>>() {
        for (i, k) in it.attachments.iter().enumerate() {
            let r = t.slot(SlotId::Feed { item: id, index: i as u16 }, *k);
            let want = th.size.feed_thumb_h * if it.faint { th.size.faint_scale } else { 1.0 };
            assert!((r.height() - want).abs() < 0.5, "{} vs {want}", r.height());
            faint += it.faint as usize;
        }
    }
    assert_eq!(faint, 2);
}

#[test]
fn copy_on_edit_states() {
    let mut t = T::new();
    t.host.sample();
    t.settle();
    let (id, _) = t.host.chat.items().next().unwrap();
    t.host.thumbs.open = Some((SlotId::Feed { item: id, index: 0 }, 1));
    t.settle();
    t.host.edit_open();
    t.run(vec![]);
    t.run(vec![]);
    let new = *t.host.pending.last().unwrap();
    assert_eq!(t.host.thumbs.open, Some((SlotId::Composer, new)));
    assert!(t.host.last.slot(SlotId::Composer, &new).is_some());
    t.settle();
}

#[test]
fn follows_the_end() {
    let mut t = T::new();
    for i in 0..40 {
        t.host.chat.push(crate::Item::activity("", format!("command {i}")));
    }
    t.settle();
    let k = t.host.image(400, 250, 0);
    let id = t.host.chat.push(crate::Item::me("", "").with([k]));
    t.settle();
    let clip = t.host.last.feed_clip;
    assert!(clip.contains_rect(t.slot(SlotId::Feed { item: id, index: 0 }, k)), "new item scrolled into view");
    // Scrolled up, new items don't pull the view down.
    t.wheel(clip.center(), 400.0);
    let k2 = t.host.image(400, 250, 1);
    let id2 = t.host.chat.push(crate::Item::me("", "").with([k2]));
    t.settle();
    assert!(!clip.intersects(t.slot(SlotId::Feed { item: id2, index: 0 }, k2)));
    // The jump button brings it into view.
    let b = Pos2::new(clip.right() - 12.0 - 14.0 - 8.0, clip.bottom() - 8.0 - 14.0);
    t.click(b);
    t.settle();
    assert!(clip.contains_rect(t.slot(SlotId::Feed { item: id2, index: 0 }, k2)));
}

#[test]
fn growing_composer_keeps_the_end() {
    let mut t = T::new();
    t.host.sample();
    t.host.status = None;
    t.settle();
    let (id, it) = t.host.chat.items().nth(7).map(|(i, it)| (i, it.clone())).unwrap();
    let last = |t: &T| t.slot(SlotId::Feed { item: id, index: 0 }, it.attachments[0]);
    t.focus_text();
    let before = t.host.last.feed_clip.bottom() - last(&t).bottom();
    for i in 0..12 {
        t.run(vec![ev::text(&format!("line {i}")), ev::key(Key::Enter, Modifiers::SHIFT)]);
    }
    t.settle();
    let after = t.host.last.feed_clip.bottom() - last(&t).bottom();
    assert!(t.host.last.composer.height() > 150.0);
    assert!((before - after).abs() < 1.0, "the feed's end stays in view: {before} vs {after}");
}
