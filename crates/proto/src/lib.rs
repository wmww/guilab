//! Control protocol between the `guilab` CLI and the server.
//!
//! Transport: one Unix socket per server (`socket_path()`), line-delimited JSON.
//! Each line from the client is a [`Request`]; the server answers each with one
//! [`Response`] line. Send the next request only after the reply: a screenshot reply is
//! deferred, so pipelined replies could arrive out of order. Reply payloads are the typed structs below, sent
//! as `Response::data` so the CLI can print them with `--json` as-is.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub mod events;
pub mod input;
pub mod log;
pub mod text;
pub mod viz;
pub use events::{Annotation, Delivery, Event, EventData, Tier, Tiers};
pub use input::{Action, Button};

/// Protocol version. Bump it whenever a change to [`Request`] or the reply types would make
/// an older server or CLI misread the other; mismatches then fail with "restart the server".
pub const PROTO: u32 = 2;

/// Env var naming the server's directory (control socket, sessions), overriding
/// `$XDG_RUNTIME_DIR/guilab`: to run a second server, e.g. guilab nested in a guilab session.
pub const DIR_VAR: &str = "GUILAB_DIR";

/// The server's directory: `$GUILAB_DIR`, else `$XDG_RUNTIME_DIR/guilab`.
pub fn runtime_dir() -> io::Result<PathBuf> {
    if let Some(d) = std::env::var_os(DIR_VAR).filter(|d| !d.is_empty()) {
        return std::path::absolute(d);
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => Ok(Path::new(&d).join("guilab")),
        _ => Err(io::Error::other("XDG_RUNTIME_DIR is not set")),
    }
}

/// The socket a `WAYLAND_DISPLAY` value names: relative to `XDG_RUNTIME_DIR` unless absolute.
pub fn wayland_socket(display: &str, xdg_runtime_dir: Option<&Path>) -> Option<PathBuf> {
    let p = Path::new(display);
    match display {
        "" => None,
        _ if p.is_absolute() => Some(p.to_path_buf()),
        _ => xdg_runtime_dir.map(|d| d.join(p)),
    }
}

/// The socket of this process's `WAYLAND_DISPLAY`, or an error saying what's missing.
pub fn own_wayland_socket() -> Result<PathBuf, String> {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    wayland_socket(&display, rt.as_deref()).ok_or_else(|| "WAYLAND_DISPLAY is not set; the viewer needs a Wayland desktop to show on".into())
}

pub fn socket_path() -> io::Result<PathBuf> {
    Ok(runtime_dir()?.join("control.sock"))
}

/// Which session a request is about. The server resolves it:
/// `name` > the session created from `root` > the only session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Target {
    /// From `--session` or `$GUILAB_SESSION`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The CLI's git worktree root, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// Lock token (`-t` or `$GUILAB_TOKEN`), for commands that change a locked session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// The client's [`PROTO`]; 0 = an old client that didn't send one.
    #[serde(default)]
    pub proto: u32,
    #[serde(default)]
    pub target: Target,
    #[serde(flatten)]
    pub cmd: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Reply: [`ServerInfo`].
    Ping,
    /// Stop the server (ends all sessions). Reply: null.
    Shutdown,
    /// Open the viewer window on the Wayland display at `display` (a socket path; the CLI
    /// sends its own `WAYLAND_DISPLAY`, since the server's environment may be stale).
    /// Reply: [`ViewerInfo`], once the first frame is up.
    Show { display: PathBuf },
    /// Close the viewer window. Sessions are unaffected. Reply: null.
    Hide,
    /// Reply: [`SessionInfo`]. Uses `target.root` as the session's root.
    New {
        name: Option<String>,
        size: Option<[u32; 2]>,
        scale: Option<f64>,
        /// Idle seconds before the session ends; 0 = never.
        ttl: Option<u64>,
        /// Start Xwayland (the reply comes once it's ready).
        #[serde(default)]
        xwayland: bool,
    },
    /// Reply: [`SessionInfo`] of the ended session.
    End,
    /// Reply: `Vec<SessionInfo>`. Ignores the target.
    Ls,
    /// Reply: [`ProcInfo`].
    Run {
        argv: Vec<String>,
        cwd: PathBuf,
        /// The CLI's environment; the server's may be stale. Session vars are added on top.
        env: Vec<(String, String)>,
    },
    /// Kill one app (by id) or all apps in the session. Reply: `Vec<ProcInfo>` of those signalled.
    Kill { id: Option<u32> },
    /// Reply: `Vec<ProcInfo>`.
    Ps,
    /// Reply: `Vec<TreeNode>` (roots, children nested).
    Tree,
    /// Reply: [`ShotInfo`].
    Screenshot {
        /// Absolute path; None = a new file in the session dir.
        path: Option<PathBuf>,
        settle: bool,
        /// Draw the pointer's cursor in.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cursor: bool,
    },
    /// Change and/or show the output. Reply: [`OutputInfo`].
    Output { size: Option<[u32; 2]>, scale: Option<f64> },
    /// Take the session's lock. Reply: [`LockInfo`]. With the current token: same token.
    /// `force` takes it from another holder.
    Lock { force: bool },
    /// Release the lock (and any held keys, buttons, touches). Reply: null.
    Unlock { force: bool },
    /// Run an input chain; needs the lock. Reply: [`InputInfo`] after the chain finished.
    Input { actions: Vec<Action> },
    /// Reply: the clipboard's text (String).
    ClipGet,
    /// Set the clipboard to text (owned by the server). Reply: null.
    ClipSet { text: String },
    /// Wait for a wake event. Reply: [`events::PollInfo`], events in `Response::events`.
    /// `follow`: keep the connection open and send one reply per wake event.
    Poll {
        /// Give up after this many ms (None = wait forever).
        timeout_ms: Option<u64>,
        #[serde(default)]
        follow: bool,
        #[serde(default)]
        tiers: Tiers,
    },
    /// Read the event log. Reply: [`events::LogInfo`].
    Log {
        /// Only events with a larger seq.
        after: Option<u64>,
        /// Only events at or after this unix time (ms).
        since: Option<u64>,
        /// Kind or group names; empty = all.
        #[serde(default)]
        kinds: Vec<String>,
        /// At most this many (the newest); 0 = all.
        #[serde(default)]
        last: usize,
    },
    /// The agent is working on `status`; `message` (with images, copied into the session
    /// dir) is posted to the viewer's feed. Reply: null.
    Work {
        status: String,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        images: Vec<PathBuf>,
    },
    /// The agent is done and waits for the human: posts `message`, releases the lock.
    /// Reply: [`DoneInfo`].
    Done {
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        images: Vec<PathBuf>,
    },
    /// What the viewer does for the human (until it exists: `guilab debug`, for tests).
    Human { op: HumanOp },
}

/// Human actions (the viewer's, later). Reply: null.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HumanOp {
    /// A message; images are copied into the session dir.
    Message {
        text: String,
        #[serde(default)]
        images: Vec<PathBuf>,
        #[serde(default)]
        annotations: Vec<Annotation>,
    },
    /// Input as the human (blocked while the session is locked and not shared).
    Input { actions: Vec<Action> },
    Revoke,
    Share { on: bool },
}

/// What a command may do to a session, for the lock check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Never needs a token.
    Read,
    /// Needs the token while the session is locked.
    Change,
    /// Always needs the lock (it competes with the human's input).
    Drive,
}

impl Command {
    pub fn access(&self) -> Access {
        match self {
            Command::Input { .. } => Access::Drive,
            Command::Run { .. } | Command::Kill { .. } | Command::End | Command::ClipSet { .. } => Access::Change,
            Command::Output { size, scale } if size.is_some() || scale.is_some() => Access::Change,
            _ => Access::Read,
        }
    }

    /// The command as it goes into the event log (CLI form without `guilab`), or None
    /// for commands that aren't logged (they don't act on a session or have their own events).
    pub fn log_text(&self) -> Option<String> {
        let opt = |name: &str, v: Option<String>| v.map(|v| format!(" {name} {v}")).unwrap_or_default();
        Some(match self {
            Command::Ping | Command::Shutdown | Command::Ls | Command::New { .. } | Command::Show { .. } | Command::Hide => return None,
            Command::Poll { .. } | Command::Log { .. } | Command::Human { .. } => return None,
            Command::Work { .. } | Command::Done { .. } => return None,
            Command::End => "end".into(),
            Command::Run { argv, .. } => format!("run -- {}", argv.join(" ")),
            Command::Kill { id } => format!("kill{}", id.map(|i| format!(" {i}")).unwrap_or_default()),
            Command::Ps => "ps".into(),
            Command::Tree => "tree".into(),
            Command::Screenshot { path, settle, cursor } => format!(
                "screenshot{}{}{}",
                opt("-o", path.as_ref().map(|p| p.display().to_string())),
                if *settle { "" } else { " --no-settle" },
                if *cursor { " --cursor" } else { "" }
            ),
            Command::Output { size, scale } => format!(
                "output{}{}",
                opt("-s", size.map(|[w, h]| format!("{w}x{h}"))),
                opt("--scale", scale.map(|s| s.to_string()))
            ),
            Command::Lock { force } => if *force { "lock --force" } else { "lock" }.into(),
            Command::Unlock { force } => if *force { "unlock --force" } else { "unlock" }.into(),
            Command::Input { actions } => {
                format!("input {}", actions.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(" "))
            }
            Command::ClipGet => "clip get".into(),
            Command::ClipSet { text } => format!("clip set {text:?}"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
    /// Session events this client hasn't seen yet (see `log.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Delivery>,
}

impl Response {
    pub fn ok(data: impl Serialize) -> Self {
        Response { ok: true, error: None, data: serde_json::to_value(data).unwrap_or_default(), events: None }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Response { ok: false, error: Some(msg.into()), data: serde_json::Value::Null, events: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// The server's [`PROTO`] (0 = older than the field).
    #[serde(default)]
    pub proto: u32,
    /// GNU build id of the server binary (hex), if it has one.
    #[serde(default)]
    pub build: String,
    pub pid: u32,
    pub exe: PathBuf,
    /// Seconds since the epoch of the server binary's mtime when it started.
    pub exe_mtime: u64,
    pub renderer: String,
    /// The viewer window, if shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<ViewerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerInfo {
    /// The host display's socket.
    pub display: PathBuf,
    /// How frames reach the host: "dmabuf" or "shm" (CPU copy).
    pub buffers: String,
    /// Frames presented so far (the viewer draws only when something changed).
    #[serde(default)]
    pub frames: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    /// Absolute path of the session's Wayland socket (usable as `WAYLAND_DISPLAY`).
    pub wayland_display: PathBuf,
    /// `DISPLAY` of the session's Xwayland (`:N`), if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_display: Option<String>,
    /// Per-session `XDG_RUNTIME_DIR` given to apps; also holds app logs and screenshots.
    pub dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    pub output: OutputInfo,
    pub apps: usize,
    pub windows: usize,
    pub ttl: u64,
    /// Seconds until the session ends from idleness; None if no TTL.
    pub expires_in: Option<u64>,
    /// The agent's status while it's working (`guilab work`); None while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working: Option<String>,
    /// The session the viewer window shows (its current tab).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_viewer: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct OutputInfo {
    /// Pixel size: screenshots have this size.
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProcState {
    Running,
    Exited { code: i32 },
    Signaled { signal: i32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcInfo {
    /// Per-session app id (used by `kill`).
    pub id: u32,
    pub pid: u32,
    pub argv: Vec<String>,
    #[serde(flatten)]
    pub state: ProcState,
    /// stdout+stderr of the app.
    pub log: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Toplevel,
    Popup,
    Subsurface,
    /// Managed X11 window (via Xwayland).
    X11,
    /// Override-redirect X11 window: menu, tooltip, drag icon.
    X11Popup,
}

/// A surface in the session. Geometry is in output pixels (screenshot coordinates).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Stable per-session surface id.
    pub id: u32,
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Pid of the owning client, if known (X11: `_NET_WM_PID`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// X11 window id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xid: Option<u32>,
    /// X11 `WM_CLASS` class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotInfo {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// False if the settle wait timed out (app not drawing yet) or the app animates.
    pub settled: bool,
    /// Taken while something keeps redrawing (spinner, video), once input had time to land.
    #[serde(default)]
    pub animating: bool,
    /// Windows that don't fit the output, so the image cuts them off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cropped: Vec<WindowSize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSize {
    /// `tree` id.
    pub id: u32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoneInfo {
    /// `done` released the session's lock.
    pub unlocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub session: String,
    pub token: String,
}

/// State after an input chain. Pointer in output pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputInfo {
    pub pointer: [f64; 2],
    /// Surface under the pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<u32>,
    /// Surface with keyboard focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<u32>,
    /// Surface under the pointer at the chain's last press, if it's not `under` any more
    /// (e.g. a menu item whose click closed the menu).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressed_on: Option<u32>,
}

/// A blocking client connection.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let s = UnixStream::connect(path)?;
        Ok(Client { reader: BufReader::new(s.try_clone()?), writer: s })
    }

    pub fn call(&mut self, req: &Request) -> io::Result<Response> {
        let mut line = serde_json::to_vec(req)?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        self.read_reply()
    }

    /// Reads one more reply line (`poll --follow` streams them).
    pub fn read_reply(&mut self) -> io::Result<Response> {
        let mut buf = String::new();
        if self.reader.read_line(&mut buf)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "server closed the connection"));
        }
        Ok(serde_json::from_str(&buf)?)
    }

    /// `call`, then decode `data` as `T`, turning `ok: false` into an error.
    pub fn call_as<T: DeserializeOwned>(&mut self, req: &Request) -> Result<T, String> {
        let r = self.call(req).map_err(|e| e.to_string())?;
        if !r.ok {
            return Err(r.error.unwrap_or_else(|| "unknown error".into()));
        }
        serde_json::from_value(r.data).map_err(|e| format!("bad reply: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let req = Request {
            proto: PROTO,
            target: Target { name: Some("a".into()), ..Default::default() },
            cmd: Command::Screenshot { path: None, settle: true, cursor: false },
        };
        let s = serde_json::to_string(&req).unwrap();
        assert_eq!(s, format!(r#"{{"proto":{PROTO},"target":{{"name":"a"}},"cmd":"screenshot","path":null,"settle":true}}"#));
        let back: Request = serde_json::from_str(&s).unwrap();
        assert!(matches!(back.cmd, Command::Screenshot { settle: true, .. }));
        // target may be omitted
        let r: Request = serde_json::from_str(r#"{"cmd":"ls"}"#).unwrap();
        assert!(matches!(r.cmd, Command::Ls));
    }
}
