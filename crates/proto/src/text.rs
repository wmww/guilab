//! Compact text for events, made for agents reading them cold: `poll` results, the events
//! attached to command replies, `poll --follow` lines and `log`. Human input runs are
//! summarized ("clicked at 640,410; typed "hello"; pressed Return").

use crate::events::{Delivery, Event, EventData, ImageRef, Tier, Unread};

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

/// `[image w8dn: 6 elements]`, for one-line forms.
fn image_tag(i: &ImageRef) -> String {
    let n = i.elements();
    format!("[image {}: {n} element{}]", i.id, if n == 1 { "" } else { "s" })
}

/// A phrase of a human input run, before formatting. Moves and scrolls stay structured so
/// [`collapse`] can merge them.
enum Phrase {
    /// Pointer motion: from, to, ms (None once several runs merged).
    Move((i32, i32), (i32, i32), Option<i64>),
    Scroll { dx: f64, dy: f64, x: i32, y: i32, smooth: bool },
    Text(String),
}

impl Phrase {
    fn text(self) -> String {
        match self {
            Phrase::Move(a, b, ms) if a == b || ms == Some(0) => format!("moved pointer to {},{}", b.0, b.1),
            Phrase::Move(a, b, Some(ms)) => format!("moved pointer {},{} → {},{} ({})", a.0, a.1, b.0, b.1, secs(ms)),
            Phrase::Move(a, b, None) => format!("moved pointer {},{} → {},{}", a.0, a.1, b.0, b.1),
            Phrase::Scroll { dx, dy, x, y, smooth } => {
                let unit = if smooth { "px" } else { " notches" };
                let mut parts = Vec::new();
                if dy != 0.0 {
                    parts.push(format!("{} {}{unit}", if dy > 0.0 { "down" } else { "up" }, num(dy.abs())));
                }
                if dx != 0.0 {
                    parts.push(format!("{} {}{unit}", if dx > 0.0 { "right" } else { "left" }, num(dx.abs())));
                }
                if parts.is_empty() {
                    parts.push(format!("0{unit}"));
                }
                format!("scrolled {} at {x},{y}", parts.join(" "))
            }
            Phrase::Text(t) => t,
        }
    }
}

/// Drops pointer noise: a move is only kept where the pointer comes to rest at the end of
/// the run (clicks, drags and scrolls carry their own positions), and scrolls with only
/// moves between them add up.
fn collapse(phrases: Vec<Phrase>) -> Vec<Phrase> {
    let mut out: Vec<Phrase> = Vec::new();
    let mut tail: Option<Phrase> = None;
    for p in phrases {
        match p {
            Phrase::Move(a, b, ms) => {
                tail = Some(match tail.take() {
                    Some(Phrase::Move(a0, _, _)) => Phrase::Move(a0, b, None),
                    _ => Phrase::Move(a, b, ms),
                });
            }
            Phrase::Scroll { dx, dy, x, y, smooth } => {
                tail = None;
                if let Some(Phrase::Scroll { dx: pdx, dy: pdy, x: px, y: py, smooth: ps }) = out.last_mut()
                    && *ps == smooth
                {
                    (*pdx, *pdy, *px, *py) = (*pdx + dx, *pdy + dy, x, y);
                    continue;
                }
                out.push(Phrase::Scroll { dx, dy, x, y, smooth });
            }
            p => {
                tail = None;
                out.push(p);
            }
        }
    }
    out.extend(tail);
    out
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
    let mut out: Vec<Phrase> = Vec::new();
    let mut typed = String::new();
    let mut mods: Vec<String> = Vec::new();
    let mut press: Option<Press> = None;
    // Last click: (button, x, y, t, count, index in out), to merge double clicks.
    let mut last_click: Option<(String, i32, i32, u64, u32, usize)> = None;
    // Fingers down: finger -> (x, y, index in out), to merge down + up into one phrase.
    let mut fingers: Vec<(u32, i32, i32, usize)> = Vec::new();
    let flush = |typed: &mut String, out: &mut Vec<Phrase>| {
        if !typed.is_empty() {
            out.push(Phrase::Text(format!("typed {}", quote(typed))));
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
                out.push(Phrase::Move((a[0], a[1]), (b[0], b[1]), Some((b[2] - a[2]) as i64)));
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
                            out.push(Phrase::Text(format!("dragged from {},{} to {},{}{b}", p.x, p.y, ex, ey)));
                            last_click = None;
                            continue;
                        }
                        // Moves between the clicks of a double click don't separate them.
                        let n = match &last_click {
                            Some((b, lx, ly, lt, n, i))
                                if b == button
                                    && out[*i + 1..].iter().all(|p| matches!(p, Phrase::Move(..)))
                                    && e.t.saturating_sub(*lt) < 500
                                    && (lx - p.x).abs() + (ly - p.y).abs() <= 4 =>
                            {
                                out.truncate(*i);
                                n + 1
                            }
                            _ => 1,
                        };
                        out.push(Phrase::Text(format!("{} {}", clicked(button, n), at(p.x, p.y, p.window))));
                        last_click = Some((button.clone(), p.x, p.y, e.t, n, out.len() - 1));
                    }
                    _ => out.push(Phrase::Text(format!("released {button} button {}", at(*x, *y, *window)))),
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
                        out.push(Phrase::Text(format!("pressed {}", c.join("+"))));
                    }
                }
                last_click = None;
            }
            EventData::Scroll { dx, dy, x, y, smooth } => {
                flush(&mut typed, &mut out);
                out.push(Phrase::Scroll { dx: *dx, dy: *dy, x: *x, y: *y, smooth: *smooth });
            }
            EventData::Touch { finger, down: true, x, y } => {
                flush(&mut typed, &mut out);
                fingers.retain(|f| f.0 != *finger);
                fingers.push((*finger, *x, *y, out.len()));
                out.push(Phrase::Text(format!("finger {finger} down at {x},{y} (still down)")));
            }
            EventData::Touch { finger, down: false, x, y } => {
                flush(&mut typed, &mut out);
                match fingers.iter().position(|f| f.0 == *finger) {
                    Some(i) => {
                        let (_, x0, y0, at) = fingers.remove(i);
                        out[at] = Phrase::Text(if (x - x0).abs() + (y - y0).abs() <= 4 {
                            format!("tapped at {x0},{y0}")
                        } else {
                            format!("dragged finger {finger} from {x0},{y0} to {x},{y}")
                        });
                    }
                    None => out.push(Phrase::Text(format!("finger {finger} up at {x},{y}"))),
                }
            }
            EventData::Gesture { .. } => {
                flush(&mut typed, &mut out);
                out.push(Phrase::Text(gesture(&e.data)));
            }
            _ => {}
        }
    }
    flush(&mut typed, &mut out);
    let mut out: Vec<String> = collapse(out).into_iter().map(Phrase::text).collect();
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

fn gesture(d: &EventData) -> String {
    match d {
        EventData::Gesture { gesture, fingers, dx, dy, scale, cancelled } => {
            let mut s = format!("{fingers}-finger {gesture}");
            match scale {
                Some(k) if gesture == "pinch" => s += &format!(" to {}x", num((k * 10.0).round() / 10.0)),
                _ => s += &format!(" by {},{}", num(dx.round()), num(dy.round())),
            }
            if *cancelled {
                s += " (cancelled)";
            }
            s
        }
        _ => String::new(),
    }
}

/// The local UTC offset in seconds, now.
pub fn local_offset() -> i64 {
    // SAFETY: localtime_r writes only to `tm`.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff }
    }
}

impl Fmt {
    /// Formats in the local offset.
    pub fn local() -> Fmt {
        Fmt { tz: local_offset() }
    }

    /// `YYYYMMDD-HHMMSS` in the configured offset, for file names.
    pub fn file_stamp(&self, t: u64) -> String {
        let s = (t / 1000) as i64 + self.tz;
        let (days, secs) = (s.div_euclid(86400), s.rem_euclid(86400));
        // Civil date from days since 1970-01-01 (Howard Hinnant's days_from_civil, inverted).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    }

    /// `HH:MM:SS` in the configured offset.
    pub fn clock(&self, t: u64) -> String {
        let s = ((t / 1000) as i64 + self.tz).rem_euclid(86400);
        format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }

    /// One event as one line, without its time. Human input is described per event.
    pub fn describe(&self, e: &Event) -> String {
        match &e.data {
            EventData::Command { cmd } => format!("agent ran: guilab {cmd}"),
            EventData::Work { status } => format!("agent is working: {status}"),
            EventData::Done => "agent is done, waiting for the human".into(),
            EventData::AgentMessage { text, images } => {
                let mut s = format!("agent said: {}", quote(text));
                for i in images {
                    s += &format!(" [image {}]", i.id);
                }
                s
            }
            EventData::Message { text, images, screen, viewing } => {
                let mut s = format!("human message: {}", quote(text));
                for i in images {
                    s += &format!(" {}", image_tag(i));
                }
                if let Some(id) = screen {
                    s += &format!(" [their screen: image {id}]");
                }
                if let Some(id) = viewing {
                    s += &format!(" [open in their editor: image {id}]");
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
            EventData::Touch { finger, down, x, y } => format!("human finger {finger} {} at {x},{y}", if *down { "down" } else { "up" }),
            EventData::Gesture { .. } | EventData::Scroll { .. } => format!("human {}", input_phrases(&[e]).join("; ")),
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
            EventData::WindowUnmap { id, title, by } => {
                let by = match by.as_deref() {
                    Some("app") => " by its app",
                    Some("exit") => " (its app exited)",
                    Some("kill") => " (after guilab kill)",
                    _ => "",
                };
                match title {
                    Some(t) => format!("window {id} closed{by}: {}", quote(t)),
                    None => format!("window {id} closed{by}"),
                }
            }
            EventData::Title { id, title } => format!("window {id} title is now {}", quote(title.as_deref().unwrap_or(""))),
            EventData::Output { width, height, scale, human } => {
                let by = if *human { " (the human resized it)" } else { "" };
                if *scale == 1.0 {
                    format!("output is now {width}x{height}{by}")
                } else {
                    format!("output is now {width}x{height} scale {scale}{by}")
                }
            }
            EventData::Lock { forced: false } => "session locked".into(),
            EventData::Lock { forced: true } => "lock taken over (guilab lock --force)".into(),
            EventData::Unlock => "session unlocked".into(),
            EventData::Revoke => {
                "the human revoked the agent's lock: stop sending input; `guilab lock` again only once they hand control back".into()
            }
            EventData::Screenshot { image, path } => format!("screenshot: image {image}, {}", path.display()),
            EventData::ImageMade { image } => format!("agent made image {image}"),
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
            EventData::Message { text, images, screen, viewing } => {
                let mut v = vec![format!("{} human message: {}", self.clock(e.t), quote(text))];
                for i in images {
                    v.extend(i.listing.lines().map(|l| format!("  {l}")));
                    v.push(match i.marks {
                        Some(_) => format!("  (see what the marks point at: {})", i.fetch()),
                        None => format!("  (see it: {})", i.fetch()),
                    });
                }
                if let Some(id) = screen {
                    v.push(format!("  (their screen as they sent it, with their pointer: image {id}; see it: guilab image {id})"));
                }
                if let Some(id) = viewing {
                    v.push(format!("  (they had image {id} open in their editor, not attached; see it: guilab image {id})"));
                }
                v.push("  (you're marked working on it; reply when finished with: guilab done \"...\")".into());
                v
            }
            _ => vec![format!("{} {}", self.clock(e.t), self.describe(e))],
        }
    }

    /// The notice on command replies while human messages are unread.
    pub fn unread(&self, u: &Unread) -> String {
        let what = if u.count == 1 { "a human message".to_string() } else { format!("{} human messages", u.count) };
        format!("[guilab: {what} waiting since {}; read it now: guilab poll]", self.clock(u.first))
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
    fn pointer_noise_collapses() {
        let mv = |t, a: i32, b: i32| ev(t, EventData::Motion { path: vec![[a, a, 0], [b, b, 300]] });
        let sc = |t, dy| ev(t, EventData::Scroll { dx: 0.0, dy, x: 7, y: 8, smooth: false });
        let evs = [
            mv(0, 1, 2),
            sc(1000, 1.0),
            mv(2000, 3, 4),
            sc(3000, 2.0),
            mv(4000, 5, 6),
            ev(5000, EventData::Touch { finger: 1, down: true, x: 10, y: 10 }),
            ev(5100, EventData::Touch { finger: 1, down: false, x: 11, y: 10 }),
            ev(6000, EventData::Touch { finger: 2, down: true, x: 10, y: 10 }),
            ev(6100, EventData::Touch { finger: 2, down: false, x: 90, y: 10 }),
            mv(7000, 7, 8),
            mv(8000, 9, 10),
        ];
        let refs: Vec<&Event> = evs.iter().collect();
        assert_eq!(
            input_phrases(&refs),
            ["scrolled down 3 notches at 7,8", "tapped at 10,10", "dragged finger 2 from 10,10 to 90,10", "moved pointer 7,7 → 10,10"]
        );
        assert_eq!(input_phrases(&refs[..1]), ["moved pointer 1,1 → 2,2 (0.3s)"]);
        let g = |gesture: &str, scale| ev(0, EventData::Gesture { gesture: gesture.into(), fingers: 2, dx: 300.4, dy: -2.0, scale, cancelled: false });
        let (pinch, swipe) = (g("pinch", Some(1.83)), g("swipe", None));
        assert_eq!(input_phrases(&[&pinch, &swipe]), ["2-finger pinch to 1.8x", "2-finger swipe by 300,-2"]);
    }

    #[test]
    fn delivery_text() {
        assert_eq!(Fmt { tz: 0 }.file_stamp(0), "19700101-000000");
        // 2024-02-29 23:59:59 UTC, shown at +01:00.
        assert_eq!(Fmt { tz: 3600 }.file_stamp(1_709_251_199_000), "20240301-005959");
        let f = Fmt { tz: 3600 };
        let img = ImageRef {
            id: "w8dn".into(),
            size: [800, 600],
            listing: "image w8dn 800x600, parent h2kc (screenshot)\n  1 raster r7qa 0,0 800x600 (screenshot 14:04:50)\n  2 red box 10,10 50x40".into(),
            marks: Some([0, 0, 82, 72]),
        };
        let mut msg = ev(7_200_000, EventData::Message { text: "fix this".into(), images: vec![img], screen: None, viewing: None });
        msg.tier = Some(Tier::Wake);
        let d = Delivery {
            since: Some(3_600_000),
            events: vec![ev(3_700_000, EventData::Title { id: 4, title: Some("Hi".into()) }), msg],
            unread: None,
        };
        assert_eq!(
            f.delivery(&d),
            [
                "02:01:40 window 4 title is now \"Hi\"",
                "03:00:00 human message: \"fix this\"",
                "  image w8dn 800x600, parent h2kc (screenshot)",
                "    1 raster r7qa 0,0 800x600 (screenshot 14:04:50)",
                "    2 red box 10,10 50x40",
                "  (see what the marks point at: guilab image w8dn --crop '0,0 82x72')",
                "  (you're marked working on it; reply when finished with: guilab done \"...\")",
            ]
        );
        assert_eq!(
            f.follow_lines(&d),
            ["03:00:00 human message: \"fix this\" [image w8dn: 2 elements] (meanwhile: 02:01:40 window 4 title is now \"Hi\")"]
        );
    }
}
