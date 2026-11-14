//! Input delivery: every pointer, key, touch and gesture event reaches the seat through
//! [`Session::inject`], whoever sent it (agent chains today, the viewer later). That's the
//! hook for the event log and for blocking human input while an agent holds the lock.
//!
//! Positions here are global logical coordinates; callers convert from output pixels.

use std::{collections::BTreeMap, time::Instant};

use smithay::{
    backend::input::{AxisSource, ButtonState, InputTime, KeyState},
    desktop::{Window, WindowSurfaceType},
    input::{
        keyboard::{FilterResult, Keycode, xkb},
        pointer::{
            AxisFrame, ButtonEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
        },
        touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::seat::WaylandFocus,
};

use guilab_proto::EventData;

use crate::{focus::KbFocus, handlers::surface_id, session::Session, xwayland::is_or};

/// Who sent an event (for the event log and lock policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Agent,
    Human,
}

/// Key repeat for human typing: keys per second, and ms before repeating starts.
const HUMAN_REPEAT_RATE: i32 = 25;
const HUMAN_REPEAT_DELAY: i32 = 600;

#[derive(Debug, Clone)]
pub enum InputEvent {
    PointerMotion { pos: Point<f64, Logical> },
    /// The pointer left the output (e.g. the viewer's pointer left the app's image): clients
    /// get a leave, and the next motion an enter.
    PointerLeave,
    /// Linux button code (BTN_LEFT = 0x110).
    PointerButton { button: u32, pressed: bool },
    /// `steps` = wheel notches (discrete), else smooth.
    PointerAxis { source: AxisSource, delta: (f64, f64), steps: Option<(f64, f64)>, stop: bool },
    /// xkb keycode (evdev + 8).
    Key { code: Keycode, pressed: bool },
    TouchDown { slot: u32, pos: Point<f64, Logical> },
    TouchMotion { slot: u32, pos: Point<f64, Logical> },
    TouchUp { slot: u32 },
    TouchFrame,
    SwipeBegin { fingers: u32 },
    SwipeUpdate { delta: Point<f64, Logical> },
    SwipeEnd { cancelled: bool },
    PinchBegin { fingers: u32 },
    /// `scale` is absolute (vs. begin), `rotation` relative degrees.
    PinchUpdate { delta: Point<f64, Logical>, scale: f64, rotation: f64 },
    PinchEnd { cancelled: bool },
}

/// Buttons, touch points and the touchpad gesture held through `inject`, to release on
/// unlock (keys: the seat keyboard knows them).
#[derive(Default)]
pub struct Held {
    pub buttons: Vec<u32>,
    /// Touch slot -> position.
    pub touches: BTreeMap<u32, Point<f64, Logical>>,
    /// A swipe or pinch begun and not ended.
    pub gesture: Option<GestureKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKind {
    Swipe,
    Pinch,
}

impl Session {
    /// The surface under a global point, with its global origin.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let (w, loc) = self.space.element_under(pos)?;
        let (s, sloc) = w.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)?;
        Some((s, (sloc + loc).to_f64()))
    }

    fn window_under(&self, pos: Point<f64, Logical>) -> Option<Window> {
        self.space.element_under(pos).map(|(w, _)| w.clone())
    }

    pub fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default()
    }

    /// Click-to-focus: a press focuses and raises the window under it, unless a grab
    /// (popup menu, drag) owns the input.
    fn focus_at(&mut self, pos: Point<f64, Logical>) {
        let grabbed = self.seat.get_pointer().is_some_and(|p| p.is_grabbed())
            || self.seat.get_keyboard().is_some_and(|k| k.is_grabbed())
            || self.seat.get_touch().is_some_and(|t| t.is_grabbed());
        if grabbed {
            return;
        }
        // Override-redirect X windows (menus) never take focus.
        let Some(w) = self.window_under(pos).filter(|w| !is_or(w)) else { return };
        let focused = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let top = self.space.elements().rev().find(|w| !is_or(w)) == Some(&w);
        if !top || focused != KbFocus::of(&w) {
            self.focus_window(&w);
        }
    }

    /// A click outside a popup menu ends the popup grab, but only its pointer half: the
    /// keyboard half keeps focus on the dying popup until the next key, which it swallows.
    /// End it right away and focus the window that was clicked.
    fn fix_popup_focus(&mut self) {
        let (Some(kb), Some(ptr)) = (self.seat.get_keyboard(), self.seat.get_pointer()) else { return };
        // Keyboard grabs only come from popups, which grab the pointer too.
        if !kb.is_grabbed() || ptr.is_grabbed() {
            return;
        }
        kb.unset_grab(self);
        match self.window_under(ptr.current_location()) {
            Some(w) => self.focus_window(&w),
            None => self.focus_top(),
        }
    }

    /// After a click while a popup menu was open: whether the menu is still up, i.e. the
    /// client may still be closing it. Keys sent now would go to the menu, so chains wait
    /// a little. If the menu is gone, moves keyboard focus off it now (smithay's popup
    /// grab would only do that at the next key).
    pub(crate) fn popup_may_close(&mut self) -> bool {
        if !self.click_in_grab {
            return false;
        }
        let Some(kb) = self.seat.get_keyboard() else { return false };
        if !kb.is_grabbed() {
            self.click_in_grab = false;
            return false;
        }
        self.popups.cleanup();
        let focus = kb.current_focus();
        if focus.and_then(|f| f.wl_surface().map(|s| s.into_owned())).is_some_and(|f| self.popups.find_popup(&f).is_some()) {
            return true;
        }
        // The grab ended: it accepts any focus now, and removes itself.
        self.click_in_grab = false;
        let w = self.window_under(self.pointer_location()).filter(|w| !is_or(w)).or_else(|| self.space.elements().rev().find(|w| !is_or(w)).cloned());
        kb.set_focus(self, w.and_then(|w| KbFocus::of(&w)), SERIAL_COUNTER.next_serial());
        false
    }

    /// Output pixels of a global logical point.
    fn px(&self, p: Point<f64, Logical>) -> (i32, i32) {
        let s = self.output.current_scale().fractional_scale();
        ((p.x * s).round() as i32, (p.y * s).round() as i32)
    }

    /// Logs a human input event (agent input is logged as its command). Keys are logged
    /// by `inject` itself, which knows their keysym.
    fn log_human(&mut self, ev: &InputEvent) {
        let data = match ev {
            InputEvent::PointerMotion { pos } => {
                let (x, y) = self.px(*pos);
                EventData::Motion { path: vec![[x, y, 0]] }
            }
            InputEvent::PointerButton { button, pressed } => {
                let p = self.pointer_location();
                let (x, y) = self.px(p);
                let window = self.window_under(p).and_then(|w| w.wl_surface().and_then(|s| surface_id(&s)));
                let button = match button {
                    0x110 => "left".to_string(),
                    0x111 => "right".to_string(),
                    0x112 => "middle".to_string(),
                    0x113 => "back".to_string(),
                    0x114 => "forward".to_string(),
                    b => format!("{b:#x}"),
                };
                EventData::Button { button, pressed: *pressed, x, y, window }
            }
            InputEvent::PointerAxis { delta, steps, .. } => {
                let (x, y) = self.px(self.pointer_location());
                let s = self.output.current_scale().fractional_scale();
                let ((dx, dy), smooth) = match steps {
                    Some(st) => (*st, false),
                    None => ((delta.0 * s, delta.1 * s), true),
                };
                if dx == 0.0 && dy == 0.0 {
                    return;
                }
                EventData::Scroll { dx, dy, x, y, smooth }
            }
            InputEvent::TouchDown { slot, pos } => {
                let (x, y) = self.px(*pos);
                EventData::Touch { finger: *slot, down: true, x, y }
            }
            InputEvent::TouchUp { slot } => {
                let Some(pos) = self.held.touches.get(slot) else { return };
                let (x, y) = self.px(*pos);
                EventData::Touch { finger: *slot, down: false, x, y }
            }
            // Gestures are logged once, when they end, with their totals.
            InputEvent::SwipeBegin { fingers } | InputEvent::PinchBegin { fingers } => {
                let gesture = if matches!(ev, InputEvent::SwipeBegin { .. }) { "swipe" } else { "pinch" };
                let scale = (gesture == "pinch").then_some(1.0);
                self.gesture = Some(EventData::Gesture { gesture: gesture.into(), fingers: *fingers, dx: 0.0, dy: 0.0, scale, cancelled: false });
                return;
            }
            InputEvent::SwipeUpdate { delta } | InputEvent::PinchUpdate { delta, .. } => {
                let s = self.output.current_scale().fractional_scale();
                if let Some(EventData::Gesture { dx, dy, scale, .. }) = &mut self.gesture {
                    (*dx, *dy) = (*dx + delta.x * s, *dy + delta.y * s);
                    if let InputEvent::PinchUpdate { scale: k, .. } = ev {
                        *scale = Some(*k);
                    }
                }
                return;
            }
            InputEvent::SwipeEnd { cancelled: c } | InputEvent::PinchEnd { cancelled: c } => {
                let Some(mut g) = self.gesture.take() else { return };
                if let EventData::Gesture { cancelled, .. } = &mut g {
                    *cancelled = *c;
                }
                g
            }
            _ => return,
        };
        self.log.push(data, None);
    }

    /// Key repeat is client-side, so it's per seat, not per key: on while a human types (held
    /// keys repeat as usual), off for agents (held keys in input chains must not repeat).
    fn set_repeat(&mut self, kb: &smithay::input::keyboard::KeyboardHandle<Session>, on: bool) {
        if self.key_repeat != on {
            self.key_repeat = on;
            kb.change_repeat_info(if on { HUMAN_REPEAT_RATE } else { 0 }, HUMAN_REPEAT_DELAY);
        }
    }

    /// Records pointer and touch input for the viewer's overlay (keys: see `inject`).
    fn push_viz(&mut self, ev: &InputEvent, agent: bool) {
        use guilab_proto::viz::{TouchPhase, VizKind};
        let s = self.output.current_scale().fractional_scale();
        let px = |p: Point<f64, Logical>| (p.x * s, p.y * s);
        let kind = match ev {
            InputEvent::PointerMotion { pos } => {
                let (x, y) = px(*pos);
                VizKind::Motion { x, y }
            }
            InputEvent::PointerButton { pressed, .. } => VizKind::Button { pressed: *pressed },
            InputEvent::PointerAxis { delta, steps, .. } => match steps {
                Some((sx, sy)) => VizKind::Scroll { dx: *sx, dy: *sy, discrete: true },
                None => VizKind::Scroll { dx: delta.0 * s, dy: delta.1 * s, discrete: false },
            },
            InputEvent::PointerLeave => VizKind::Leave,
            InputEvent::TouchDown { slot, pos } | InputEvent::TouchMotion { slot, pos } => {
                let (x, y) = px(*pos);
                let phase = if matches!(ev, InputEvent::TouchDown { .. }) { TouchPhase::Down } else { TouchPhase::Move };
                VizKind::Touch { id: *slot, x, y, phase }
            }
            InputEvent::TouchUp { slot } => {
                let (x, y) = self.held.touches.get(slot).map_or((0.0, 0.0), |p| px(*p));
                VizKind::Touch { id: *slot, x, y, phase: TouchPhase::Up }
            }
            _ => return,
        };
        self.viz.push(agent, kind);
    }

    /// Delivers one event to the seat. The only way input reaches clients.
    pub fn inject(&mut self, ev: InputEvent, source: InputSource) {
        self.last_input = Instant::now();
        self.dirty = true;
        let time = InputTime::now();
        if source == InputSource::Human {
            self.log_human(&ev);
        }
        self.push_viz(&ev, source == InputSource::Agent);
        match ev {
            InputEvent::PointerMotion { pos } => {
                let Some(ptr) = self.seat.get_pointer() else { return };
                let under = self.surface_under(pos);
                ptr.motion(self, under.clone(), &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
                // A popup grab whose popups are gone swallows the next motion (it only
                // removes itself), so the pointer wouldn't move. Send it again.
                if ptr.current_location() != pos {
                    ptr.motion(self, under, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
                }
                ptr.frame(self);
            }
            InputEvent::PointerLeave => {
                let Some(ptr) = self.seat.get_pointer() else { return };
                // A grab (a drag, a popup menu) keeps the pointer.
                if ptr.is_grabbed() {
                    return;
                }
                let location = ptr.current_location();
                ptr.motion(self, None, &MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time });
                ptr.frame(self);
            }
            InputEvent::PointerButton { button, pressed } => {
                let Some(ptr) = self.seat.get_pointer() else { return };
                if pressed {
                    // A click while a menu is open may close it (the client decides).
                    self.click_in_grab |= self.seat.get_keyboard().is_some_and(|k| k.is_grabbed());
                    self.focus_at(ptr.current_location());
                    self.held.buttons.push(button);
                } else {
                    self.held.buttons.retain(|b| *b != button);
                }
                let state = if pressed { ButtonState::Pressed } else { ButtonState::Released };
                ptr.button(self, &ButtonEvent { button, state, serial: SERIAL_COUNTER.next_serial(), time });
                ptr.frame(self);
                if pressed {
                    self.fix_popup_focus();
                }
            }
            InputEvent::PointerAxis { source, delta, steps, stop } => {
                use smithay::backend::input::Axis;
                let Some(ptr) = self.seat.get_pointer() else { return };
                let mut f = AxisFrame::new(time).source(source);
                for (axis, d, s) in [(Axis::Horizontal, delta.0, steps.map(|s| s.0)), (Axis::Vertical, delta.1, steps.map(|s| s.1))] {
                    if d != 0.0 {
                        f = f.value(axis, d);
                        if let Some(s) = s {
                            f = f.v120(axis, (s * 120.0).round() as i32);
                        }
                    } else if stop {
                        f = f.stop(axis);
                    }
                }
                ptr.axis(self, f);
                ptr.frame(self);
            }
            InputEvent::Key { code, pressed } => {
                let Some(kb) = self.seat.get_keyboard() else { return };
                if pressed {
                    self.set_repeat(&kb, source == InputSource::Human);
                }
                let state = if pressed { KeyState::Pressed } else { KeyState::Released };
                let mut sym = None;
                kb.input::<(), _>(self, code, state, SERIAL_COUNTER.next_serial(), time, |_, _, h| {
                    sym = Some(h.modified_sym());
                    FilterResult::Forward
                });
                if pressed
                    && let Some(sym) = sym
                {
                    let m = kb.modifier_state();
                    if let Some(label) = guilab_proto::viz::key_label(&xkb::keysym_get_name(sym), sym.key_char(), m.ctrl, m.alt, m.logo, m.shift) {
                        self.viz.push(source == InputSource::Agent, guilab_proto::viz::VizKind::Key(label));
                    }
                }
                if source == InputSource::Human
                    && let Some(sym) = sym
                {
                    let key = xkb::keysym_get_name(sym);
                    let text = sym.key_char().map(String::from);
                    self.log.push(EventData::Key { key, pressed, text }, None);
                }
            }
            InputEvent::TouchDown { slot, pos } => {
                let Some(touch) = self.seat.get_touch() else { return };
                if self.held.touches.is_empty() {
                    self.focus_at(pos);
                }
                self.held.touches.insert(slot, pos);
                let under = self.surface_under(pos);
                touch.down(self, under, &DownEvent { slot: Some(slot).into(), location: pos, serial: SERIAL_COUNTER.next_serial(), time });
            }
            InputEvent::TouchMotion { slot, pos } => {
                let Some(touch) = self.seat.get_touch() else { return };
                self.held.touches.insert(slot, pos);
                let under = self.surface_under(pos);
                touch.motion(self, under, &TouchMotionEvent { slot: Some(slot).into(), location: pos, time });
            }
            InputEvent::TouchUp { slot } => {
                let Some(touch) = self.seat.get_touch() else { return };
                self.held.touches.remove(&slot);
                touch.up(self, &UpEvent { slot: Some(slot).into(), serial: SERIAL_COUNTER.next_serial(), time });
            }
            InputEvent::TouchFrame => {
                if let Some(touch) = self.seat.get_touch() {
                    touch.frame(self);
                }
            }
            ev => {
                let Some(ptr) = self.seat.get_pointer() else { return };
                match ev {
                    InputEvent::SwipeBegin { .. } => self.held.gesture = Some(GestureKind::Swipe),
                    InputEvent::PinchBegin { .. } => self.held.gesture = Some(GestureKind::Pinch),
                    InputEvent::SwipeEnd { .. } | InputEvent::PinchEnd { .. } => self.held.gesture = None,
                    _ => {}
                }
                let serial = SERIAL_COUNTER.next_serial();
                match ev {
                    InputEvent::SwipeBegin { fingers } => ptr.gesture_swipe_begin(self, &GestureSwipeBeginEvent { serial, time, fingers }),
                    InputEvent::SwipeUpdate { delta } => ptr.gesture_swipe_update(self, &GestureSwipeUpdateEvent { time, delta }),
                    InputEvent::SwipeEnd { cancelled } => ptr.gesture_swipe_end(self, &GestureSwipeEndEvent { serial, time, cancelled }),
                    InputEvent::PinchBegin { fingers } => ptr.gesture_pinch_begin(self, &GesturePinchBeginEvent { serial, time, fingers }),
                    InputEvent::PinchUpdate { delta, scale, rotation } => {
                        ptr.gesture_pinch_update(self, &GesturePinchUpdateEvent { time, delta, scale, rotation })
                    }
                    InputEvent::PinchEnd { cancelled } => ptr.gesture_pinch_end(self, &GesturePinchEndEvent { serial, time, cancelled }),
                    _ => unreachable!(),
                }
                ptr.frame(self);
            }
        }
        self.flush();
    }

    /// Releases every held key, button and touch point and cancels an open gesture (when a
    /// lock ends, so nothing stays stuck).
    pub fn release_all(&mut self, source: InputSource) {
        match self.held.gesture {
            Some(GestureKind::Swipe) => self.inject(InputEvent::SwipeEnd { cancelled: true }, source),
            Some(GestureKind::Pinch) => self.inject(InputEvent::PinchEnd { cancelled: true }, source),
            None => {}
        }
        for button in self.held.buttons.clone() {
            self.inject(InputEvent::PointerButton { button, pressed: false }, source);
        }
        let touches: Vec<u32> = self.held.touches.keys().copied().collect();
        for slot in &touches {
            self.inject(InputEvent::TouchUp { slot: *slot }, source);
        }
        if !touches.is_empty() {
            self.inject(InputEvent::TouchFrame, source);
        }
        if let Some(kb) = self.seat.get_keyboard() {
            for code in kb.pressed_keys() {
                self.inject(InputEvent::Key { code, pressed: false }, source);
            }
        }
    }
}
