//! Agent input chains: turns `input` actions into [`InputEvent`]s.
//!
//! A chain runs until it reaches a pause (`sleep`, or the steps of a glide or gesture) and
//! returns how long to wait; the server resumes it from its timer, so sleeping never blocks
//! other sessions or requests.

use std::{collections::VecDeque, time::Duration};

use guilab_proto::{Action, Button, InputInfo, TreeNode};
use smithay::{
    backend::input::AxisSource,
    input::keyboard::{Keycode, Keysym},
    utils::{Logical, Point},
    wayland::seat::WaylandFocus,
};

use crate::{
    handlers::surface_id,
    input::{InputEvent, InputSource},
    keys::{KeyPos, char_keysym},
    session::Session,
};

/// Interval between the steps of glides and gestures.
const STEP: Duration = Duration::from_millis(16);
/// Keys wait this long after keyboard focus moved to an X window: our SetInputFocus (X11
/// connection) and the keys (Wayland connection) reach Xwayland on different sockets, so
/// keys sent at once can arrive first and go to the old X focus.
const X_FOCUS_WAIT: Duration = Duration::from_millis(100);
/// Longest wait for a clicked-away menu to close before keys (see `popup_may_close`).
const MENU_WAIT: Duration = Duration::from_millis(500);
/// Pause between keymap swaps when typing into an X client (see `type_text`).
const X_KEYMAP_GAP: Duration = Duration::from_millis(100);
/// Logical pixels per wheel notch (libinput's convention).
const WHEEL_PX: f64 = 15.0;

enum Step {
    Act(usize, Action),
    Ev(InputEvent),
    Sleep(Duration),
    /// Switch the seat keymap (text, extra keysyms placed on spare keycodes).
    Keymap(String, Vec<(Keysym, Keycode)>),
}

pub struct Chain {
    steps: VecDeque<Step>,
    /// `on` surface: coordinates are relative to its top-left.
    origin: Option<u32>,
    /// How long the next keyboard action has waited for a menu to close.
    menu_wait: Duration,
    source: InputSource,
    /// Surface under the pointer at the chain's last button press (a menu item it closed).
    pressed_on: Option<u32>,
}

/// Failure in the middle of a chain: which action, and why.
pub fn action_err(i: usize, a: &Action, msg: impl std::fmt::Display) -> String {
    let done = if i > 0 { " (the actions before it were done)" } else { "" };
    format!("input action {} `{a}`: {msg}{done}", i + 1)
}

fn button_code(b: Button) -> u32 {
    match b {
        Button::Left => 0x110,
        Button::Right => 0x111,
        Button::Middle => 0x112,
        Button::Back => 0x113,
        Button::Forward => 0x114,
    }
}

fn find_node(nodes: &[TreeNode], id: u32) -> Option<&TreeNode> {
    nodes.iter().find_map(|n| if n.id == id { Some(n) } else { find_node(&n.children, id) })
}

impl Chain {
    pub fn new(actions: Vec<Action>, source: InputSource) -> Chain {
        let steps = actions.into_iter().enumerate().map(|(i, a)| Step::Act(i, a)).collect();
        Chain { steps, origin: None, menu_wait: Duration::ZERO, source, pressed_on: None }
    }

    pub fn source(&self) -> InputSource {
        self.source
    }
}

impl Session {
    /// Checks what can be checked before running anything (limits, key names, text).
    pub fn check_chain(&self, actions: &[Action]) -> Result<(), String> {
        for (i, a) in actions.iter().enumerate() {
            let r = a.check().and_then(|()| match a {
                Action::Key { keys } => keys.iter().try_for_each(|k| self.keymap.key(k).map(drop)),
                Action::KeyDown { key } | Action::KeyUp { key } => self.keymap.key(key).map(drop),
                Action::Type { text } => match text.chars().find(|c| char_keysym(*c).is_none()) {
                    Some(c) => Err(format!("can't type control character {c:?}")),
                    None => Ok(()),
                },
                _ => Ok(()),
            });
            r.map_err(|e| format!("input action {} `{a}`: {e} (nothing was done)", i + 1))?;
        }
        Ok(())
    }

    /// Runs the chain until it finishes (`Ok(None)`) or must wait (`Ok(Some(delay))`).
    pub fn run_chain(&mut self, chain: &mut Chain) -> Result<Option<Duration>, String> {
        while let Some(step) = chain.steps.pop_front() {
            match step {
                Step::Sleep(d) => return Ok(Some(d)),
                Step::Ev(ev) => self.inject(ev, chain.source),
                Step::Keymap(text, placed) => {
                    let kb = self.seat.get_keyboard().ok_or("no keyboard")?;
                    kb.set_keymap_from_string(self, text).map_err(|e| format!("typing keymap: {e}"))?;
                    tracing::debug!(?placed, "typing keymap");
                    self.input.extra_keys = placed;
                }
                Step::Act(i, a) => {
                    // Keys right after a click that may close a menu: give the app a moment
                    // to close it, or the keys go to the menu.
                    let keyboard = matches!(a, Action::Key { .. } | Action::KeyDown { .. } | Action::KeyUp { .. } | Action::Type { .. });
                    if keyboard && chain.menu_wait < MENU_WAIT && self.popup_may_close() {
                        chain.menu_wait += STEP;
                        chain.steps.push_front(Step::Act(i, a));
                        return Ok(Some(STEP));
                    }
                    chain.menu_wait = Duration::ZERO;
                    if keyboard && let Some(left) = self.input.x_focus_at.and_then(|t| X_FOCUS_WAIT.checked_sub(t.elapsed())) {
                        chain.steps.push_front(Step::Act(i, a));
                        return Ok(Some(left.max(Duration::from_millis(1))));
                    }
                    let mut later = Vec::new();
                    self.act(chain, &a, &mut later).map_err(|e| action_err(i, &a, e))?;
                    for s in later.into_iter().rev() {
                        chain.steps.push_front(s);
                    }
                }
            }
        }
        self.popup_may_close();
        Ok(None)
    }

    /// State after `chain` finished.
    pub fn input_info(&self, chain: &Chain) -> InputInfo {
        let scale = self.scale();
        let p = self.pointer_location();
        let under = self.surface_under(p).and_then(|(s, _)| surface_id(&s));
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface().and_then(|s| surface_id(&s)));
        let r = |v: f64| (v * scale * 10.0).round() / 10.0;
        InputInfo { pointer: [r(p.x), r(p.y)], under, focus, pressed_on: chain.pressed_on.filter(|s| Some(*s) != under) }
    }

    /// Output pixels (or `on`-surface pixels) -> global logical point, bounds-checked.
    fn point(&self, chain: &Chain, x: f64, y: f64) -> Result<Point<f64, Logical>, String> {
        let (mut gx, mut gy) = (x, y);
        if let Some(id) = chain.origin {
            let tree = self.tree();
            let n = find_node(&tree, id).ok_or_else(|| format!("surface {id} (from `on {id}`) is gone; see guilab tree"))?;
            gx += n.x as f64;
            gy += n.y as f64;
        }
        let o = self.output_info();
        if gx < 0.0 || gy < 0.0 || gx > o.width as f64 || gy > o.height as f64 {
            let rel = if chain.origin.is_some() { format!(" (output pixel {gx},{gy})") } else { String::new() };
            return Err(format!("{x},{y}{rel} is outside the {}x{} output", o.width, o.height));
        }
        let scale = o.scale;
        Ok((gx / scale, gy / scale).into())
    }

    fn to_logical(&self, px: f64) -> f64 {
        px / self.scale()
    }

    /// Agent keys need a focused window (else they go nowhere, silently). The human's
    /// keys go wherever they go.
    fn need_focus(&self, src: InputSource) -> Result<(), String> {
        if src == InputSource::Agent && self.seat.get_keyboard().and_then(|k| k.current_focus()).is_none() {
            return Err("no window has keyboard focus (click one first)".into());
        }
        Ok(())
    }

    fn key(&mut self, code: Keycode, pressed: bool, source: InputSource) {
        self.inject(InputEvent::Key { code, pressed }, source);
    }

    /// Plans typing `text`. Characters missing from the keymap are typed through a seat
    /// keymap extended with them (keys.rs). The extended keymap stays active afterwards
    /// (its other keys are unchanged): swapping straight back raced with X clients, which
    /// can read the swap-back before looking up the keys typed just before it.
    fn type_text(&mut self, text: &str, src: InputSource, later: &mut Vec<Step>) -> Result<(), String> {
        self.need_focus(src)?;
        let syms: Vec<Keysym> = text.chars().map(|c| char_keysym(c).ok_or_else(|| format!("can't type {c:?}"))).collect::<Result<_, _>>()?;
        let x11 = self.seat.get_keyboard().and_then(|k| k.current_focus()).is_some_and(|f| f.is_x11());
        let spare = self.keymap.spare_count();
        let held = self.seat.get_keyboard().map(|kb| kb.pressed_keys()).unwrap_or_default();
        let mut active = self.input.extra_keys.clone();
        let mut swaps = 0;
        let mut i = 0;
        while i < syms.len() {
            // The longest run whose missing keysyms fit on the spare keycodes.
            let mut extra: Vec<Keysym> = Vec::new();
            let mut j = i;
            while j < syms.len() {
                let s = syms[j];
                if self.keymap.find_typable(s).is_none() && !extra.contains(&s) {
                    if extra.len() == spare {
                        break;
                    }
                    extra.push(s);
                }
                j += 1;
            }
            if j == i {
                return Err("no spare keycodes to type non-keyboard characters".into());
            }
            if !extra.iter().all(|s| active.iter().any(|(a, _)| a == s)) {
                let (text, placed) = self.keymap.extended(&extra).ok_or("couldn't build a keymap for these characters")?;
                // Let X clients look up the previous run's keys before its keymap goes.
                if swaps > 0 && x11 {
                    later.push(Step::Sleep(X_KEYMAP_GAP));
                }
                later.push(Step::Keymap(text, placed.clone()));
                active = placed;
                swaps += 1;
            }
            for s in &syms[i..j] {
                let k = match self.keymap.find_typable(*s) {
                    Some(k) => k.clone(),
                    None => KeyPos { code: active.iter().find(|(p, _)| p == s).unwrap().1, mods: Vec::new() },
                };
                // Modifiers already held aren't touched.
                let mods: Vec<Keycode> = k.mods.iter().copied().filter(|m| !held.contains(m)).collect();
                let ev = |code, pressed| Step::Ev(InputEvent::Key { code, pressed });
                later.extend(mods.iter().map(|m| ev(*m, true)));
                later.push(ev(k.code, true));
                later.push(ev(k.code, false));
                later.extend(mods.iter().rev().map(|m| ev(*m, false)));
            }
            i = j;
        }
        Ok(())
    }

    /// Steps for a glide from `from` to `to` over `ms`, ending exactly at `to`.
    fn glide(from: Point<f64, Logical>, to: Point<f64, Logical>, ms: u32, ev: impl Fn(Point<f64, Logical>) -> InputEvent) -> Vec<Step> {
        let n = (ms as u64 / STEP.as_millis() as u64).max(1);
        let dt = Duration::from_millis(ms as u64) / n as u32;
        let mut out = Vec::new();
        for k in 1..=n {
            let t = k as f64 / n as f64;
            out.push(Step::Sleep(dt));
            out.push(Step::Ev(ev(from + (to - from).upscale(t))));
        }
        out
    }

    fn act(&mut self, chain: &mut Chain, a: &Action, later: &mut Vec<Step>) -> Result<(), String> {
        let src = chain.source;
        match a {
            Action::Move { x, y, ms } => {
                let to = self.point(chain, *x, *y)?;
                if *ms == 0 {
                    self.inject(InputEvent::PointerMotion { pos: to }, src);
                } else {
                    *later = Self::glide(self.pointer_location(), to, *ms, |pos| InputEvent::PointerMotion { pos });
                }
            }
            Action::Down { button } => {
                chain.pressed_on = self.surface_under(self.pointer_location()).and_then(|(s, _)| surface_id(&s));
                self.inject(InputEvent::PointerButton { button: button_code(*button), pressed: true }, src)
            }
            Action::Up { button } => self.inject(InputEvent::PointerButton { button: button_code(*button), pressed: false }, src),
            Action::Click { button, count } => {
                chain.pressed_on = self.surface_under(self.pointer_location()).and_then(|(s, _)| surface_id(&s));
                for _ in 0..*count {
                    self.inject(InputEvent::PointerButton { button: button_code(*button), pressed: true }, src);
                    self.inject(InputEvent::PointerButton { button: button_code(*button), pressed: false }, src);
                }
            }
            Action::Scroll { dx, dy } => self.inject(
                InputEvent::PointerAxis { source: AxisSource::Wheel, delta: (dx * WHEEL_PX, dy * WHEEL_PX), steps: Some((*dx, *dy)), stop: false },
                src,
            ),
            Action::ScrollPx { dx, dy } => {
                // Touchpad-style: a few frames, then the fingers rest before lifting (the stop),
                // so toolkits see no velocity and don't add kinetic scrolling.
                let (lx, ly) = (self.to_logical(*dx), self.to_logical(*dy));
                let n = 5;
                for _ in 0..n {
                    later.push(Step::Ev(InputEvent::PointerAxis { source: AxisSource::Finger, delta: (lx / n as f64, ly / n as f64), steps: None, stop: false }));
                    later.push(Step::Sleep(STEP));
                }
                later.push(Step::Sleep(Duration::from_millis(200)));
                later.push(Step::Ev(InputEvent::PointerAxis { source: AxisSource::Finger, delta: (0.0, 0.0), steps: None, stop: true }));
            }
            Action::Key { keys } => {
                self.need_focus(src)?;
                let mut press: Vec<Keycode> = Vec::new();
                for k in keys {
                    let k = self.keymap.key(k)?;
                    for code in k.mods.iter().chain([&k.code]) {
                        if !press.contains(code) {
                            press.push(*code);
                        }
                    }
                }
                for c in &press {
                    self.key(*c, true, chain.source);
                }
                for c in press.iter().rev() {
                    self.key(*c, false, chain.source);
                }
            }
            Action::KeyDown { key } => {
                self.need_focus(src)?;
                let k = self.keymap.key(key)?;
                for code in k.mods.iter().chain([&k.code]) {
                    self.key(*code, true, chain.source);
                }
            }
            Action::KeyUp { key } => {
                let k = self.keymap.key(key)?;
                for code in [&k.code].into_iter().chain(k.mods.iter().rev()) {
                    self.key(*code, false, chain.source);
                }
            }
            Action::Type { text } => self.type_text(text, src, later)?,
            Action::TouchDown { id, x, y } => {
                if self.input.held.touches.contains_key(id) {
                    return Err(format!("finger {id} is already down"));
                }
                let pos = self.point(chain, *x, *y)?;
                self.inject(InputEvent::TouchDown { slot: *id, pos }, src);
                self.inject(InputEvent::TouchFrame, src);
            }
            Action::TouchMove { id, x, y, ms } => {
                let Some(from) = self.input.held.touches.get(id).copied() else {
                    return Err(format!("finger {id} isn't down (use touch down {id} X Y first)"));
                };
                let to = self.point(chain, *x, *y)?;
                let slot = *id;
                for s in Self::glide(from, to, *ms, |pos| InputEvent::TouchMotion { slot, pos }) {
                    let is_ev = matches!(s, Step::Ev(_));
                    later.push(s);
                    if is_ev {
                        later.push(Step::Ev(InputEvent::TouchFrame));
                    }
                }
                if *ms == 0 {
                    // No pause needed for a jump.
                    later.retain(|s| !matches!(s, Step::Sleep(_)));
                }
            }
            Action::TouchUp { id } => {
                if !self.input.held.touches.contains_key(id) {
                    return Err(format!("finger {id} isn't down"));
                }
                self.inject(InputEvent::TouchUp { slot: *id }, src);
                self.inject(InputEvent::TouchFrame, src);
            }
            Action::Tap { x, y } => {
                let pos = self.point(chain, *x, *y)?;
                let slot = (0..).find(|s| !self.input.held.touches.contains_key(s)).unwrap();
                for ev in [InputEvent::TouchDown { slot, pos }, InputEvent::TouchFrame, InputEvent::TouchUp { slot }, InputEvent::TouchFrame] {
                    self.inject(ev, src);
                }
            }
            Action::Pinch { scale, rotate } => {
                let n = 10;
                later.push(Step::Ev(InputEvent::PinchBegin { fingers: 2 }));
                for k in 1..=n {
                    later.push(Step::Sleep(STEP));
                    let s = scale.powf(k as f64 / n as f64);
                    later.push(Step::Ev(InputEvent::PinchUpdate { delta: (0.0, 0.0).into(), scale: s, rotation: rotate / n as f64 }));
                }
                later.push(Step::Ev(InputEvent::PinchEnd { cancelled: false }));
            }
            Action::Swipe { dx, dy, fingers } => {
                let n = 10;
                let d: Point<f64, Logical> = (self.to_logical(*dx) / n as f64, self.to_logical(*dy) / n as f64).into();
                later.push(Step::Ev(InputEvent::SwipeBegin { fingers: *fingers }));
                for _ in 0..n {
                    later.push(Step::Sleep(STEP));
                    later.push(Step::Ev(InputEvent::SwipeUpdate { delta: d }));
                }
                later.push(Step::Ev(InputEvent::SwipeEnd { cancelled: false }));
            }
            Action::On { surface } => {
                if let Some(id) = surface
                    && find_node(&self.tree(), *id).is_none()
                {
                    return Err(format!("no surface {id}; see guilab tree"));
                }
                chain.origin = *surface;
            }
            Action::Sleep { ms } => later.push(Step::Sleep(Duration::from_millis(*ms as u64))),
        }
        Ok(())
    }
}
