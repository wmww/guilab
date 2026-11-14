//! What the viewer shows of input reaching a session's apps (pointer glow, click ripples,
//! touch points, a key stream), from agents and humans alike. In-process only.

use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum VizKind {
    /// Pointer position, output pixels.
    Motion { x: f64, y: f64 },
    Button { pressed: bool },
    /// The pointer left the output (e.g. the human's pointer left the app in the viewer).
    Leave,
    /// Wheel notches (`discrete`) or smooth output pixels; positive = down/right.
    Scroll { dx: f64, dy: f64, discrete: bool },
    Touch { id: u32, x: f64, y: f64, phase: TouchPhase },
    /// A key press, labeled for display ("A", "Ctrl+S", "Enter", "F2"; the viewer draws
    /// symbols for Enter, Space, Backspace and arrows).
    Key(String),
    /// Another change to the app's world, e.g. "1280×720".
    Note(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone)]
pub struct VizEvent {
    /// Increases by one per event in a session.
    pub seq: u64,
    pub at: Instant,
    pub agent: bool,
    pub kind: VizKind,
}

/// Recent viz events of a session (a bounded ring).
#[derive(Debug, Default)]
pub struct VizLog {
    pub events: std::collections::VecDeque<VizEvent>,
    next: u64,
}

impl VizLog {
    const CAP: usize = 512;

    pub fn push(&mut self, agent: bool, kind: VizKind) {
        self.events.push_back(VizEvent { seq: self.next, at: Instant::now(), agent, kind });
        self.next += 1;
        while self.events.len() > Self::CAP {
            self.events.pop_front();
        }
    }

    /// The seq the next event will get.
    pub fn end(&self) -> u64 {
        self.next
    }
}

/// A display label for a key: the character for printable keys (uppercased), a symbol or
/// short name otherwise; `None` for bare modifiers (they show up in chords instead).
pub fn key_label(name: &str, ch: Option<char>, ctrl: bool, alt: bool, logo: bool, shift: bool) -> Option<String> {
    let base = match name {
        "Shift_L" | "Shift_R" | "Control_L" | "Control_R" | "Alt_L" | "Alt_R" | "Super_L" | "Super_R" | "Meta_L" | "Meta_R" | "ISO_Level3_Shift" | "Caps_Lock" | "Num_Lock" => return None,
        "Return" | "KP_Enter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        "BackSpace" => "Backspace".to_string(),
        "Tab" | "ISO_Left_Tab" => "Tab".to_string(),
        "Escape" => "Esc".to_string(),
        "Left" => "Left".to_string(),
        "Right" => "Right".to_string(),
        "Up" => "Up".to_string(),
        "Down" => "Down".to_string(),
        "Delete" => "Del".to_string(),
        "Page_Up" => "PgUp".to_string(),
        "Page_Down" => "PgDn".to_string(),
        _ => match ch.filter(|c| !c.is_control() && !c.is_whitespace()) {
            Some(c) => c.to_uppercase().collect(),
            None => name.to_string(),
        },
    };
    let printable = ch.is_some_and(|c| !c.is_control() && !c.is_whitespace());
    let mut s = String::new();
    for (on, m) in [(ctrl, "Ctrl+"), (alt, "Alt+"), (logo, "Super+"), (shift && !printable, "Shift+")] {
        if on {
            s.push_str(m);
        }
    }
    s.push_str(&base);
    Some(s)
}
