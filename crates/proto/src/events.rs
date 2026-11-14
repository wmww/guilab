//! Session event log types: what happened, and how it's delivered to agents.
//!
//! Every event has a delivery tier (see notes/design.md "Event log"):
//! - `wake`: ends a blocking `poll` / emits a `poll --follow` line;
//! - `passive`: rides along with the next delivery (poll result or any command's reply);
//! - `log`: only in `guilab log` (the agent's own commands, messages, status, screenshots).
//!
//! Human messages are special: they stay unread until a poll shows them (see `log.rs`).
//!
//! Defaults come from [`KINDS`] (via [`default_tier`]); `poll --wake/--passive` override
//! them by kind or group name ([`GROUPS`]), resolved once per request ([`ResolvedTiers`]).

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
    /// echoed back to it. Server-internal, never serialized.
    #[serde(skip)]
    pub by: Option<String>,
}

/// An image in a message or event: its id plus what the agent reads without fetching it.
/// The listing is a snapshot (images never change, but a parent may expire later).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub id: String,
    /// Canvas size.
    pub size: [u32; 2],
    /// The element listing (`guilab image ID --list`).
    pub listing: String,
    /// Canvas rect `[x, y, w, h]` around every element but the base screenshot, plus a
    /// margin, clamped to the canvas: where to look. None if there are no such elements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<[i32; 4]>,
}

impl ImageRef {
    /// The command that shows the image, cropped to its marks if it has any.
    pub fn fetch(&self) -> String {
        match self.marks {
            Some([x, y, w, h]) => format!("guilab image {} --crop '{x},{y} {w}x{h}'", self.id),
            None => format!("guilab image {}", self.id),
        }
    }

    /// Number of elements in the listing.
    pub fn elements(&self) -> usize {
        self.listing.lines().count().saturating_sub(1)
    }
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
        images: Vec<ImageRef>,
    },
    /// The agent is working (`guilab work`, or a poll handed it a human message).
    Work { status: String },
    /// The agent finished and waits for the human (`guilab done`).
    Done,
    /// Human -> agent message. `screen`: what the session showed when it was sent, for
    /// messages without images (they refer to what the human looks at).
    Message {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen: Option<String>,
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
    /// An agent screenshot: the image and the PNG written for it.
    Screenshot { image: String, path: PathBuf },
    /// `guilab image make` made an image.
    ImageMade { image: String },
    /// The session ends from inactivity in `secs` unless a command arrives.
    Expiring { secs: u64 },
}

/// Every kind name (as used by `--wake`, `--passive` and `log --kind`) with its default
/// tier. The one table: `default_tier`, `expand_kinds` and the CLI's `poll --help` read it.
pub const KINDS: &[(&str, Tier)] = &[
    ("message", Tier::Wake),
    ("revoke", Tier::Wake),
    // Passive after `guilab kill` (see `default_tier`).
    ("app_exit", Tier::Wake),
    ("expiring", Tier::Wake),
    ("motion", Tier::Passive),
    ("button", Tier::Passive),
    ("key", Tier::Passive),
    ("scroll", Tier::Passive),
    ("touch", Tier::Passive),
    ("gesture", Tier::Passive),
    ("window_map", Tier::Passive),
    ("window_unmap", Tier::Passive),
    ("title", Tier::Passive),
    ("output", Tier::Passive),
    ("app_start", Tier::Passive),
    ("lock", Tier::Passive),
    ("unlock", Tier::Passive),
    // The agent's own actions.
    ("command", Tier::Log),
    ("agent_message", Tier::Log),
    ("work", Tier::Log),
    ("done", Tier::Log),
    ("screenshot", Tier::Log),
    ("image_made", Tier::Log),
];

/// Group names, each standing for several kinds (plus `all`).
pub const GROUPS: &[(&str, &[&str])] = &[
    ("input", &["motion", "button", "key", "scroll", "touch", "gesture"]),
    ("window", &["window_map", "window_unmap", "title"]),
    ("app", &["app_start", "app_exit"]),
    ("agent", &["command", "agent_message", "work", "done", "screenshot", "image_made"]),
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
            EventData::Screenshot { .. } => "screenshot",
            EventData::ImageMade { .. } => "image_made",
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

/// Default tiers from the design ([`KINDS`]): wake on human messages, a revoked lock, apps
/// exiting on their own and imminent expiry; the client's own actions are log-only.
pub fn default_tier(d: &EventData) -> Tier {
    if let EventData::AppExit { killed: true, .. } = d {
        return Tier::Passive;
    }
    let k = d.kind();
    KINDS.iter().find(|(n, _)| *n == k).map(|(_, t)| *t).expect("every kind is in KINDS (tested)")
}

/// Expands kind/group names; errors name the valid ones.
pub fn expand_kinds(names: &[String]) -> Result<Vec<&'static str>, String> {
    let mut out = Vec::new();
    for n in names.iter().flat_map(|n| n.split(',')).map(str::trim).filter(|n| !n.is_empty()) {
        if let Some((k, _)) = KINDS.iter().find(|(k, _)| *k == n) {
            out.push(*k);
        } else if n == "all" {
            out.extend(KINDS.iter().map(|(k, _)| *k));
        } else if let Some((_, ks)) = GROUPS.iter().find(|(g, _)| *g == n) {
            out.extend_from_slice(ks);
        } else {
            let kinds: Vec<&str> = KINDS.iter().map(|(k, _)| *k).collect();
            let groups: Vec<&str> = GROUPS.iter().map(|(g, _)| *g).collect();
            return Err(format!("unknown event kind '{n}'. Kinds: {}. Groups: {} all", kinds.join(" "), groups.join(" ")));
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
    /// Checks the names once, for the log to use per event.
    pub fn resolve(&self) -> Result<ResolvedTiers, String> {
        let wake = expand_kinds(&self.wake)?;
        let passive = expand_kinds(&self.passive)?;
        if let Some(k) = wake.iter().find(|k| passive.contains(k)) {
            return Err(format!("'{k}' is in both --wake and --passive"));
        }
        Ok(ResolvedTiers { wake, passive })
    }
}

/// [`Tiers`] as kind names; the default is no overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedTiers {
    wake: Vec<&'static str>,
    passive: Vec<&'static str>,
}

impl ResolvedTiers {
    pub fn tier(&self, d: &EventData) -> Tier {
        let k = d.kind();
        if self.wake.contains(&k) {
            Tier::Wake
        } else if self.passive.contains(&k) {
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
        let msg = EventData::Message { text: "hi".into(), images: vec![], screen: None };
        let key = EventData::Key { key: "a".into(), pressed: true, text: Some("a".into()) };
        assert_eq!(default_tier(&msg), Tier::Wake);
        assert_eq!(default_tier(&key), Tier::Passive);
        let t = Tiers { wake: vec!["input".into()], passive: vec!["message".into()] }.resolve().unwrap();
        assert_eq!(t.tier(&key), Tier::Wake);
        assert_eq!(t.tier(&msg), Tier::Passive);
        assert!(Tiers { wake: vec!["key".into()], passive: vec!["input".into()] }.resolve().is_err());
        assert!(expand_kinds(&["nope".into()]).unwrap_err().contains("Kinds:"));
        assert_eq!(expand_kinds(&["window,key".into()]).unwrap(), ["window_map", "window_unmap", "title", "key"]);
    }

    /// One event of every kind. The match makes a new variant fail to compile here, so it
    /// gets a sample, and then the test below checks it's in `KINDS`.
    fn samples() -> Vec<EventData> {
        use EventData as E;
        let all = vec![
            E::Command { cmd: String::new() },
            E::AgentMessage { text: String::new(), images: vec![] },
            E::Work { status: String::new() },
            E::Done,
            E::Message { text: String::new(), images: vec![], screen: None },
            E::Motion { path: vec![] },
            E::Button { button: String::new(), pressed: true, x: 0, y: 0, window: None },
            E::Key { key: String::new(), pressed: true, text: None },
            E::Scroll { dx: 0.0, dy: 0.0, x: 0, y: 0, smooth: false },
            E::Touch { finger: 0, down: true, x: 0, y: 0 },
            E::Gesture { gesture: String::new(), fingers: 2, dx: 0.0, dy: 0.0, scale: None, cancelled: false },
            E::AppStart { id: 1, argv: vec![] },
            E::AppExit { id: 1, argv: vec![], code: None, signal: None, killed: false, log: PathBuf::new() },
            E::WindowMap { id: 1, window: String::new(), app_id: None, title: None },
            E::WindowUnmap { id: 1, title: None, by: None },
            E::Title { id: 1, title: None },
            E::Output { width: 1, height: 1, scale: 1.0, human: false },
            E::Lock { forced: false },
            E::Unlock,
            E::Revoke,
            E::Screenshot { image: String::new(), path: PathBuf::new() },
            E::ImageMade { image: String::new() },
            E::Expiring { secs: 1 },
        ];
        for d in &all {
            match d {
                E::Command { .. } | E::AgentMessage { .. } | E::Work { .. } | E::Done | E::Message { .. } | E::Motion { .. } => {}
                E::Button { .. } | E::Key { .. } | E::Scroll { .. } | E::Touch { .. } | E::Gesture { .. } | E::AppStart { .. } => {}
                E::AppExit { .. } | E::WindowMap { .. } | E::WindowUnmap { .. } | E::Title { .. } | E::Output { .. } => {}
                E::Lock { .. } | E::Unlock | E::Revoke | E::Screenshot { .. } | E::ImageMade { .. } | E::Expiring { .. } => {}
            }
        }
        all
    }

    #[test]
    fn kinds_table() {
        let all = samples();
        let kinds: Vec<&str> = all.iter().map(|d| d.kind()).collect();
        let table: Vec<&str> = KINDS.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds.len(), table.len(), "KINDS has a stale or duplicate entry");
        for d in &all {
            assert!(table.contains(&d.kind()), "{} missing from KINDS", d.kind());
            let group = |g: &str| GROUPS.iter().find(|(n, _)| *n == g).unwrap().1.contains(&d.kind());
            assert_eq!(d.is_input(), group("input"), "{}", d.kind());
            // The agent group is exactly the agent's own (log-only) actions.
            assert_eq!(default_tier(d) == Tier::Log, group("agent"), "{}", d.kind());
        }
        for (_, ks) in GROUPS {
            assert!(ks.iter().all(|k| table.contains(k)), "{ks:?}");
        }
        let killed = EventData::AppExit { id: 1, argv: vec![], code: None, signal: Some(15), killed: true, log: PathBuf::new() };
        assert_eq!(default_tier(&killed), Tier::Passive);
    }

    #[test]
    fn serde_shape() {
        let e = Event { seq: 3, t: 1000, data: EventData::Unlock, tier: None, by: Some("tok".into()) };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"seq":3,"t":1000,"kind":"unlock"}"#);
        let b: Event = serde_json::from_str(r#"{"seq":1,"t":5,"kind":"motion","path":[[1,2,0]],"tier":"passive"}"#).unwrap();
        assert_eq!(b.data, EventData::Motion { path: vec![[1, 2, 0]] });
    }
}
