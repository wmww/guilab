//! Connecting to the server (autostarting it) and running it.

use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command as Proc, Stdio},
    time::{Duration, Instant},
};

use guilab_proto::{Client, Command, Request, Response, ServerInfo, Target};

/// Autostarted servers exit after this long without sessions.
const AUTOSTART_IDLE_EXIT: u64 = 600;

pub struct Conn {
    client: Client,
}

/// Connects to the server, starting it if it isn't running.
pub fn connect() -> Result<Conn, String> {
    let path = guilab_proto::socket_path().map_err(|e| e.to_string())?;
    if let Ok(client) = Client::connect(&path) {
        let mut c = Conn { client };
        c.warn_if_stale();
        return Ok(c);
    }
    autostart(&path)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(client) = Client::connect(&path) {
            return Ok(Conn { client });
        }
        if Instant::now() > deadline {
            return Err(format!("server did not start; see {}", log_path().display()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn log_path() -> PathBuf {
    guilab_proto::runtime_dir().map(|d| d.join("server.log")).unwrap_or_else(|_| "server.log".into())
}

fn autostart(sock: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = sock.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let log = std::fs::File::create(log_path()).map_err(|e| e.to_string())?;
    let mut cmd = Proc::new(exe);
    cmd.args(["server", "--idle-exit", &AUTOSTART_IDLE_EXIT.to_string()])
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        // Never leak the caller's displays into the server (it has none of its own).
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY");
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()?;
            Ok(())
        });
    }
    // Not waited for: it outlives us (reparented to init/subreaper).
    cmd.spawn().map_err(|e| format!("starting server: {e}"))?;
    Ok(())
}

/// The git worktree root containing the cwd: the nearest ancestor with a `.git` entry.
fn worktree_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

impl Conn {
    /// Sends a request; the reply may be an error (`ok: false`) carrying events.
    pub fn request(&mut self, mut target: Target, cmd: Command) -> Result<Response, String> {
        target.root = worktree_root();
        let req = Request { target, cmd };
        self.client.call(&req).map_err(|e| e.to_string())
    }

    /// The next reply of a streaming request (`poll --follow`).
    pub fn next(&mut self) -> Result<Response, String> {
        self.client.read_reply().map_err(|e| e.to_string())
    }

    /// A server started from an older build keeps running old code; say so.
    fn warn_if_stale(&mut self) {
        let Ok(info) = self.client.call_as::<ServerInfo>(&Request { target: Target::default(), cmd: Command::Ping }) else {
            return;
        };
        let me = std::env::current_exe().ok();
        let mtime = me
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        if me.as_deref() == Some(info.exe.as_path()) && mtime.is_some_and(|m| m > info.exe_mtime) {
            eprintln!("guilab: warning: the server runs an older build; restart it with `guilab server --stop`");
        }
    }
}

pub fn run_server(idle_exit: u64, show: Option<PathBuf>) -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("GUILAB_LOG").unwrap_or_else(|_| "warn,guilab_server=info,guilab_compositor=info,guilab_viewer=info".into()))
        .with_writer(std::io::stderr)
        .init();
    let opts = guilab_server::Options { idle_exit: (idle_exit > 0).then(|| Duration::from_secs(idle_exit)), show };
    guilab_server::run(opts).map_err(|e| format!("{e:#}"))
}

pub fn stop_server() -> Result<(), String> {
    let path = guilab_proto::socket_path().map_err(|e| e.to_string())?;
    let Ok(mut client) = Client::connect(&path) else {
        return Err("no server running".into());
    };
    let info: ServerInfo = client.call_as(&Request { target: Target::default(), cmd: Command::Ping })?;
    client.call_as::<serde_json::Value>(&Request { target: Target::default(), cmd: Command::Shutdown })?;
    // Wait for it to finish cleaning up, so a following command gets a fresh server.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && Path::new(&format!("/proc/{}", info.pid)).exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
