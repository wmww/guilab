//! The guilab daemon: one calloop loop holding the control socket, the frame timer and
//! every session's nested loop. Single-threaded.

mod requests;

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{ErrorKind, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    process::Child,
    rc::Rc,
    cell::RefCell,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
    generic::Generic,
    signals::{Signal, Signals},
    timer::{TimeoutAction, Timer},
};
use guilab_compositor::{Gpu, Session, SharedGpu, procs};
use guilab_proto::{Response, ServerInfo};

pub struct Options {
    /// Exit after this long without sessions (autostarted servers).
    pub idle_exit: Option<Duration>,
}

/// Default idle TTL of a session, in seconds.
pub const DEFAULT_TTL: u64 = 1800;
const TICK: Duration = Duration::from_millis(16);

pub(crate) struct Slot {
    pub session: Session,
    pub root: Option<PathBuf>,
    pub ttl: u64,
    pub last_active: Instant,
    token: RegistrationToken,
}

/// A screenshot request waiting for its session to settle.
pub(crate) struct ShotWait {
    pub session: String,
    pub stream: UnixStream,
    pub path: PathBuf,
    pub settle: bool,
    pub start: Instant,
}

/// Processes of an ended session, to SIGKILL after the grace period and reap.
struct Grave {
    children: Vec<Child>,
    tag: String,
    kill_at: Instant,
    killed: bool,
}

pub struct Server {
    pub(crate) handle: LoopHandle<'static, Server>,
    pub(crate) gpu: SharedGpu,
    pub(crate) sessions: BTreeMap<String, Slot>,
    pub(crate) waiters: Vec<ShotWait>,
    graveyard: Vec<Grave>,
    pub(crate) sessions_dir: PathBuf,
    pub(crate) info: ServerInfo,
    pub(crate) stop: bool,
    idle_exit: Option<Duration>,
    idle_since: Instant,
    last_slow_tick: Instant,
}

pub fn run(opts: Options) -> anyhow::Result<()> {
    let rt = guilab_proto::runtime_dir()?;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&rt)?;
    // One server per runtime dir.
    let lock = File::create(rt.join("server.lock"))?;
    if rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_err() {
        bail!("a guilab server is already running");
    }
    let sock_path = guilab_proto::socket_path()?;
    let _ = fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).with_context(|| format!("binding {}", sock_path.display()))?;
    listener.set_nonblocking(true)?;

    // Sessions of a previous (crashed) server are gone; their apps died with it (PDEATHSIG).
    let sessions_dir = rt.join("s");
    let _ = fs::remove_dir_all(&sessions_dir);
    fs::DirBuilder::new().mode(0o700).create(&sessions_dir)?;

    let gpu = Gpu::new()?;
    tracing::info!("renderer: {}", gpu.name);
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_mtime = fs::metadata(&exe)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let info = ServerInfo { pid: std::process::id(), exe, exe_mtime, renderer: gpu.name.clone() };

    let mut event_loop: EventLoop<'static, Server> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    handle
        .insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, l, srv: &mut Server| {
            loop {
                match l.accept() {
                    Ok((s, _)) => srv.add_conn(s),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!("accept: {e}");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| e.error)?;
    handle
        .insert_source(Signals::new(&[Signal::SIGTERM, Signal::SIGINT, Signal::SIGHUP])?, |ev, _, srv: &mut Server| {
            tracing::info!("got {:?}, stopping", ev.signal());
            srv.stop = true;
        })
        .map_err(|e| e.error)?;
    handle
        .insert_source(Timer::from_duration(TICK), |_, _, srv: &mut Server| {
            srv.tick();
            TimeoutAction::ToDuration(TICK)
        })
        .map_err(|e| e.error)?;

    let now = Instant::now();
    let mut srv = Server {
        handle,
        gpu: Rc::new(RefCell::new(gpu)),
        sessions: BTreeMap::new(),
        waiters: Vec::new(),
        graveyard: Vec::new(),
        sessions_dir,
        info,
        stop: false,
        idle_exit: opts.idle_exit,
        idle_since: now,
        last_slow_tick: now,
    };
    eprintln!("guilab server {} listening on {}", std::process::id(), sock_path.display());
    while !srv.stop {
        event_loop.dispatch(None, &mut srv)?;
    }

    let names: Vec<String> = srv.sessions.keys().cloned().collect();
    for n in names {
        srv.end_session(&n);
    }
    // Give apps the grace period, then kill and reap what's left.
    let deadline = Instant::now() + procs::KILL_GRACE;
    while !srv.graveyard.is_empty() && Instant::now() < deadline {
        srv.reap_graves(Instant::now());
        std::thread::sleep(Duration::from_millis(50));
    }
    srv.reap_graves(deadline + Duration::from_secs(1));
    let _ = fs::remove_file(&sock_path);
    drop(lock);
    Ok(())
}

impl Server {
    fn add_conn(&mut self, stream: UnixStream) {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let mut buf = Vec::new();
        let res = self.handle.insert_source(Generic::new(stream, Interest::READ, Mode::Level), move |_, s, srv: &mut Server| {
            // Safety: we never drop or replace the stream.
            let stream = unsafe { s.get_mut() };
            let mut tmp = [0u8; 4096];
            let mut eof = false;
            loop {
                match stream.read(&mut tmp) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return Ok(PostAction::Remove),
                }
            }
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                if line.iter().all(|b| b.is_ascii_whitespace()) {
                    continue;
                }
                if let Some(resp) = srv.handle_line(&line, stream) {
                    send(stream, &resp);
                }
            }
            Ok(if eof { PostAction::Remove } else { PostAction::Continue })
        });
        if let Err(e) = res {
            tracing::warn!("adding connection: {}", e.error);
        }
    }

    fn handle_line(&mut self, line: &[u8], stream: &UnixStream) -> Option<Response> {
        match serde_json::from_slice::<guilab_proto::Request>(line) {
            Ok(req) => self.handle(req, stream),
            Err(e) => Some(Response::err(format!("bad request: {e}"))),
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        for slot in self.sessions.values_mut() {
            slot.session.tick();
            slot.session.reap_tick(now);
        }
        self.check_waiters(now);
        self.reap_graves(now);
        if now.duration_since(self.last_slow_tick) >= Duration::from_secs(1) {
            self.last_slow_tick = now;
            let expired: Vec<String> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.ttl > 0 && now.duration_since(s.last_active).as_secs() >= s.ttl)
                .map(|(n, _)| n.clone())
                .collect();
            for n in expired {
                tracing::info!("session {n} expired");
                self.end_session(&n);
            }
            if !self.sessions.is_empty() {
                self.idle_since = now;
            } else if self.idle_exit.is_some_and(|d| now.duration_since(self.idle_since) >= d) {
                tracing::info!("idle, exiting");
                self.stop = true;
            }
        }
    }

    fn reap_graves(&mut self, now: Instant) {
        self.graveyard.retain_mut(|g| {
            if !g.killed && now >= g.kill_at {
                procs::signal_tagged(&g.tag, rustix::process::Signal::KILL);
                for c in &mut g.children {
                    let _ = c.kill();
                }
                g.killed = true;
            }
            g.children.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
            !(g.killed && g.children.is_empty())
        });
    }

    /// Ends a session: drops its display (disconnecting clients) and kills its apps.
    pub(crate) fn end_session(&mut self, name: &str) -> Option<guilab_proto::SessionInfo> {
        let mut slot = self.sessions.remove(name)?;
        let info = requests::session_info(name, &slot);
        self.handle.remove(slot.token);
        let (children, tag) = slot.session.shutdown();
        let dir = slot.session.dir.clone();
        drop(slot);
        self.graveyard.push(Grave { children, tag, kill_at: Instant::now() + procs::KILL_GRACE, killed: false });
        let _ = fs::remove_dir_all(&dir);
        let mut i = 0;
        while i < self.waiters.len() {
            if self.waiters[i].session == name {
                let w = self.waiters.remove(i);
                send(&w.stream, &Response::err(format!("session {name} ended")));
            } else {
                i += 1;
            }
        }
        Some(info)
    }

    /// Registers a session's nested loop in the server loop.
    pub(crate) fn insert_session(
        &mut self,
        session: Session,
        event_loop: calloop::EventLoop<'static, Session>,
        root: Option<PathBuf>,
        ttl: u64,
    ) -> anyhow::Result<()> {
        let name = session.name.clone();
        let key = name.clone();
        let token = self
            .handle
            .insert_source(Generic::new(event_loop, Interest::READ, Mode::Level), move |_, el, srv: &mut Server| {
                if let Some(slot) = srv.sessions.get_mut(&key) {
                    // Safety: we never drop or replace the loop from here.
                    let el = unsafe { el.get_mut() };
                    if let Err(e) = el.dispatch(Some(Duration::ZERO), &mut slot.session) {
                        tracing::warn!("session {key}: {e}");
                    }
                    slot.session.flush();
                }
                Ok(PostAction::Continue)
            })
            .map_err(|e| e.error)?;
        let _ = fs::set_permissions(&session.dir, fs::Permissions::from_mode(0o700));
        self.sessions.insert(name, Slot { session, root, ttl, last_active: Instant::now(), token });
        Ok(())
    }
}

/// Writes one response line, blocking briefly if the socket is full.
pub(crate) fn send(stream: &UnixStream, resp: &Response) {
    let mut line = serde_json::to_vec(resp).unwrap_or_default();
    line.push(b'\n');
    let mut s = stream;
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    if let Err(e) = s.write_all(&line) {
        tracing::debug!("reply: {e}");
    }
    let _ = stream.set_nonblocking(true);
}
