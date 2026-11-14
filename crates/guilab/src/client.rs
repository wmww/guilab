//! Connecting to the server (autostarting it) and running it.

use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command as Proc, Stdio},
    time::{Duration, Instant},
};

use guilab_proto::{Client, Command, GlobalCmd, Request, Response, ServerInfo, Target};

/// Autostarted servers exit after this long without sessions.
const AUTOSTART_IDLE_EXIT: u64 = 600;

pub struct Conn {
    client: Client,
}

/// Set to 1 to never autostart a server (e.g. driving a nested guilab that's restarting).
const NO_AUTOSTART_VAR: &str = "GUILAB_NO_AUTOSTART";

/// Connects to the server, starting it if it isn't running.
pub fn connect() -> Result<Conn, String> {
    let path = guilab_proto::socket_path().map_err(|e| e.to_string())?;
    if let Ok(client) = Client::connect(&path) {
        let mut c = Conn { client };
        c.check_server()?;
        return Ok(c);
    }
    if std::env::var(NO_AUTOSTART_VAR).is_ok_and(|v| !v.is_empty() && v != "0") {
        return Err(format!(
            "no guilab server at {} ({NO_AUTOSTART_VAR} is set, so none was started). Start one with `guilab server`, \
             or wait for one with `guilab server --wait`",
            path.display()
        ));
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
        let req = Request { proto: guilab_proto::PROTO, target, cmd };
        self.client.call(&req).map_err(|e| e.to_string())
    }

    /// The next reply of a streaming request (`poll --follow`).
    pub fn next(&mut self) -> Result<Response, String> {
        self.client.read_reply().map_err(|e| e.to_string())
    }

    /// A server from another build: an error if it speaks another protocol, else a warning
    /// (it keeps running its old code until restarted).
    fn check_server(&mut self) -> Result<(), String> {
        let ping = Request { proto: guilab_proto::PROTO, target: Target::default(), cmd: GlobalCmd::Ping.into() };
        let Ok(info) = self.client.call_as::<ServerInfo>(&ping) else { return Ok(()) };
        if info.proto != guilab_proto::PROTO {
            return Err(format!(
                "the running guilab server (pid {}, {}) is an incompatible build (protocol {}, this CLI {}). Restart it \
                 with `guilab server --stop` (ends its sessions), or use the guilab binary it runs",
                info.pid,
                info.exe.display(),
                info.proto,
                guilab_proto::PROTO
            ));
        }
        let mine = crate::build_id::build_id();
        let differs = if !mine.is_empty() && !info.build.is_empty() {
            mine != info.build
        } else {
            // No build ids: an older binary at the same path means a rebuild.
            let me = std::env::current_exe().ok();
            let mtime = me
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            me.as_deref() == Some(info.exe.as_path()) && mtime.is_some_and(|m| m > info.exe_mtime)
        };
        if differs {
            eprintln!(
                "guilab: note: the server (pid {}) runs another build of guilab than this CLI; restart it to use this one: \
                 guilab server --stop (ends its sessions)",
                info.pid
            );
        }
        Ok(())
    }
}

pub fn run_server(idle_exit: u64, show: Option<PathBuf>, build: String) -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("GUILAB_LOG").unwrap_or_else(|_| "warn,guilab_server=info,guilab_compositor=info,guilab_viewer=info".into()))
        .with_writer(std::io::stderr)
        .init();
    let opts = guilab_server::Options { idle_exit: (idle_exit > 0).then(|| Duration::from_secs(idle_exit)), show, build };
    guilab_server::run(opts).map_err(|e| format!("{e:#}"))
}

/// `server --wait`: returns once a server answers (up to 30 s).
pub fn wait_server() -> Result<(), String> {
    let path = guilab_proto::socket_path().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let ping = Request { proto: guilab_proto::PROTO, target: Target::default(), cmd: GlobalCmd::Ping.into() };
    loop {
        if let Ok(mut c) = Client::connect(&path)
            && c.call_as::<ServerInfo>(&ping).is_ok()
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("no guilab server answered at {} within 30 s", path.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn stop_server() -> Result<(), String> {
    let path = guilab_proto::socket_path().map_err(|e| e.to_string())?;
    let Ok(mut client) = Client::connect(&path) else {
        return Err("no server running".into());
    };
    let req = |cmd| Request { proto: guilab_proto::PROTO, target: Target::default(), cmd };
    let info: ServerInfo = client.call_as(&req(GlobalCmd::Ping.into()))?;
    client.call_as::<serde_json::Value>(&req(GlobalCmd::Shutdown.into()))?;
    // Wait for it to finish cleaning up, so a following command gets a fresh server.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && Path::new(&format!("/proc/{}", info.pid)).exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
