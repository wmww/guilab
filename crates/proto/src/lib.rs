//! Control protocol between the `guilab` CLI and the server.
//!
//! Transport: one Unix socket per server (`socket_path()`), line-delimited JSON.
//! Each line from the client is a [`Request`]; the server answers each with one
//! [`Response`] line, in order. Reply payloads are the typed structs below, sent
//! as `Response::data` so the CLI can print them with `--json` as-is.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// `$XDG_RUNTIME_DIR/guilab`.
pub fn runtime_dir() -> io::Result<PathBuf> {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => Ok(Path::new(&d).join("guilab")),
        _ => Err(io::Error::other("XDG_RUNTIME_DIR is not set")),
    }
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
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
    /// Reply: [`SessionInfo`]. Uses `target.root` as the session's root.
    New {
        name: Option<String>,
        size: Option<[u32; 2]>,
        scale: Option<f64>,
        /// Idle seconds before the session ends; 0 = never.
        ttl: Option<u64>,
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
    },
    /// Change and/or show the output. Reply: [`OutputInfo`].
    Output { size: Option<[u32; 2]>, scale: Option<f64> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl Response {
    pub fn ok(data: impl Serialize) -> Self {
        Response { ok: true, error: None, data: serde_json::to_value(data).unwrap_or_default() }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Response { ok: false, error: Some(msg.into()), data: serde_json::Value::Null }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub pid: u32,
    pub exe: PathBuf,
    /// Seconds since the epoch of the server binary's mtime when it started.
    pub exe_mtime: u64,
    pub renderer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    /// Absolute path of the session's Wayland socket (usable as `WAYLAND_DISPLAY`).
    pub wayland_display: PathBuf,
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
    /// Pid of the owning client, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
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
    /// False if the settle wait timed out (animation, spinner, app not drawing yet).
    pub settled: bool,
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
            target: Target { name: Some("a".into()), root: None },
            cmd: Command::Screenshot { path: None, settle: true },
        };
        let s = serde_json::to_string(&req).unwrap();
        assert_eq!(s, r#"{"target":{"name":"a"},"cmd":"screenshot","path":null,"settle":true}"#);
        let back: Request = serde_json::from_str(&s).unwrap();
        assert!(matches!(back.cmd, Command::Screenshot { settle: true, .. }));
        // target may be omitted
        let r: Request = serde_json::from_str(r#"{"cmd":"ls"}"#).unwrap();
        assert!(matches!(r.cmd, Command::Ls));
    }
}
