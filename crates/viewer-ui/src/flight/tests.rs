use egui::{pos2, vec2};
use guilab_kit::Theme;

use super::*;

const A: Att = Att::Draft(1);
const B: Att = Att::Draft(2);
const SLOT: SlotId = SlotId::Composer;

fn place(x: f32, y: f32, w: f32, h: f32) -> Place {
    let r = Rect::from_min_size(pos2(x, y), vec2(w, h));
    Place { canvas: r, clip: r, alpha: 1.0 }
}

fn slot_p() -> Place {
    place(10.0, 500.0, 80.0, 50.0)
}

fn view_p() -> Place {
    place(100.0, 0.0, 800.0, 500.0)
}

struct T {
    f: Flights,
    now: f64,
    secs: f64,
}

impl T {
    fn new() -> T {
        let mut f = Flights::default();
        let m = Theme::dark().motion;
        f.begin(0.0, &m);
        T { f, now: 0.0, secs: m.whoosh.secs as f64 }
    }

    fn at(&mut self, now: f64) {
        self.now = now;
        self.f.begin(now, &Theme::dark().motion);
    }

    /// Places with fixed ends: the slot (if `slot`) and the view (for the open `open`).
    fn places(&mut self, slot: Option<Place>, open: Option<Att>) -> Vec<(Att, Place)> {
        self.f.places(|_| slot, |a| (Some(a) == open).then(view_p)).into_iter().map(|(f, p, _)| (f.att, p)).collect()
    }
}

fn close(a: Place, b: Place) -> bool {
    let d = (a.canvas.min - b.canvas.min).length() + (a.canvas.max - b.canvas.max).length() + (a.clip.min - b.clip.min).length() + (a.clip.max - b.clip.max).length();
    d < 0.01 && (a.alpha - b.alpha).abs() < 0.01
}

#[test]
fn out_flies_from_slot_to_view_and_lands() {
    let mut t = T::new();
    t.f.out(A, SLOT);
    assert!(t.f.flying_in(A) && t.f.touching(SLOT, A) && !t.f.touching(SLOT, B));
    let p = t.places(Some(slot_p()), Some(A));
    assert!(close(p[0].1, slot_p()), "starts at the slot");
    t.at(t.secs * 0.5);
    let mid = t.places(Some(slot_p()), Some(A))[0].1;
    assert!(mid.canvas.width() > slot_p().canvas.width() && mid.canvas.width() < view_p().canvas.width());
    t.at(t.secs * 0.999);
    assert!(close(t.places(Some(slot_p()), Some(A))[0].1, view_p()) || t.f.progress(&t.f.flights()[0]) > 0.99);
    t.at(t.secs + 0.001);
    assert!(t.f.flights().is_empty() && !t.f.flying_in(A), "landed");
    assert!(!t.f.moving());
}

#[test]
fn live_app_fades_out_under_an_incoming_image_and_is_back_at_once() {
    let mut t = T::new();
    assert_eq!(t.f.live.value(0.0), 1.0);
    t.f.out(A, SLOT);
    t.at(t.secs * 0.5);
    let v = t.f.live.value(t.now);
    assert!(v > 0.0 && v < 1.0, "fading: {v}");
    t.places(Some(slot_p()), Some(A));
    t.f.home(A, SLOT, Some(view_p()));
    assert_eq!(t.f.live.value(t.now), 1.0);
    // Switching: from one image to the next, the live app stays hidden.
    let mut t = T::new();
    t.f.quiet(false);
    t.f.home(A, SLOT, Some(view_p()));
    t.f.out(B, SLOT);
    assert_eq!(t.f.live.value(t.now), 0.0);
}

#[test]
fn turning_around_mid_air_starts_where_it_is() {
    let mut t = T::new();
    t.f.out(A, SLOT);
    t.at(t.secs * 0.4);
    let last = t.places(Some(slot_p()), Some(A))[0].1;
    // Closed mid-flight: it flies home from where it was, not from the view.
    t.f.home(A, SLOT, Some(view_p()));
    assert_eq!(t.f.flights().len(), 1);
    assert_eq!(t.f.flights()[0].dir, Dir::Home);
    let p = t.places(Some(slot_p()), None)[0].1;
    assert!(close(p, last), "{p:?} vs {last:?}");
    assert!(!t.f.flying_in(A) && t.f.touching(SLOT, A));
    // And reopened mid-way home: out again from there.
    t.at(t.secs * 0.6);
    let last = t.places(Some(slot_p()), None)[0].1;
    t.f.out(A, SLOT);
    let p = t.places(Some(slot_p()), Some(A))[0].1;
    assert!(close(p, last));
    // Full duration from the turn.
    t.at(t.secs * 0.6 + t.secs * 0.9);
    assert_eq!(t.f.flights().len(), 1);
}

#[test]
fn ends_follow_the_slot_and_the_view() {
    let mut t = T::new();
    t.f.home(A, SLOT, Some(view_p()));
    t.at(t.secs * 0.98);
    // The slot scrolled by 40: the flight aims at where it is now.
    let moved = Place { canvas: slot_p().canvas.translate(vec2(0.0, -40.0)), clip: slot_p().clip.translate(vec2(0.0, -40.0)), alpha: 1.0 };
    let p = t.places(Some(moved), None)[0].1;
    assert!((p.canvas.center() - moved.canvas.center()).length() < 3.0, "{p:?}");
    // A slot no longer laid out: the last one seen stays the target.
    let p2 = t.places(None, None)[0].1;
    assert!(close(p, p2));
}

#[test]
fn a_flight_without_an_end_lands_at_once() {
    let mut t = T::new();
    // Never saw the slot.
    t.f.out(A, SLOT);
    assert!(t.places(None, Some(A)).is_empty());
    assert!(t.f.flights().is_empty());
    // Nothing to start from: no flight at all.
    t.f.home(B, SLOT, None);
    assert!(t.f.flights().is_empty());
}

#[test]
fn a_press_lands_incoming_flights_only() {
    let mut t = T::new();
    t.f.home(A, SLOT, Some(view_p()));
    t.f.out(B, SLOT);
    t.f.land_out();
    assert_eq!(t.f.flights().len(), 1);
    assert_eq!(t.f.flights()[0].att, A);
    t.f.finish();
    assert!(t.f.flights().is_empty());
}

#[test]
fn glow_source_slides_between_slots_and_holds_briefly() {
    let mut t = T::new();
    let r1 = Rect::from_min_size(pos2(10.0, 300.0), vec2(80.0, 50.0));
    let r2 = Rect::from_min_size(pos2(10.0, 500.0), vec2(80.0, 50.0));
    let feed = (SlotId::Feed { item: 1, index: 0 }, Att::Draft(9));
    // Appearing quietly: fades in.
    let (src, a) = t.f.glow_source(Some((feed, r1))).unwrap();
    assert_eq!(src, r1);
    assert!(a < 0.01);
    t.at(1.0);
    assert_eq!(t.f.glow_source(Some((feed, r1))).unwrap().1, 1.0);
    // The new slot isn't laid out for a frame or two: the old source holds.
    t.at(1.01);
    assert_eq!(t.f.glow_source(None).unwrap().0, r1);
    t.at(1.02);
    // Then the open image's slot changes (copy-on-edit): it slides over.
    let (src, _) = t.f.glow_source(Some(((SLOT, A), r2))).unwrap();
    assert_eq!(src, r1);
    t.at(1.02 + t.secs * 0.5);
    let (src, _) = t.f.glow_source(Some(((SLOT, A), r2))).unwrap();
    assert!(src.min.y > r1.min.y && src.min.y < r2.min.y);
    assert!(t.f.moving());
    t.at(1.02 + t.secs * 1.01);
    assert_eq!(t.f.glow_source(Some(((SLOT, A), r2))).unwrap().0, r2);
    // Gone for longer than the hold: off.
    for i in 0..4 {
        t.at(2.0 + i as f64 * 0.016);
        t.f.glow_source(None);
    }
    assert!(t.f.glow_source(None).is_none());
    // A jump after a gap doesn't slide.
    t.at(3.0);
    assert_eq!(t.f.glow_source(Some(((SLOT, A), r2))).unwrap().0, r2);
}

#[test]
fn place_in_slot_covers_and_fades_out_of_view() {
    let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 300.0));
    // A 2:1 image with a crop, in a 100x50 slot: the crop fills the slot.
    let slot = Rect::from_min_size(pos2(20.0, 20.0), vec2(100.0, 50.0));
    let p = Place::in_slot([400, 200], IRect { x: 100, y: 50, w: 200, h: 100 }, slot, area);
    assert!((p.canvas.width() - 200.0).abs() < 0.01 && (p.canvas.min.x - (20.0 - 50.0)).abs() < 0.01);
    assert_eq!(p.clip, slot);
    assert_eq!(p.alpha, 1.0);
    // An extreme aspect is center-cropped like the thumbnail.
    let p = Place::in_slot([1000, 100], IRect { x: 0, y: 0, w: 1000, h: 100 }, slot, area);
    assert!((p.canvas.height() - 50.0).abs() < 0.01 && (p.canvas.center().x - slot.center().x).abs() < 0.01);
    // Scrolled out above: at the edge, transparent.
    let out = slot.translate(vec2(0.0, -200.0));
    let p = Place::in_slot([400, 200], IRect { x: 0, y: 0, w: 400, h: 200 }, out, area);
    assert_eq!(p.alpha, 0.0);
    assert!(area.contains_rect(p.clip));
}
