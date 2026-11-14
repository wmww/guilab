//! The `input` action chain: parsing CLI words into [`Action`]s.
//!
//! Coordinates are output pixels (screenshot pixels), or surface-local pixels after `on ID`.
//! The grammar is documented in [`HELP`], which the CLI prints for `guilab input --help`.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl Button {
    fn parse(s: &str) -> Option<Button> {
        Some(match s {
            "left" => Button::Left,
            "right" => Button::Right,
            "middle" => Button::Middle,
            "back" => Button::Back,
            "forward" => Button::Forward,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
            Button::Back => "back",
            Button::Forward => "forward",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "a", rename_all = "snake_case")]
pub enum Action {
    /// Pointer to (x, y), gliding over `ms` milliseconds (0 = jump).
    Move { x: f64, y: f64, ms: u32 },
    Down { button: Button },
    Up { button: Button },
    Click { button: Button, count: u32 },
    /// Mouse wheel, in notches (positive = down / right).
    Scroll { dx: f64, dy: f64 },
    /// Smooth (touchpad-style) scroll, in pixels.
    ScrollPx { dx: f64, dy: f64 },
    /// Press the keys in order, release in reverse: `ctrl+shift+t`.
    Key { keys: Vec<String> },
    KeyDown { key: String },
    KeyUp { key: String },
    Type { text: String },
    TouchDown { id: u32, x: f64, y: f64 },
    TouchMove { id: u32, x: f64, y: f64, ms: u32 },
    TouchUp { id: u32 },
    Tap { x: f64, y: f64 },
    /// Touchpad pinch at the pointer: final scale, rotation in degrees (clockwise).
    Pinch { scale: f64, rotate: f64 },
    /// Touchpad swipe at the pointer, in pixels.
    Swipe { dx: f64, dy: f64, fingers: u32 },
    /// Coordinates are relative to this surface's top-left from now on; None = output.
    On { surface: Option<u32> },
    Sleep { ms: u32 },
}

/// Help for `guilab input`: the whole grammar, for agents with no other docs.
pub const HELP: &str = "\
Send input to the session. Needs a lock (`guilab lock`, then -t TOKEN).
Actions run in order in one command, so held buttons and keys carry across them.

Coordinates are output pixels, the same as in screenshots and `tree`, at any scale.
Decimals are fine. Positive Y is down.

Pointer:
  move X Y [MS]          move the pointer; with MS, glide there over MS milliseconds
  down [BTN]             press a button (BTN: left (default), right, middle, back, forward)
  up [BTN]               release it
  click [BTN] [N]        press and release N times (2 = double-click)
  scroll DX DY           mouse wheel notches (DY 3 = three notches down; negative = up/left)
  scrollpx DX DY         smooth touchpad-style scroll by pixels (toolkits scale it: GTK ~2.5x)
Keyboard (key names: a, A, 1, slash, Return, Escape, Tab, BackSpace, Delete, space, Up,
Down, Left, Right, Home, End, Page_Up, Page_Down, F1-F24, ctrl, shift, alt, super, or
any xkb keysym name; case-insensitive except single letters):
  key CHORD              tap a key or chord: key Return, key ctrl+shift+t, key alt+F4
  keydown KEY / keyup KEY   hold / release one key
  type TEXT              type any Unicode text (\\n = Enter, \\t = Tab)
Touch (ID: any number naming a finger):
  touch down ID X Y      put a finger down
  touch move ID X Y [MS] move it; with MS, glide over MS milliseconds
  touch up ID            lift it
  tap X Y                one-finger tap
Touchpad gestures, sent to the window under the pointer:
  pinch SCALE [DEG]      pinch to SCALE (2 = zoom in 2x, 0.5 = out), optionally rotating
  swipe DX DY [FINGERS]  swipe by pixels (default 3 fingers)
Other:
  on ID                  later coordinates are relative to surface ID's top-left (IDs from
                         `guilab tree`); `on output` switches back. Events still go to
                         whatever is on top at that point.
  sleep MS               wait (e.g. for a menu to open before clicking in it)

Key repeat is off: a held key never repeats. Clicking a window focuses and raises it.

Examples:
  guilab -t TOKEN input move 200 150 click
  guilab -t TOKEN input click right sleep 300 move 230 180 click
  guilab -t TOKEN input move 10 10 down move 300 200 200 up      (drag, gliding 200 ms)
  guilab -t TOKEN input key ctrl+l type 'héllo wörld →' key Return
  guilab -t TOKEN input on 7 move 20 10 click 2                  (double-click in surface 7)
Prints where the pointer ended up, the surface under it and the keyboard focus.
Take a screenshot afterwards to see the effect (it waits for the app to redraw).";

const ACTIONS: &str = "move, down, up, click, scroll, scrollpx, key, keydown, keyup, type, touch, tap, pinch, swipe, on, sleep";

/// Parses CLI words into actions. Errors name the action and say what was expected.
pub fn parse(words: &[String]) -> Result<Vec<Action>, String> {
    let mut p = Parser { words, i: 0 };
    let mut out = Vec::new();
    while p.i < words.len() {
        let start = p.i;
        let word = p.next().unwrap();
        let act = p.action(word).map_err(|e| {
            let shown: Vec<&str> = words[start..p.i.min(words.len())].iter().map(String::as_str).collect();
            format!("input action {} `{}`: {e}", out.len() + 1, shown.join(" "))
        })?;
        out.push(act);
    }
    if out.is_empty() {
        return Err(format!("no actions given; see `guilab input --help` (actions: {ACTIONS})"));
    }
    Ok(out)
}

struct Parser<'a> {
    words: &'a [String],
    i: usize,
}

impl<'a> Parser<'a> {
    fn next(&mut self) -> Option<&'a str> {
        let w = self.words.get(self.i)?;
        self.i += 1;
        Some(w)
    }

    fn peek(&self) -> Option<&'a str> {
        self.words.get(self.i).map(String::as_str)
    }

    /// A required number.
    fn num(&mut self, what: &str) -> Result<f64, String> {
        let w = self.next().ok_or_else(|| format!("missing {what}"))?;
        match w.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(v),
            _ => {
                self.i -= 1;
                Err(format!("{what} must be a number, got '{w}'"))
            }
        }
    }

    /// An optional trailing number: taken only if the next word parses as one.
    fn opt_num(&mut self) -> Option<f64> {
        let v = self.peek()?.parse::<f64>().ok().filter(|v| v.is_finite())?;
        self.i += 1;
        Some(v)
    }

    fn uint(&mut self, what: &str, max: u32) -> Result<u32, String> {
        let v = self.num(what)?;
        if v < 0.0 || v.fract() != 0.0 || v > max as f64 {
            return Err(format!("{what} must be a whole number from 0 to {max}"));
        }
        Ok(v as u32)
    }

    fn opt_ms(&mut self) -> Result<u32, String> {
        match self.opt_num() {
            None => Ok(0),
            Some(v) if (0.0..=60_000.0).contains(&v) => Ok(v as u32),
            Some(_) => Err("milliseconds must be 0 to 60000".into()),
        }
    }

    fn opt_button(&mut self) -> Button {
        match self.peek().and_then(Button::parse) {
            Some(b) => {
                self.i += 1;
                b
            }
            None => Button::Left,
        }
    }

    fn word(&mut self, what: &str) -> Result<&'a str, String> {
        self.next().ok_or_else(|| format!("missing {what}"))
    }

    fn action(&mut self, word: &str) -> Result<Action, String> {
        Ok(match word {
            "move" => Action::Move { x: self.num("X")?, y: self.num("Y")?, ms: self.opt_ms()? },
            "down" => Action::Down { button: self.opt_button() },
            "up" => Action::Up { button: self.opt_button() },
            "click" => {
                let button = self.opt_button();
                let count = match self.opt_num() {
                    None => 1,
                    Some(n) if n.fract() == 0.0 && (1.0..=10.0).contains(&n) => n as u32,
                    Some(_) => return Err("click count must be 1 to 10".into()),
                };
                Action::Click { button, count }
            }
            "scroll" => Action::Scroll { dx: self.num("DX")?, dy: self.num("DY")? },
            "scrollpx" => Action::ScrollPx { dx: self.num("DX")?, dy: self.num("DY")? },
            "key" => {
                let chord = self.word("key or chord, e.g. Return or ctrl+c")?;
                // "ctrl++" means ctrl and the plus key.
                let keys: Vec<String> = match chord.strip_suffix("++") {
                    Some(mods) => mods.split('+').chain(["plus"]).map(str::to_string).collect(),
                    None => chord.split('+').map(str::to_string).collect(),
                };
                if keys.iter().any(String::is_empty) {
                    return Err("empty key name in chord".into());
                }
                Action::Key { keys }
            }
            "keydown" => Action::KeyDown { key: self.word("KEY")?.to_string() },
            "keyup" => Action::KeyUp { key: self.word("KEY")?.to_string() },
            "type" => Action::Type { text: unescape(self.word("TEXT (quote it if it has spaces)")?) },
            "touch" => match self.word("down, move or up")? {
                "down" => Action::TouchDown { id: self.uint("ID", 1000)?, x: self.num("X")?, y: self.num("Y")? },
                "move" => Action::TouchMove { id: self.uint("ID", 1000)?, x: self.num("X")?, y: self.num("Y")?, ms: self.opt_ms()? },
                "up" => Action::TouchUp { id: self.uint("ID", 1000)? },
                w => {
                    self.i -= 1;
                    return Err(format!("expected touch down|move|up, got '{w}'"));
                }
            },
            "tap" => Action::Tap { x: self.num("X")?, y: self.num("Y")? },
            "pinch" => {
                let scale = self.num("SCALE")?;
                if scale <= 0.0 {
                    return Err("SCALE must be positive (2 = zoom in, 0.5 = zoom out)".into());
                }
                Action::Pinch { scale, rotate: self.opt_num().unwrap_or(0.0) }
            }
            "swipe" => {
                let (dx, dy) = (self.num("DX")?, self.num("DY")?);
                let fingers = match self.opt_num() {
                    None => 3,
                    Some(n) if n.fract() == 0.0 && (1.0..=5.0).contains(&n) => n as u32,
                    Some(_) => return Err("FINGERS must be 1 to 5".into()),
                };
                Action::Swipe { dx, dy, fingers }
            }
            "on" => {
                let w = self.word("surface ID (from guilab tree) or 'output'")?;
                if w == "output" {
                    Action::On { surface: None }
                } else {
                    match w.parse() {
                        Ok(id) => Action::On { surface: Some(id) },
                        Err(_) => return Err(format!("expected a surface ID from `guilab tree` or 'output', got '{w}'")),
                    }
                }
            }
            "sleep" => Action::Sleep { ms: self.uint("MS", 60_000)? },
            _ => {
                let hint = if word.starts_with('-') && word.parse::<f64>().is_err() {
                    " (options like -t TOKEN go before the actions: guilab -t TOKEN input ...)"
                } else if word.parse::<f64>().is_ok() {
                    " (a stray number: check the previous action's arguments)"
                } else if Button::parse(word).is_some() {
                    " (buttons go after down/up/click: click right)"
                } else {
                    ""
                };
                return Err(format!("unknown action{hint}; actions: {ACTIONS}; see `guilab input --help`"));
            }
        })
    }
}

/// `\n`, `\t` and `\\` in `type` text.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn n(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") }
}

impl fmt::Display for Action {
    /// Canonical CLI form, used in errors (and later in the event log).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |ms: u32| if ms > 0 { format!(" {ms}") } else { String::new() };
        match self {
            Action::Move { x, y, ms: m } => write!(f, "move {} {}{}", n(*x), n(*y), ms(*m)),
            Action::Down { button } => write!(f, "down {}", button.name()),
            Action::Up { button } => write!(f, "up {}", button.name()),
            Action::Click { button, count } => write!(f, "click {} {count}", button.name()),
            Action::Scroll { dx, dy } => write!(f, "scroll {} {}", n(*dx), n(*dy)),
            Action::ScrollPx { dx, dy } => write!(f, "scrollpx {} {}", n(*dx), n(*dy)),
            Action::Key { keys } => write!(f, "key {}", keys.join("+")),
            Action::KeyDown { key } => write!(f, "keydown {key}"),
            Action::KeyUp { key } => write!(f, "keyup {key}"),
            Action::Type { text } => write!(f, "type {text:?}"),
            Action::TouchDown { id, x, y } => write!(f, "touch down {id} {} {}", n(*x), n(*y)),
            Action::TouchMove { id, x, y, ms: m } => write!(f, "touch move {id} {} {}{}", n(*x), n(*y), ms(*m)),
            Action::TouchUp { id } => write!(f, "touch up {id}"),
            Action::Tap { x, y } => write!(f, "tap {} {}", n(*x), n(*y)),
            Action::Pinch { scale, rotate } => write!(f, "pinch {} {}", n(*scale), n(*rotate)),
            Action::Swipe { dx, dy, fingers } => write!(f, "swipe {} {} {fingers}", n(*dx), n(*dy)),
            Action::On { surface: Some(id) } => write!(f, "on {id}"),
            Action::On { surface: None } => write!(f, "on output"),
            Action::Sleep { ms } => write!(f, "sleep {ms}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Vec<Action>, String> {
        parse(&s.split(' ').map(str::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn parses_chains() {
        let a = p("move 10 20.5 click right 2 click move 1 2 300 scroll 0 -3 key ctrl+shift+t key ctrl++ sleep 50").unwrap();
        assert_eq!(a[0], Action::Move { x: 10.0, y: 20.5, ms: 0 });
        assert_eq!(a[1], Action::Click { button: Button::Right, count: 2 });
        assert_eq!(a[2], Action::Click { button: Button::Left, count: 1 });
        assert_eq!(a[3], Action::Move { x: 1.0, y: 2.0, ms: 300 });
        assert_eq!(a[4], Action::Scroll { dx: 0.0, dy: -3.0 });
        assert_eq!(a[5], Action::Key { keys: vec!["ctrl".into(), "shift".into(), "t".into()] });
        assert_eq!(a[6], Action::Key { keys: vec!["ctrl".into(), "plus".into()] });
        assert_eq!(a[7], Action::Sleep { ms: 50 });
        let a = p("touch down 1 5 5 touch move 1 9 9 100 touch up 1 on 7 tap 1 1 on output pinch 2 swipe 10 0").unwrap();
        assert_eq!(a[1], Action::TouchMove { id: 1, x: 9.0, y: 9.0, ms: 100 });
        assert_eq!(a[3], Action::On { surface: Some(7) });
        assert_eq!(a[5], Action::On { surface: None });
        assert_eq!(a[7], Action::Swipe { dx: 10.0, dy: 0.0, fingers: 3 });
        let t = parse(&["type".into(), "a b\\nc".into()]).unwrap();
        assert_eq!(t[0], Action::Type { text: "a b\nc".into() });
    }

    #[test]
    fn errors_say_where() {
        let e = p("move 10 click").unwrap_err();
        assert!(e.contains("action 1 `move 10`") && e.contains("Y must be a number, got 'click'"), "{e}");
        let e = p("click clik").unwrap_err();
        assert!(e.contains("action 2 `clik`: unknown action"), "{e}");
        let e = p("move 1 2 3 4").unwrap_err();
        assert!(e.contains("action 2 `4`") && e.contains("stray number"), "{e}");
        let e = p("right").unwrap_err();
        assert!(e.contains("click right"), "{e}");
        assert!(parse(&[]).unwrap_err().contains("no actions"));
    }

    #[test]
    fn display_roundtrips() {
        let s = "move 1 2.5 100 click right 2 key ctrl+c touch down 3 4 5 on 7 sleep 10";
        let a = p(s).unwrap();
        let shown: Vec<String> = a.iter().map(|a| a.to_string()).collect();
        assert_eq!(p(&shown.join(" ")).unwrap(), a);
    }
}
