//! Compact text for events, made for agents reading them cold: `poll` results, the events
//! attached to command replies, `poll --follow` lines and `log`. Human input runs are
//! summarized ("clicked at 640,410; typed "hello"; pressed Return").

use crate::events::{Annotation, Delivery, Event, EventData, Tier};

/// Formats times in a fixed UTC offset (the CLI passes the local one).
#[derive(Debug, Clone, Copy, Default)]
pub struct Fmt {
    /// Seconds east of UTC.
    pub tz: i64,
}

/// More human input phrases than this in one run are elided in the middle.
const MAX_PHRASES: usize = 16;

fn signal_name(s: i32) -> Option<&'static str> {
    Some(match s {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return None,
    })
}

fn secs(ms: i64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v:.1}") }
}

fn quote(s: &str) -> String {
    format!("{s:?}")
}

fn is_modifier(key: &str) -> bool {
    matches!(
        key,
        "Shift_L" | "Shift_R" | "Control_L" | "Control_R" | "Alt_L" | "Alt_R" | "Super_L" | "Super_R" | "Meta_L" | "Meta_R" | "ISO_Level3_Shift"
    )
}

fn mod_name(key: &str) -> &str {
    match key {
        "Shift_L" | "Shift_R" => "shift",
        "Control_L" | "Control_R" => "ctrl",
        "Alt_L" | "Alt_R" | "Meta_L" | "Meta_R" => "alt",
        "Super_L" | "Super_R" => "super",
        "ISO_Level3_Shift" => "altgr",
        k => k,
    }
}

fn clicked(button: &str, n: u32) -> String {
    let b = match button {
        "left" => String::new(),
        b => format!("{b}-"),
    };
    match n {
        1 => format!("{b}clicked"),
        2 => format!("{b}double-clicked"),
        3 => format!("{b}triple-clicked"),
        n => format!("{b}clicked {n} times"),
    }
}

fn at(x: i32, y: i32, window: Option<u32>) -> String {
    match window {
        Some(w) => format!("at {x},{y} in window {w}"),
        None => format!("at {x},{y}"),
    }
}

fn annotation(a: &Annotation) -> String {
    let mut s = match &a.color {
        Some(c) => format!("{c} text annotation"),
        None => "annotation".to_string(),
    };
    if let Some(i) = a.image {
        s += &format!(" on image {}", i + 1);
    }
    if let (Some(x), Some(y)) = (a.x, a.y) {
        s += &format!(" at pixel {x},{y}");
    }
    format!("{s}: {}", quote(&a.text))
}

/// Summarizes a run of human input events into phrases.
fn input_phrases(events: &[&Event]) -> Vec<String> {
    struct Press {
        button: String,
        x: i32,
        y: i32,
        window: Option<u32>,
        to: Option<(i32, i32)>,
    }
    let mut out: Vec<String> = Vec::new();
    let mut typed = String::new();
    let mut mods: Vec<String> = Vec::new();
    let mut press: Option<Press> = None;
    // Last click: (button, x, y, t, count, index in out), to merge double clicks.
    let mut last_click: Option<(String, i32, i32, u64, u32, usize)> = None;
    let flush = |typed: &mut String, out: &mut Vec<String>| {
        if !typed.is_empty() {
            out.push(format!("typed {}", quote(typed)));
            typed.clear();
        }
    };
    for e in events {
        match &e.data {
            EventData::Motion { path } => {
                let (Some(a), Some(b)) = (path.first(), path.last()) else { continue };
                if let Some(p) = &mut press {
                    p.to = Some((b[0], b[1]));
                    continue;
                }
                flush(&mut typed, &mut out);
                if path.len() == 1 {
                    out.push(format!("moved pointer to {},{}", b[0], b[1]));
                } else {
                    out.push(format!("moved pointer {},{} → {},{} ({})", a[0], a[1], b[0], b[1], secs((b[2] - a[2]) as i64)));
                }
            }
            EventData::Button { button, pressed: true, x, y, window } => {
                flush(&mut typed, &mut out);
                press = Some(Press { button: button.clone(), x: *x, y: *y, window: *window, to: None });
            }
            EventData::Button { button, pressed: false, x, y, window } => {
                flush(&mut typed, &mut out);
                match press.take() {
                    Some(p) if &p.button == button => {
                        let (ex, ey) = p.to.unwrap_or((p.x, p.y));
                        if (ex - p.x).abs() + (ey - p.y).abs() > 4 {
                            let b = if button == "left" { String::new() } else { format!(" ({button} button)") };
                            out.push(format!("dragged from {},{} to {},{}{b}", p.x, p.y, ex, ey));
                            last_click = None;
                            continue;
                        }
                        let n = match &last_click {
                            Some((b, lx, ly, lt, n, i))
                                if b == button && *i + 1 == out.len() && e.t.saturating_sub(*lt) < 500 && (lx - p.x).abs() + (ly - p.y).abs() <= 4 =>
                            {
                                out.pop();
                                n + 1
                            }
                            _ => 1,
                        };
                        out.push(format!("{} {}", clicked(button, n), at(p.x, p.y, p.window)));
                        last_click = Some((button.clone(), p.x, p.y, e.t, n, out.len() - 1));
                    }
                    _ => out.push(format!("released {button} button {}", at(*x, *y, *window))),
                }
            }
            EventData::Key { key, pressed, text } => {
                if is_modifier(key) {
                    let m = mod_name(key).to_string();
                    if *pressed {
                        if !mods.contains(&m) {
                            mods.push(m);
                        }
                    } else {
                        mods.retain(|x| *x != m);
                    }
                    continue;
                }
                if !pressed {
                    continue;
                }
                let chord = mods.iter().any(|m| m != "shift" && m != "altgr");
                match text {
                    Some(t) if !chord && !t.chars().any(char::is_control) => typed.push_str(t),
                    _ => {
                        flush(&mut typed, &mut out);
                        let mut c: Vec<&str> = mods.iter().map(String::as_str).collect();
                        c.push(key);
                        out.push(format!("pressed {}", c.join("+")));
                    }
                }
                last_click = None;
            }
            EventData::Scroll { dx, dy, x, y, smooth } => {
                flush(&mut typed, &mut out);
                let unit = if *smooth { "px" } else { " notches" };
                let mut parts = Vec::new();
                if *dy != 0.0 {
                    parts.push(format!("{} {}{unit}", if *dy > 0.0 { "down" } else { "up" }, num(dy.abs())));
                }
                if *dx != 0.0 {
                    parts.push(format!("{} {}{unit}", if *dx > 0.0 { "right" } else { "left" }, num(dx.abs())));
                }
                out.push(format!("scrolled {} at {x},{y}", parts.join(" ")));
            }
            EventData::Touch { finger, down, x, y } => {
                flush(&mut typed, &mut out);
                out.push(format!("finger {finger} {} at {x},{y}", if *down { "down" } else { "up" }));
            }
            EventData::Gesture { gesture, fingers } => {
                flush(&mut typed, &mut out);
                out.push(format!("{fingers}-finger {gesture}"));
            }
            _ => {}
        }
    }
    flush(&mut typed, &mut out);
    if let Some(p) = press {
        out.push(format!("pressed {} button {} (still held)", p.button, at(p.x, p.y, p.window)));
    }
    if !mods.is_empty() {
        out.push(format!("holding {}", mods.join("+")));
    }
    if out.len() > MAX_PHRASES {
        let n = out.len() - MAX_PHRASES;
        out.splice(MAX_PHRASES / 2..MAX_PHRASES / 2 + n, [format!("…({n} more)…")]);
    }
    out
}

impl Fmt {
    /// `HH:MM:SS` in the configured offset.
    pub fn clock(&self, t: u64) -> String {
        let s = ((t / 1000) as i64 + self.tz).rem_euclid(86400);
        format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }

    /// One event as one line, without its time. Human input is described per event.
    pub fn describe(&self, e: &Event) -> String {
        match &e.data {
            EventData::Command { cmd } => format!("agent ran: guilab {cmd}"),
            EventData::Say { text, images } => {
                let mut s = format!("agent said: {}", quote(text));
                for (i, p) in images.iter().enumerate() {
                    s += &format!(" [image {}: {}]", i + 1, p.display());
                }
                s
            }
            EventData::Message { text, images, annotations } => {
                let mut s = format!("human message: {}", quote(text));
                for (i, p) in images.iter().enumerate() {
                    s += &format!(" [image {}: {}]", i + 1, p.display());
                }
                for a in annotations {
                    s += &format!(" [{}]", annotation(a));
                }
                s
            }
            EventData::Motion { path } => match (path.first(), path.last()) {
                (Some(a), Some(b)) => format!(
                    "human moved pointer {},{} → {},{} over {} ({} points)",
                    a[0],
                    a[1],
                    b[0],
                    b[1],
                    secs((b[2] - a[2]) as i64),
                    path.len()
                ),
                _ => "human moved pointer".into(),
            },
            EventData::Button { button, pressed, x, y, window } => {
                format!("human {} {button} button {}", if *pressed { "pressed" } else { "released" }, at(*x, *y, *window))
            }
            EventData::Key { key, pressed, text } => {
                let t = match text {
                    Some(t) if *pressed && !t.chars().any(char::is_control) => format!(" ({})", quote(t)),
                    _ => String::new(),
                };
                format!("human key {key} {}{t}", if *pressed { "down" } else { "up" })
            }
            EventData::Touch { .. } | EventData::Gesture { .. } | EventData::Scroll { .. } => {
                format!("human {}", input_phrases(&[e]).join("; "))
            }
            EventData::AppStart { id, argv } => format!("app {id} started: {}", argv.join(" ")),
            EventData::AppExit { id, argv, code, signal, killed, log } => {
                let name = argv.first().map(|a| a.rsplit('/').next().unwrap_or(a)).unwrap_or("?");
                let how = match (code, signal) {
                    _ if *killed => return format!("app {id} ({name}) ended after guilab kill"),
                    (Some(0), _) => "exited normally (code 0)".to_string(),
                    (Some(c), _) => format!("exited with code {c}"),
                    (None, Some(s @ (4 | 6 | 7 | 8 | 11))) => format!("crashed ({})", signal_name(*s).unwrap_or("?")),
                    (None, Some(s)) => match signal_name(*s) {
                        Some(n) => format!("was killed by {n}"),
                        None => format!("was killed by signal {s}"),
                    },
                    (None, None) => "exited".to_string(),
                };
                format!("app {id} ({name}) {how}; log: {}", log.display())
            }
            EventData::WindowMap { id, window, app_id, title } => {
                let mut s = format!("window {id} opened ({window}");
                if let Some(a) = app_id {
                    s += &format!(", app_id {a}");
                }
                s.push(')');
                if let Some(t) = title {
                    s += &format!(": {}", quote(t));
                }
                s
            }
            EventData::WindowUnmap { id, title } => match title {
                Some(t) => format!("window {id} closed: {}", quote(t)),
                None => format!("window {id} closed"),
            },
            EventData::Title { id, title } => format!("window {id} title is now {}", quote(title.as_deref().unwrap_or(""))),
            EventData::Output { width, height, scale } => {
                if *scale == 1.0 {
                    format!("output is now {width}x{height}")
                } else {
                    format!("output is now {width}x{height} scale {scale}")
                }
            }
            EventData::Lock { forced: false } => "session locked".into(),
            EventData::Lock { forced: true } => "lock taken over (guilab lock --force)".into(),
            EventData::Unlock => "session unlocked".into(),
            EventData::Revoke => {
                "the human revoked the agent's lock: stop sending input; `guilab lock` again only once they hand control back".into()
            }
            EventData::Share { on: true } => "the human shared the session: they can use it while you hold the lock".into(),
            EventData::Share { on: false } => "the human stopped sharing the session".into(),
            EventData::Screenshot { path } => format!("screenshot {}", path.display()),
            EventData::Expiring { secs } => {
                format!("the session (and its apps) ends in {secs}s from inactivity (a waiting poll doesn't count); run any guilab command (e.g. guilab ps) to keep it")
            }
        }
    }

    /// `guilab log` line: `#SEQ HH:MM:SS description`.
    pub fn log_line(&self, e: &Event) -> String {
        format!("#{} {} {}", e.seq, self.clock(e.t), self.describe(e))
    }

    /// Lines for events in order, merging consecutive human input into one line each.
    fn lines(&self, events: &[&Event], indent: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < events.len() {
            let e = events[i];
            if e.data.is_input() {
                let j = events[i..].iter().position(|e| !e.data.is_input()).map_or(events.len(), |n| i + n);
                let end = events[i..j].iter().map(|e| match &e.data {
                    EventData::Motion { path } => e.t + path.last().map_or(0, |p| p[2].max(0) as u64),
                    _ => e.t,
                });
                let (a, b) = (self.clock(e.t), self.clock(end.max().unwrap_or(e.t)));
                let when = if a == b { a } else { format!("{a}-{b}") };
                out.push(format!("{indent}{when} human: {}", input_phrases(&events[i..j]).join("; ")));
                i = j;
                continue;
            }
            out.push(format!("{indent}{} {}", self.clock(e.t), self.describe(e)));
            i += 1;
        }
        out
    }

    /// A wake event in full, on several lines if it has attachments.
    fn wake_lines(&self, e: &Event) -> Vec<String> {
        match &e.data {
            EventData::Message { text, images, annotations } => {
                let mut v = vec![format!("{} human message: {}", self.clock(e.t), quote(text))];
                for (i, p) in images.iter().enumerate() {
                    v.push(format!("  image {}: {}", i + 1, p.display()));
                }
                v.extend(annotations.iter().map(|a| format!("  {}", annotation(a))));
                if annotations.iter().any(|a| a.color.is_some()) {
                    v.push("  (text annotations aren't drawn in the image; the pixel is the text's top-left, where the human typed it, usually next to the mark it's about)".into());
                }
                v.push("  (reply with: guilab say \"...\")".into());
                v
            }
            _ => vec![format!("{} {}", self.clock(e.t), self.describe(e))],
        }
    }

    /// "since your previous guilab command (HH:MM:SS)".
    pub fn since(&self, d: &Delivery) -> String {
        match d.since {
            Some(t) => format!("since your previous guilab command at {}", self.clock(t)),
            None => "since the session started".into(),
        }
    }

    /// A delivery as lines, oldest first: wake events in full, human input runs merged.
    pub fn delivery(&self, d: &Delivery) -> Vec<String> {
        let mut out = Vec::new();
        let mut run: Vec<&Event> = Vec::new();
        for e in &d.events {
            if e.tier == Some(Tier::Wake) {
                out.extend(self.lines(&run, ""));
                run.clear();
                out.extend(self.wake_lines(e));
            } else {
                run.push(e);
            }
        }
        out.extend(self.lines(&run, ""));
        out
    }

    /// `poll --follow`: one line per wake event; passive events are appended to the last.
    pub fn follow_lines(&self, d: &Delivery) -> Vec<String> {
        let mut out: Vec<String> = d
            .events
            .iter()
            .filter(|e| e.tier == Some(Tier::Wake))
            .map(|e| format!("{} {}", self.clock(e.t), self.describe(e)))
            .collect();
        let rest: Vec<&Event> = d.events.iter().filter(|e| e.tier != Some(Tier::Wake)).collect();
        if !rest.is_empty() {
            let also = self.lines(&rest, "").join("; ");
            match out.last_mut() {
                Some(l) => *l += &format!(" (meanwhile: {also})"),
                None => out.push(also),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Tier;

    fn ev(t: u64, data: EventData) -> Event {
        Event { seq: 0, t, data, tier: Some(Tier::Passive), by: None }
    }
    fn key(k: &str, pressed: bool, text: Option<&str>) -> EventData {
        EventData::Key { key: k.into(), pressed, text: text.map(Into::into) }
    }
    fn btn(pressed: bool, x: i32) -> EventData {
        EventData::Button { button: "left".into(), pressed, x, y: 5, window: Some(3) }
    }

    #[test]
    fn input_summary() {
        let evs = [
            ev(0, EventData::Motion { path: vec![[1, 2, 0], [30, 40, 800]] }),
            ev(900, btn(true, 30)),
            ev(950, btn(false, 30)),
            ev(1000, btn(true, 30)),
            ev(1050, btn(false, 30)),
            ev(2000, key("h", true, Some("h"))),
            ev(2001, key("h", false, None)),
            ev(2002, key("Shift_L", true, None)),
            ev(2003, key("I", true, Some("I"))),
            ev(2004, key("Shift_L", false, None)),
            ev(2005, key("Return", true, Some("\r"))),
            ev(2006, key("Control_L", true, None)),
            ev(2007, key("s", true, Some("s"))),
            ev(3000, btn(true, 100)),
            ev(3100, EventData::Motion { path: vec![[200, 5, 0]] }),
            ev(3200, btn(false, 200)),
        ];
        let refs: Vec<&Event> = evs.iter().collect();
        assert_eq!(
            input_phrases(&refs),
            [
                "moved pointer 1,2 → 30,40 (0.8s)",
                "double-clicked at 30,5 in window 3",
                "typed \"hI\"",
                "pressed Return",
                "pressed ctrl+s",
                "dragged from 100,5 to 200,5",
                "holding ctrl",
            ]
        );
    }

    #[test]
    fn delivery_text() {
        let f = Fmt { tz: 3600 };
        let mut msg = ev(
            7_200_000,
            EventData::Message {
                text: "fix this".into(),
                images: vec!["/s/msg-1.png".into()],
                annotations: vec![Annotation { text: "here".into(), image: Some(0), x: Some(5), y: Some(6), color: None }],
            },
        );
        msg.tier = Some(Tier::Wake);
        let d = Delivery {
            since: Some(3_600_000),
            events: vec![ev(3_700_000, EventData::Title { id: 4, title: Some("Hi".into()) }), msg],
        };
        assert_eq!(
            f.delivery(&d),
            [
                "02:01:40 window 4 title is now \"Hi\"",
                "03:00:00 human message: \"fix this\"",
                "  image 1: /s/msg-1.png",
                "  annotation on image 1 at pixel 5,6: \"here\"",
                "  (reply with: guilab say \"...\")",
            ]
        );
        assert_eq!(
            f.follow_lines(&d),
            ["03:00:00 human message: \"fix this\" [image 1: /s/msg-1.png] [annotation on image 1 at pixel 5,6: \"here\"] (meanwhile: 02:01:40 window 4 title is now \"Hi\")"]
        );
    }
}
