//! Session event log types: what happened, and how it's delivered to agents.
//!
//! Every event has a delivery tier (see notes/design.md "Event log"):
//! - `wake`: ends a blocking `poll` / emits a `poll --follow` line;
//! - `passive`: rides along with the next delivery (poll result or any command's reply);
//! - `log`: only in `guilab log` (the agent's own commands, messages, status, screenshots).
//!
//! Human messages are special: they stay unread until a poll shows them (see `log.rs`).
//!
//! Defaults come from [`default_tier`]; `poll --wake/--passive` override them by kind or
//! group name ([`KINDS`], [`GROUPS`]).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Log,
    Passive,
    Wake,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Per-session sequence number (`guilab log --after N`).
    pub seq: u64,
    /// Unix time in ms.
    pub t: u64,
    #[serde(flatten)]
    pub data: EventData,
    /// Tier it was delivered with (deliveries only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// Delivery key of the client whose command caused it, so its own actions aren't
    /// echoed back to it. Never serialized: it may be a lock token.
    #[serde(skip)]
    pub by: Option<String>,
}

/// A text annotation on a human message: text, optionally placed on one of its images.
/// The viewer sends text the human typed onto a drawing this way, and doesn't draw it into
/// the image: agents read it more reliably as text, and baked-in text hides the UI under it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub text: String,
    /// Index into the message's `images` (0-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<usize>,
    /// Position in that image's pixels (= output pixels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// Color name of the text as drawn (e.g. "red"), to match it with same-colored marks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventData {
    /// An agent command, in CLI form without `guilab` (e.g. `input move 10 20 click`).
    Command { cmd: String },
    /// Agent -> human message (`guilab work -m` / `guilab done MSG`).
    AgentMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<PathBuf>,
    },
    /// The agent is working (`guilab work`, or a poll handed it a human message).
    Work { status: String },
    /// The agent finished and waits for the human (`guilab done`).
    Done,
    /// Human -> agent message. Images are files in the session dir.
    Message {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<PathBuf>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        annotations: Vec<Annotation>,
    },
    /// Human pointer motion, coalesced: points `[x, y, ms after t]` in output pixels.
    Motion { path: Vec<[i32; 3]> },
    /// Human mouse button. `window` = id of the window under the pointer (`tree` id).
    Button {
        button: String,
        pressed: bool,
        x: i32,
        y: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<u32>,
    },
    /// Human key. `key` = keysym name; `text` = the character it typed, if printable.
    Key {
        key: String,
        pressed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Human scroll, coalesced: wheel notches, or pixels if `smooth`.
    Scroll { dx: f64, dy: f64, x: i32, y: i32, smooth: bool },
    /// Human touch: finger down or up (at its last position).
    Touch { finger: u32, down: bool, x: i32, y: i32 },
    /// Human touchpad gesture (`swipe`, `pinch`), logged when it ends: total movement in
    /// output pixels, the final pinch scale.
    Gesture {
        gesture: String,
        fingers: u32,
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cancelled: bool,
    },
    AppStart { id: u32, argv: Vec<String> },
    /// `code` on a normal exit, `signal` if killed by one. `killed`: after `guilab kill`.
    AppExit {
        id: u32,
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        killed: bool,
        log: PathBuf,
    },
    /// A toplevel window appeared. `window` = `toplevel` or `x11` (`tree` kinds).
    WindowMap {
        id: u32,
        window: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    WindowUnmap {
        id: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Who closed it, if known: `app` (its app, still running, e.g. after a click on
        /// its close button), `exit` (its app exited), `kill` (after `guilab kill`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    Title {
        id: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Output {
        width: u32,
        height: u32,
        scale: f64,
        /// The human changed it in the viewer (else an agent's `output`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        human: bool,
    },
    Lock { forced: bool },
    Unlock,
    /// The human revoked the agent's lock (viewer).
    Revoke,
    /// The human may use the session while it's locked (viewer's Share button).
    Share { on: bool },
    Screenshot { path: PathBuf },
    /// The session ends from inactivity in `secs` unless a command arrives.
    Expiring { secs: u64 },
}

/// Every kind name, as used by `--wake`, `--passive` and `log --kind`.
pub const KINDS: &[&str] = &[
    "command", "agent_message", "work", "done", "message", "motion", "button", "key", "scroll", "touch", "gesture", "app_start", "app_exit",
    "window_map", "window_unmap", "title", "output", "lock", "unlock", "revoke", "share", "screenshot", "expiring",
];

/// Group names, each standing for several kinds.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("input", &["motion", "button", "key", "scroll", "touch", "gesture"]),
    ("window", &["window_map", "window_unmap", "title"]),
    ("app", &["app_start", "app_exit"]),
    ("agent", &["command", "agent_message", "work", "done", "screenshot"]),
    ("all", KINDS),
];

impl EventData {
    pub fn kind(&self) -> &'static str {
        match self {
            EventData::Command { .. } => "command",
            EventData::AgentMessage { .. } => "agent_message",
            EventData::Work { .. } => "work",
            EventData::Done => "done",
            EventData::Message { .. } => "message",
            EventData::Motion { .. } => "motion",
            EventData::Button { .. } => "button",
            EventData::Key { .. } => "key",
            EventData::Scroll { .. } => "scroll",
            EventData::Touch { .. } => "touch",
            EventData::Gesture { .. } => "gesture",
            EventData::AppStart { .. } => "app_start",
            EventData::AppExit { .. } => "app_exit",
            EventData::WindowMap { .. } => "window_map",
            EventData::WindowUnmap { .. } => "window_unmap",
            EventData::Title { .. } => "title",
            EventData::Output { .. } => "output",
            EventData::Lock { .. } => "lock",
            EventData::Unlock => "unlock",
            EventData::Revoke => "revoke",
            EventData::Share { .. } => "share",
            EventData::Screenshot { .. } => "screenshot",
            EventData::Expiring { .. } => "expiring",
        }
    }

    pub fn is_input(&self) -> bool {
        matches!(
            self,
            EventData::Motion { .. }
                | EventData::Button { .. }
                | EventData::Key { .. }
                | EventData::Scroll { .. }
                | EventData::Touch { .. }
                | EventData::Gesture { .. }
        )
    }
}

/// Default tiers from the design: wake on human messages, a revoked lock, apps exiting on
/// their own and imminent expiry; the client's own actions are log-only.
pub fn default_tier(d: &EventData) -> Tier {
    match d {
        EventData::Message { .. } | EventData::Revoke | EventData::Expiring { .. } => Tier::Wake,
        EventData::AppExit { killed, .. } => {
            if *killed {
                Tier::Passive
            } else {
                Tier::Wake
            }
        }
        EventData::Command { .. }
        | EventData::AgentMessage { .. }
        | EventData::Work { .. }
        | EventData::Done
        | EventData::Screenshot { .. } => Tier::Log,
        _ => Tier::Passive,
    }
}

/// Expands kind/group names; errors name the valid ones.
pub fn expand_kinds(names: &[String]) -> Result<Vec<&'static str>, String> {
    let mut out = Vec::new();
    for n in names.iter().flat_map(|n| n.split(',')).map(str::trim).filter(|n| !n.is_empty()) {
        if let Some(k) = KINDS.iter().find(|k| **k == n) {
            out.push(*k);
        } else if let Some((_, ks)) = GROUPS.iter().find(|(g, _)| *g == n) {
            out.extend_from_slice(ks);
        } else {
            let groups: Vec<&str> = GROUPS.iter().map(|(g, _)| *g).collect();
            return Err(format!("unknown event kind '{n}'. Kinds: {}. Groups: {}", KINDS.join(" "), groups.join(" ")));
        }
    }
    Ok(out)
}

/// Tier overrides for one poll (`--wake`, `--passive`); names as in [`expand_kinds`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tiers {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wake: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passive: Vec<String>,
}

impl Tiers {
    pub fn check(&self) -> Result<(), String> {
        let w = expand_kinds(&self.wake)?;
        let p = expand_kinds(&self.passive)?;
        if let Some(k) = w.iter().find(|k| p.contains(k)) {
            return Err(format!("'{k}' is in both --wake and --passive"));
        }
        Ok(())
    }

    pub fn tier(&self, d: &EventData) -> Tier {
        let k = d.kind();
        let has = |l: &[String]| expand_kinds(l).is_ok_and(|v| v.contains(&k));
        if has(&self.wake) {
            Tier::Wake
        } else if has(&self.passive) {
            Tier::Passive
        } else {
            default_tier(d)
        }
    }
}

/// What a client hasn't seen yet, attached to replies (`Response::events`) and polls.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Delivery {
    /// Time of this client's previous delivery (unix ms), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    pub events: Vec<Event>,
    /// Human messages no poll has shown yet (command replies only; polls include them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<Unread>,
}

impl Delivery {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.unread.is_none()
    }
}

/// Unread human messages: how many, and when the first arrived (unix ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unread {
    pub count: usize,
    pub first: u64,
}

/// `poll` reply data; the events come in `Response::events`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollInfo {
    /// No wake event before the timeout.
    pub timed_out: bool,
}

/// `log` reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogInfo {
    pub events: Vec<Event>,
    /// Matching events left out by `--last`.
    pub omitted: usize,
    /// Events dropped from the start of the log (memory cap).
    #[serde(default)]
    pub dropped: u64,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers() {
        let msg = EventData::Message { text: "hi".into(), images: vec![], annotations: vec![] };
        let key = EventData::Key { key: "a".into(), pressed: true, text: Some("a".into()) };
        assert_eq!(default_tier(&msg), Tier::Wake);
        assert_eq!(default_tier(&key), Tier::Passive);
        let t = Tiers { wake: vec!["input".into()], passive: vec!["message".into()] };
        assert!(t.check().is_ok());
        assert_eq!(t.tier(&key), Tier::Wake);
        assert_eq!(t.tier(&msg), Tier::Passive);
        assert!(Tiers { wake: vec!["key".into()], passive: vec!["input".into()] }.check().is_err());
        assert!(expand_kinds(&["nope".into()]).unwrap_err().contains("Kinds:"));
        assert_eq!(expand_kinds(&["window,key".into()]).unwrap(), ["window_map", "window_unmap", "title", "key"]);
    }

    #[test]
    fn serde_shape() {
        let e = Event { seq: 3, t: 1000, data: EventData::Unlock, tier: None, by: Some("tok".into()) };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"seq":3,"t":1000,"kind":"unlock"}"#);
        let b: Event = serde_json::from_str(r#"{"seq":1,"t":5,"kind":"motion","path":[[1,2,0]],"tier":"passive"}"#).unwrap();
        assert_eq!(b.data, EventData::Motion { path: vec![[1, 2, 0]] });
    }
}
