//! Apps spawned in a session: `run`, `kill`, `ps`, and cleanup when the session ends.
//!
//! Each app gets its own process group (setsid) and the env var `GUILAB_TAG=<session tag>`,
//! inherited by everything it spawns. Ending a session signals the groups, then every
//! process carrying the tag (catches double-forked daemons), like guibox did.

use std::{
    fs::File,
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use guilab_proto::{EventData, ProcInfo, ProcState};
use rustix::process::{Pid, PidfdFlags, Signal, kill_process_group, pidfd_open};
use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};

use crate::session::{SOCKET_NAME, Session};

/// After SIGTERM, wait this long before SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(2);

pub const TAG_VAR: &str = "GUILAB_TAG";

struct Proc {
    info: ProcInfo,
    child: Option<Child>,
    kill_at: Option<Instant>,
    /// Signalled by `kill` (its exit is expected).
    killed: bool,
}

#[derive(Default)]
pub struct Procs {
    list: Vec<Proc>,
    next_id: u32,
}

/// Env vars never passed from the CLI to apps: they would point at the wrong display or bus.
const DROP_ENV: &[&str] = &[
    "WAYLAND_DISPLAY",
    "WAYLAND_SOCKET",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "SWAYSOCK",
    "I3SOCK",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "XDG_SESSION_TYPE",
    "GUILAB_SESSION",
    TAG_VAR,
];

impl Session {
    /// The tag every app process of this session carries in its environment.
    pub fn tag(&self) -> String {
        self.tag.clone()
    }

    pub fn spawn(&mut self, argv: &[String], cwd: &Path, env: &[(String, String)]) -> anyhow::Result<ProcInfo> {
        let Some(prog) = argv.first() else { bail!("nothing to run") };
        self.procs.next_id += 1;
        let id = self.procs.next_id;
        let log = self.dir.join(format!("app-{id}.log"));
        let out = File::create(&log).with_context(|| format!("creating {}", log.display()))?;
        let mut cmd = Command::new(prog);
        cmd.args(&argv[1..])
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter().filter(|(k, _)| !DROP_ENV.contains(&k.as_str())).map(|(k, v)| (k, v)))
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("WAYLAND_DISPLAY", SOCKET_NAME)
            .env("XDG_SESSION_TYPE", "wayland")
            .env("GUILAB_SESSION", &self.name)
            .env(TAG_VAR, self.tag())
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out);
        if let Some(d) = self.x_display() {
            cmd.env("DISPLAY", d);
        }
        // Qt and SDL default to X11 without these; don't override an explicit choice.
        if !env.iter().any(|(k, _)| k == "QT_QPA_PLATFORM") {
            cmd.env("QT_QPA_PLATFORM", "wayland");
        }
        if !env.iter().any(|(k, _)| k == "SDL_VIDEODRIVER") {
            cmd.env("SDL_VIDEODRIVER", "wayland");
        }
        unsafe {
            cmd.pre_exec(|| {
                // The server blocks SIGTERM/SIGINT/SIGHUP (signalfd) and ignores SIGPIPE;
                // children inherit both, so undo them.
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                rustix::process::setsid()?;
                // Die with the server, even if it crashes.
                rustix::process::set_parent_process_death_signal(Some(Signal::TERM))?;
                Ok(())
            });
        }
        let child = cmd.spawn().with_context(|| format!("running {prog}"))?;
        let pid = child.id();
        let info = ProcInfo { id, pid, argv: argv.to_vec(), state: ProcState::Running, log };

        // Exit notification via pidfd (fd-based, so it works in the nested session loop).
        let fd = pidfd_open(Pid::from_child(&child), PidfdFlags::empty())?;
        self.handle
            .insert_source(Generic::new(fd, Interest::READ, Mode::Level), move |_, _, s: &mut Session| {
                s.reap(id);
                Ok(PostAction::Remove)
            })
            .map_err(|e| e.error)?;
        self.procs.list.push(Proc { info: info.clone(), child: Some(child), kill_at: None, killed: false });
        Ok(info)
    }

    fn reap(&mut self, id: u32) {
        let Some(p) = self.procs.list.iter_mut().find(|p| p.info.id == id) else { return };
        let Some(child) = p.child.as_mut() else { return };
        if let Ok(Some(status)) = child.try_wait() {
            use std::os::unix::process::ExitStatusExt;
            p.info.state = match (status.code(), status.signal()) {
                (Some(code), _) => ProcState::Exited { code },
                (None, Some(signal)) => ProcState::Signaled { signal },
                _ => ProcState::Exited { code: -1 },
            };
            p.child = None;
            p.kill_at = None;
            tracing::info!("{}: app {id} {:?}", self.name, p.info.state);
            let (code, signal) = match p.info.state {
                ProcState::Exited { code } => (Some(code), None),
                ProcState::Signaled { signal } => (None, Some(signal)),
                ProcState::Running => (None, None),
            };
            let ev = EventData::AppExit { id, argv: p.info.argv.clone(), code, signal, killed: p.killed, log: p.info.log.clone() };
            self.log.push(ev, None);
        }
    }

    pub fn ps(&self) -> Vec<ProcInfo> {
        self.procs.list.iter().map(|p| p.info.clone()).collect()
    }

    pub fn running_apps(&self) -> usize {
        self.procs.list.iter().filter(|p| p.child.is_some()).count()
    }

    /// SIGTERM one app (or all), escalating to SIGKILL after `KILL_GRACE` (see `reap_tick`).
    pub fn kill(&mut self, id: Option<u32>) -> anyhow::Result<Vec<ProcInfo>> {
        if let Some(id) = id
            && !self.procs.list.iter().any(|p| p.info.id == id)
        {
            bail!("no app {id} (guilab ps)");
        }
        let now = Instant::now();
        let mut out = Vec::new();
        for p in self.procs.list.iter_mut().filter(|p| id.is_none_or(|i| i == p.info.id)) {
            if p.child.is_some() {
                signal_group(p.info.pid, Signal::TERM);
                p.kill_at.get_or_insert(now + KILL_GRACE);
                p.killed = true;
                out.push(p.info.clone());
            }
        }
        Ok(out)
    }

    /// Escalates overdue kills. Called from the server's timer.
    pub fn reap_tick(&mut self, now: Instant) {
        for p in &mut self.procs.list {
            if p.child.is_some() && p.kill_at.is_some_and(|t| now >= t) {
                signal_group(p.info.pid, Signal::KILL);
                p.kill_at = None;
            }
        }
    }

    /// SIGTERMs everything in the session. Returns the children (still to be reaped)
    /// and the tag; the caller SIGKILLs leftovers after `KILL_GRACE`.
    pub fn shutdown(&mut self) -> (Vec<Child>, String) {
        let tag = self.tag();
        let mut children = Vec::new();
        for p in &mut self.procs.list {
            if let Some(c) = p.child.take() {
                signal_group(p.info.pid, Signal::TERM);
                children.push(c);
            }
        }
        signal_tagged(&tag, Signal::TERM);
        (children, tag)
    }
}

fn signal_group(pid: u32, sig: Signal) {
    if let Some(pid) = Pid::from_raw(pid as i32) {
        let _ = kill_process_group(pid, sig);
    }
}

/// Signals every process whose environment has `GUILAB_TAG=tag`. Returns how many.
pub fn signal_tagged(tag: &str, sig: Signal) -> usize {
    let needle = format!("{TAG_VAR}={tag}");
    let mut n = 0;
    let Ok(dir) = std::fs::read_dir("/proc") else { return 0 };
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        let Ok(env) = std::fs::read(e.path().join("environ")) else { continue };
        if env.split(|b| *b == 0).any(|v| v == needle.as_bytes())
            && let Some(pid) = Pid::from_raw(pid)
            && rustix::process::kill_process(pid, sig).is_ok()
        {
            n += 1;
        }
    }
    n
}
