//! Headless tests: a bare `egui::Context` plays the host (no window, no GPU).

use egui::{Color32, ColorImage, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};

use crate::{Chat, Kind, Mark, Renderer, Sketch, Tool, View};

const RED: Color32 = crate::COLORS[0].0;

fn white(w: usize, h: usize) -> ColorImage {
    ColorImage::new([w, h], vec![Color32::WHITE; w * h])
}

#[test]
fn composite_box_and_text() {
    let base = white(200, 100);
    let marks = [
        Mark { kind: Kind::Box(pos2(20.0, 20.0), pos2(80.0, 60.0)), color: RED },
        Mark { kind: Kind::Text(pos2(110.0, 30.0), "hi".into()), color: RED },
    ];
    let out = Renderer::default().composite(&base, &marks, 1.0);
    let px = |x: usize, y: usize| out.pixels[y * 200 + x];
    assert_eq!(px(20, 40), RED, "box edge");
    assert_eq!(px(50, 40), Color32::WHITE, "box inside untouched");
    assert_eq!(px(5, 5), Color32::WHITE);
    let reddish = (110..140).flat_map(|x| (30..50).map(move |y| (x, y))).filter(|&(x, y)| {
        let c = px(x, y);
        c.r() > 150 && c.g() < 120
    });
    assert!(reddish.count() > 10, "text drew glyphs");
}

/// The host side: a frame with the image at `rect`, running `f` with the sketch.
fn frame(ctx: &egui::Context, events: Vec<Event>, f: impl FnMut(&mut egui::Ui)) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
    let _ = ctx.run_ui(raw, f);
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

#[test]
fn draw_queue_send() {
    let ctx = egui::Context::default();
    let mut sketch = Sketch::new("s");
    let mut chat = Chat::new("c");
    sketch.tool = Tool::Arrow;
    // Image 400x300 shown at half size at (100, 100).
    let view = View { rect: Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 150.0)), clip: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0)), size: [400, 300], scale: 1.0 };
    let run = |events: Vec<Event>, sketch: &mut Sketch| {
        frame(&ctx, events, |ui| {
            egui::CentralPanel::default().show(ui, |ui| sketch.canvas(ui, view));
        })
    };
    run(vec![Event::PointerMoved(pos2(120.0, 120.0))], &mut sketch);
    run(vec![button(pos2(120.0, 120.0), true)], &mut sketch);
    assert!(sketch.wants_screenshot(), "starting to draw asks for a screenshot");
    assert!(!sketch.wants_screenshot(), "once");
    sketch.set_screenshot(white(400, 300));
    run(vec![Event::PointerMoved(pos2(160.0, 140.0)), Event::PointerMoved(pos2(200.0, 200.0))], &mut sketch);
    run(vec![button(pos2(200.0, 200.0), false)], &mut sketch);
    assert!(sketch.drawing_open());
    sketch.queue_drawing();
    assert_eq!(sketch.queued(), 1);
    assert!(!sketch.drawing_open() && sketch.tool == Tool::Interact);

    // Second drawing: text via the text tool, left open; sending takes both.
    sketch.tool = Tool::Text;
    run(vec![Event::PointerMoved(pos2(250.0, 110.0)), button(pos2(250.0, 110.0), true), button(pos2(250.0, 110.0), false)], &mut sketch);
    assert!(sketch.wants_screenshot());
    sketch.set_screenshot(white(400, 300));
    run(vec![], &mut sketch); // the text box takes focus
    run(vec![Event::Text("ok here".into())], &mut sketch);
    run(vec![Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], &mut sketch);

    *chat.draft_mut() = "two".into();
    let mut sent = None;
    frame(&ctx, vec![], |ui| {
        ui.memory_mut(|m| m.request_focus(egui::Id::new("c").with("draft")));
        chat.show(ui, &mut sketch);
    });
    // Shift+Enter adds a newline instead of sending.
    frame(&ctx, vec![Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::SHIFT }], |ui| {
        sent = chat.show(ui, &mut sketch);
    });
    assert!(sent.is_none(), "Shift+Enter must not send");
    assert_eq!(chat.draft_mut().as_str(), "two\n");
    chat.draft_mut().push_str("things");
    // Enter in the focused message box sends.
    frame(&ctx, vec![Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], |ui| {
        sent = chat.show(ui, &mut sketch);
    });
    let m = sent.expect("sent");
    assert_eq!(m.text, "two\nthings");
    assert_eq!(m.images.len(), 2);
    assert_eq!(sketch.queued(), 0);
    assert!(!sketch.drawing_open());
    // Arrow from image (40, 40) to (200, 200), drawn; no text in the first.
    let a = &m.images[0];
    assert!(a.notes.is_empty());
    let c = a.image.pixels[120 * 400 + 120];
    assert!(c.r() > 150 && c.g() < 120, "arrow shaft at (120,120): {c:?}");
    // Text: a note at the click (image px), not baked into the image, but in the preview.
    let b = &m.images[1];
    assert_eq!(b.notes.len(), 1);
    assert_eq!(b.notes[0].text, "ok here");
    assert_eq!(b.notes[0].pos, pos2(300.0, 20.0));
    assert!(b.image.pixels.iter().all(|p| *p == Color32::WHITE), "text isn't baked by default");
    assert!(b.preview.pixels.iter().any(|p| *p != Color32::WHITE), "the preview has it");
}
