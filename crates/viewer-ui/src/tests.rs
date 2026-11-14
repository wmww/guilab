//! The viewer UI driven headless through [`crate::demo::Demo`]: flights start and land
//! with the open image, input lands them, and quiet changes don't fly.

use egui::{Event, Pos2, pos2, vec2};
use guilab_chat::{Item, SlotId};
use guilab_editor::Tool;

use crate::demo::{Demo, press};
use crate::flight::Dir;
use crate::input::HostInput;
use crate::workspace::{Att, Open, Workspace};

fn demo() -> Demo {
    let mut d = Demo::new(vec2(1000.0, 700.0), 1.0, [1200, 750], true);
    d.idle(3);
    d
}

fn ws(d: &Demo) -> &Workspace {
    d.ui.workspace().unwrap()
}

fn ev(d: &mut Demo, evs: Vec<Event>) {
    d.frame(evs.into_iter().map(HostInput::Egui).collect(), 1.0 / 60.0);
}

/// Draws a box on the live app (a new draft, open), and settles.
fn draw(d: &mut Demo, a: Pos2, b: Pos2) {
    d.ui.workspace_mut().unwrap().editor.tool = Tool::Box;
    ev(d, vec![Event::PointerMoved(a)]);
    ev(d, vec![press(a, true)]);
    ev(d, vec![Event::PointerMoved(b)]);
    ev(d, vec![press(b, false)]);
    d.idle(30);
}

fn click(d: &mut Demo, p: Pos2) {
    ev(d, vec![Event::PointerMoved(p)]);
    ev(d, vec![press(p, true), press(p, false)]);
}

fn draft_slot(d: &Demo, i: usize) -> (Att, Pos2) {
    let att = Att::Draft(ws(d).pending[i].key);
    (att, ws(d).slot_rect(SlotId::Composer, att).expect("composer slot").center())
}

#[test]
fn drawing_on_the_live_app_flies_nothing() {
    let mut d = demo();
    d.ui.workspace_mut().unwrap().editor.tool = Tool::Box;
    ev(&mut d, vec![Event::PointerMoved(pos2(400.0, 150.0))]);
    ev(&mut d, vec![press(pos2(400.0, 150.0), true)]);
    ev(&mut d, vec![Event::PointerMoved(pos2(600.0, 260.0))]);
    ev(&mut d, vec![press(pos2(600.0, 260.0), false)]);
    assert!(matches!(ws(&d).open, Open::Draft(_)));
    assert!(ws(&d).flights.flights().is_empty());
    // The doc covers the live app in place: the live app is hidden at once (no fade).
    assert_eq!(ws(&d).flights.live.value(ws(&d).flights.now()), 0.0);
}

#[test]
fn open_and_close_fly_and_land() {
    let mut d = demo();
    draw(&mut d, pos2(400.0, 150.0), pos2(600.0, 260.0));
    let (att, _) = draft_slot(&d, 0);
    d.ui.workspace_mut().unwrap().editor.tool = Tool::Interact;
    d.idle(2);
    // Closed: it flies home.
    let f = ws(&d).flights.flights();
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].att, f[0].dir), (att, Dir::Home));
    d.idle(20);
    assert!(ws(&d).flights.flights().is_empty());
    // Opened from its slot: it flies out, and the view waits for it.
    let (_, p) = draft_slot(&d, 0);
    click(&mut d, p);
    assert!(ws(&d).flights.flying_in(att));
    assert!(ws(&d).flights.touching(SlotId::Composer, att));
    d.idle(20);
    assert!(!ws(&d).flights.flying_in(att));
    assert!(ws(&d).view_place.is_some());
}

#[test]
fn a_press_in_the_view_lands_an_incoming_image() {
    let mut d = demo();
    draw(&mut d, pos2(400.0, 150.0), pos2(600.0, 260.0));
    d.ui.workspace_mut().unwrap().editor.tool = Tool::Interact;
    d.idle(30);
    let (att, p) = draft_slot(&d, 0);
    click(&mut d, p);
    assert!(ws(&d).flights.flying_in(att));
    // Drawing right away: the press lands it and the mark is made.
    let n = ws(&d).pending[0].doc.body().elements.len();
    d.ui.workspace_mut().unwrap().editor.tool = Tool::Box;
    ev(&mut d, vec![Event::PointerMoved(pos2(300.0, 300.0))]);
    ev(&mut d, vec![press(pos2(300.0, 300.0), true)]);
    assert!(!ws(&d).flights.flying_in(att));
    ev(&mut d, vec![Event::PointerMoved(pos2(400.0, 380.0))]);
    ev(&mut d, vec![press(pos2(400.0, 380.0), false)]);
    assert_eq!(ws(&d).pending[0].doc.body().elements.len(), n + 1);
}

#[test]
fn switching_and_reversing_mid_air() {
    let mut d = demo();
    draw(&mut d, pos2(400.0, 150.0), pos2(600.0, 260.0));
    d.ui.workspace_mut().unwrap().editor.tool = Tool::Interact;
    d.idle(30);
    draw(&mut d, pos2(300.0, 300.0), pos2(500.0, 400.0));
    let (a, pa) = draft_slot(&d, 0);
    let (b, pb) = draft_slot(&d, 1);
    // B is open; opening A sends B home and A out.
    click(&mut d, pa);
    let f = ws(&d).flights.flights();
    assert_eq!(f.len(), 2);
    assert!(f.iter().any(|f| f.att == b && f.dir == Dir::Home) && f.iter().any(|f| f.att == a && f.dir == Dir::Out));
    // Mid-air, B again: both turn around.
    d.idle(5);
    click(&mut d, pb);
    let f = ws(&d).flights.flights();
    assert_eq!(f.len(), 2);
    assert!(f.iter().any(|f| f.att == b && f.dir == Dir::Out) && f.iter().any(|f| f.att == a && f.dir == Dir::Home));
    d.idle(30);
    assert!(ws(&d).flights.flights().is_empty());
    assert!(matches!(ws(&d).open, Open::Draft(k) if Att::Draft(k) == b));
}

#[test]
fn past_image_flies_out_and_its_first_edit_is_quiet() {
    let mut d = demo();
    let id = d.screenshot().unwrap();
    d.push(Item::them("Agent", "12:00", "Here").with([id]));
    d.idle(5);
    let (slot, att, r) = *ws(&d).slots.iter().find(|(s, _, _)| matches!(s, SlotId::Feed { .. })).unwrap();
    click(&mut d, r.center());
    assert!(ws(&d).flights.flying_in(att));
    d.idle(30);
    assert!(matches!(ws(&d).open, Open::Past { .. }));
    draw(&mut d, pos2(400.0, 150.0), pos2(600.0, 260.0));
    // Copy-on-edit: a new draft is open, nothing flew, and the feed slot shows again.
    assert!(matches!(ws(&d).open, Open::Draft(_)));
    assert!(ws(&d).flights.flights().is_empty());
    assert!(!ws(&d).flights.touching(slot, att));
}

#[test]
fn removing_or_sending_the_open_image_fades_it_out() {
    let mut d = demo();
    draw(&mut d, pos2(400.0, 150.0), pos2(600.0, 260.0));
    let key = ws(&d).pending[0].key;
    let z = crate::Zoom::default();
    let w = d.ui.workspace_mut().unwrap();
    w.remove(key);
    // Nothing flies; the image fades out where it was, the live app fading back in.
    assert!(w.flights.flights().is_empty());
    assert_eq!(w.flights.fades().count(), 1);
    assert_eq!(w.flights.live.value(w.flights.now()), 0.0);
    d.idle(30);
    let w = d.ui.workspace_mut().unwrap();
    assert_eq!(w.flights.fades().count(), 0);
    assert_eq!(w.flights.live.value(w.flights.now()), 1.0);
    w.undo(&d.store, z);
    d.idle(5);
    let w = d.ui.workspace_mut().unwrap();
    let _ = w.send(z);
    assert!(w.flights.flights().is_empty());
    assert_eq!(w.flights.fades().count(), 1);
}

#[test]
fn hiding_the_window_lands_everything() {
    let mut d = demo();
    draw(&mut d, pos2(400.0, 150.0), pos2(600.0, 260.0));
    d.ui.workspace_mut().unwrap().editor.tool = Tool::Interact;
    d.idle(2);
    assert!(!ws(&d).flights.flights().is_empty());
    d.ui.detach();
    assert!(ws(&d).flights.flights().is_empty());
}
