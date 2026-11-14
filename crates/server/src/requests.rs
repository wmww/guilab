//! Request handling and session resolution.

use std::{
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

use guilab_compositor::{Chain, ClipRead, InputSource, Session, SessionConfig, Settle, XState};
use guilab_proto::{Command, EventData, HumanOp, LockInfo, Request, Response, SessionInfo, ShotInfo, Target, events::PollInfo};

use crate::{ClipWait, DEFAULT_TTL, InputJob, NewWait, PollWait, Server, ShotWait, Slot, send};

/// How long the app owning the clipboard gets to send it.
const CLIP_TIMEOUT: Duration = Duration::from_secs(3);

/// How long `new -x` waits for Xwayland.
const XWAYLAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Settle wait caps: once a window is up, and while waiting for the first window.
const SETTLE_CAP: Duration = Duration::from_secs(3);
const MAP_CAP: Duration = Duration::from_secs(10);

const DEFAULT_SIZE: [u32; 2] = [1280, 720];

pub(crate) fn session_info(name: &str, slot: &Slot) -> SessionInfo {
    let s = &slot.session;
    SessionInfo {
        name: name.to_string(),
        wayland_display: s.socket_path(),
        x_display: s.x_display(),
        dir: s.dir.clone(),
        root: slot.root.clone(),
        output: s.output_info(),
        apps: s.running_apps(),
        windows: s.windows(),
        ttl: slot.ttl,
        expires_in: (slot.ttl > 0).then(|| slot.ttl.saturating_sub(slot.last_active.elapsed().as_secs())),
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 32
        && !n.starts_with('.')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn sanitize(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '-' }).collect();
    s.trim_matches('-').chars().take(32).collect()
}

impl Server {
    /// `--session` > session created from the worktree root > the only session.
    fn resolve(&self, t: &Target) -> Result<String, String> {
        if let Some(n) = &t.name {
            return if self.sessions.contains_key(n) {
                Ok(n.clone())
            } else {
                Err(format!("no session '{n}' (see guilab ls)"))
            };
        }
        if let Some(root) = &t.root {
            let mine: Vec<&String> = self.sessions.iter().filter(|(_, s)| s.root.as_ref() == Some(root)).map(|(n, _)| n).collect();
            match mine.as_slice() {
                [one] => return Ok((*one).clone()),
                [] => {}
                many => {
                    let names: Vec<&str> = many.iter().map(|s| s.as_str()).collect();
                    return Err(format!("several sessions for this worktree ({}); pick one with --session", names.join(", ")));
                }
            }
        }
        match self.sessions.len() {
            0 => Err("no session; start one with `guilab new`".into()),
            1 => Ok(self.sessions.keys().next().unwrap().clone()),
            _ => Err("several sessions; pick one with --session NAME or $GUILAB_SESSION".into()),
        }
    }

    /// Handles a request. `None` means the reply is sent later (screenshot settle wait).
    pub(crate) fn handle(&mut self, req: Request, stream: &UnixStream) -> Option<Response> {
        let Request { target, cmd } = req;
        let r = match cmd {
            Command::Ping => {
                let mut info = self.info.clone();
                info.viewer = self.viewer_info();
                Response::ok(info)
            }
            Command::Show { display } => {
                let Ok(stream) = stream.try_clone() else { return Some(Response::err("internal: clone stream")) };
                return self.show(display, Some(stream));
            }
            Command::Hide => {
                self.hide_viewer(None);
                Response::ok(())
            }
            Command::Shutdown => {
                self.stop = true;
                Response::ok(())
            }
            Command::Ls => Response::ok(self.sessions.iter().map(|(n, s)| session_info(n, s)).collect::<Vec<_>>()),
            Command::New { name, size, scale, ttl, xwayland } => match self.new_session(&target, name, size, scale, ttl, xwayland) {
                // Answered once Xwayland is up (see check_news).
                Ok(name) if xwayland => {
                    let Ok(stream) = stream.try_clone() else { return Some(Response::err("internal: clone stream")) };
                    self.news.push(NewWait { session: name, stream, deadline: Instant::now() + XWAYLAND_TIMEOUT });
                    return None;
                }
                Ok(name) => Response::ok(session_info(&name, &self.sessions[&name])),
                Err(e) => Response::err(e),
            },
            cmd => {
                let name = match self.resolve(&target) {
                    Ok(n) => n,
                    Err(e) => return Some(Response::err(e)),
                };
                let key = target.token.clone().unwrap_or_default();
                // Not deliveries: the human's actions (the viewer's), reading the log, and
                // polls (they deliver themselves).
                let attach = !matches!(cmd, Command::Human { .. } | Command::Log { .. } | Command::Poll { .. });
                let r = self.handle_session(&name, &key, cmd, target.token.as_deref(), stream);
                return if attach { r.map(|r| self.attach(&name, Some(&key), r)) } else { r };
            }
        };
        Some(r)
    }

    /// A command on session `name`. `None` = answered later.
    fn handle_session(&mut self, name: &str, key: &str, cmd: Command, token: Option<&str>, stream: &UnixStream) -> Option<Response> {
        let slot = self.sessions.get_mut(name).unwrap();
        if let Err(e) = slot.lock.check(name, cmd.access(), token) {
            return Some(Response::err(e));
        }
        slot.last_active = Instant::now();
        if let Some(text) = cmd.log_text() {
            slot.session.log.push(EventData::Command { cmd: text }, Some(key));
        }
        let clone = || stream.try_clone().map_err(|_| Response::err("internal: clone stream"));
        let s = &mut slot.session;
        let r = match cmd {
            Command::End => Response::ok(self.end_session(name)),
            Command::Lock { force } => {
                let had = slot.lock.current.as_ref().map(|l| l.token.clone());
                match slot.lock.take(name, token, force) {
                    Ok(token) => {
                        if had.as_deref() != Some(token.as_str()) {
                            s.log.push(EventData::Lock { forced: had.is_some() }, Some(key));
                            // The new token has seen everything so far.
                            s.log.start_key(&token);
                        }
                        // Taking over: the old holder's chains stop.
                        if force {
                            self.abort_inputs(name, &Response::err("your lock was taken over (guilab lock --force)"));
                        }
                        Response::ok(LockInfo { session: name.to_string(), token })
                    }
                    Err(e) => Response::err(e),
                }
            }
            Command::Unlock { force } => match slot.lock.release(name, token, force) {
                Ok(was) => {
                    if was {
                        s.log.push(EventData::Unlock, Some(key));
                    }
                    s.release_all(InputSource::Agent);
                    self.abort_inputs(name, &Response::err("the lock was released; input stopped"));
                    Response::ok(())
                }
                Err(e) => Response::err(e),
            },
            Command::Input { actions } => {
                if let Err(e) = s.check_chain(&actions) {
                    return Some(Response::err(e));
                }
                let stream = match clone() {
                    Ok(s) => s,
                    Err(e) => return Some(e),
                };
                self.queue_input(name, Some(key), stream, Chain::new(actions, InputSource::Agent));
                return None;
            }
            Command::ClipGet => match s.clip_get() {
                Ok(ClipRead::Text(t)) => Response::ok(t),
                Ok(ClipRead::Pipe(fd)) => {
                    let pipe = std::fs::File::from(fd);
                    let _ = rustix::fs::fcntl_setfl(&pipe, rustix::fs::OFlags::NONBLOCK);
                    let stream = match clone() {
                        Ok(s) => s,
                        Err(e) => return Some(e),
                    };
                    let deadline = Instant::now() + CLIP_TIMEOUT;
                    self.clips.push(ClipWait { session: name.to_string(), key: key.to_string(), stream, pipe, buf: Vec::new(), deadline });
                    return None;
                }
                Err(e) => Response::err(e),
            },
            Command::ClipSet { text } => {
                s.clip_set(text);
                Response::ok(())
            }
            Command::Run { argv, cwd, env } => match s.spawn(&argv, &cwd, &env) {
                Ok(p) => {
                    s.log.push(EventData::AppStart { id: p.id, argv: p.argv.clone() }, Some(key));
                    Response::ok(p)
                }
                Err(e) => Response::err(format!("{e:#}")),
            },
            Command::Kill { id } => result(s.kill(id)),
            Command::Ps => Response::ok(s.ps()),
            Command::Tree => Response::ok(s.tree()),
            Command::Output { size, scale } => {
                if size.is_some() || scale.is_some() {
                    if let Err(e) = s.set_output(size.map(|[w, h]| (w as i32, h as i32)), scale) {
                        return Some(Response::err(format!("{e:#}")));
                    }
                    let o = s.output_info();
                    s.log.push(EventData::Output { width: o.width, height: o.height, scale: o.scale }, Some(key));
                }
                Response::ok(s.output_info())
            }
            Command::Screenshot { path, settle } => {
                let path = match path {
                    Some(p) => p,
                    None => {
                        let n = (1..).find(|i| !s.dir.join(format!("shot-{i}.png")).exists()).unwrap();
                        s.dir.join(format!("shot-{n}.png"))
                    }
                };
                let stream = match clone() {
                    Ok(s) => s,
                    Err(e) => return Some(e),
                };
                // Answered after the next frame (see check_waiters).
                self.waiters.push(ShotWait { session: name.to_string(), key: key.to_string(), stream, path, settle, start: Instant::now() });
                return None;
            }
            Command::Poll { timeout_ms, follow, tiers } => {
                if let Err(e) = tiers.check() {
                    return Some(Response::err(e));
                }
                let log = &mut s.log;
                if log.has_wake(key, &tiers) {
                    let d = log.deliver(key, &tiers, None);
                    let mut r = Response::ok(PollInfo { timed_out: false });
                    r.events = Some(d);
                    if !follow {
                        return Some(r);
                    }
                    send(stream, &r);
                }
                // Motion going on now must not hide in an older event.
                log.seal();
                let stream = match clone() {
                    Ok(s) => s,
                    Err(e) => return Some(e),
                };
                let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
                let watch_from = log.end();
                self.polls.push(PollWait { session: name.to_string(), key: key.to_string(), stream, deadline, tiers, follow, watch_from });
                return None;
            }
            Command::Log { after, since, kinds, last } => {
                let kinds = match guilab_proto::events::expand_kinds(&kinds) {
                    Ok(k) => k,
                    Err(e) => return Some(Response::err(e)),
                };
                return Some(Response::ok(s.log.query(after, since, &kinds, last)));
            }
            Command::Say { text, images } => match copy_images(&s.dir, "say", s.log.end(), &images) {
                Ok(images) => {
                    s.log.push(EventData::Say { text, images }, Some(key));
                    Response::ok(())
                }
                Err(e) => Response::err(e),
            },
            Command::Human { op } => match op {
                HumanOp::Message { text, images, annotations } => {
                    match human_message(s, text, images.into_iter().map(MsgImage::File).collect(), annotations) {
                        Ok(()) => Response::ok(()),
                        Err(e) => Response::err(e),
                    }
                }
                HumanOp::Input { actions } => {
                    if !slot.lock.human_input_allowed() {
                        return Some(Response::err("the session is locked: human input is blocked (unless shared)"));
                    }
                    if let Err(e) = s.check_chain(&actions) {
                        return Some(Response::err(e));
                    }
                    let stream = match clone() {
                        Ok(s) => s,
                        Err(e) => return Some(e),
                    };
                    self.queue_input(name, None, stream, Chain::new(actions, InputSource::Human));
                    return None;
                }
                HumanOp::Revoke => {
                    if slot.lock.current.is_none() {
                        return Some(Response::err("not locked"));
                    }
                    self.revoke_lock(name);
                    Response::ok(())
                }
                HumanOp::Share { on } => {
                    if slot.lock.current.is_none() {
                        return Some(Response::err("not locked"));
                    }
                    self.share(name, on);
                    Response::ok(())
                }
            },
            Command::Ping | Command::Shutdown | Command::Ls | Command::New { .. } | Command::Show { .. } | Command::Hide => unreachable!(),
        };
        Some(r)
    }

    /// Queues an input chain behind the session's running one, if any; else runs it now.
    fn queue_input(&mut self, name: &str, key: Option<&str>, stream: UnixStream, chain: Chain) {
        let now = Instant::now();
        self.inputs.push(InputJob { session: name.to_string(), key: key.map(str::to_string), stream, chain, resume_at: now });
        self.run_inputs(now);
    }

    /// Answers polls that got a wake event or timed out; drops those whose client left.
    pub(crate) fn check_polls(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.polls.len() {
            let p = &mut self.polls[i];
            if peer_gone(&mut p.stream) {
                self.polls.remove(i);
                continue;
            }
            let Some(slot) = self.sessions.get_mut(&p.session) else {
                i += 1;
                continue;
            };
            let log = &mut slot.session.log;
            let (resp, done) = if let Some(w) = log.first_wake(&p.key, &p.tiers, p.watch_from) {
                let mut r = Response::ok(PollInfo { timed_out: false });
                r.events = Some(log.deliver(&p.key, &p.tiers, Some(w)));
                p.watch_from = log.end();
                (r, !p.follow)
            } else if p.deadline.is_some_and(|d| now >= d) {
                let mut r = Response::ok(PollInfo { timed_out: true });
                let d = log.deliver(&p.key, &p.tiers, None);
                r.events = (!d.events.is_empty()).then_some(d);
                (r, true)
            } else {
                i += 1;
                continue;
            };
            send(&p.stream, &resp);
            if done {
                self.polls.remove(i);
            } else {
                i += 1;
            }
        }
    }

    fn new_session(
        &mut self,
        target: &Target,
        name: Option<String>,
        size: Option<[u32; 2]>,
        scale: Option<f64>,
        ttl: Option<u64>,
        xwayland: bool,
    ) -> Result<String, String> {
        let root = target.root.clone();
        let name = match name {
            Some(n) if !valid_name(&n) => return Err(format!("bad session name '{n}': use letters, digits, - _ .")),
            Some(n) => n,
            None => {
                if let Some(root) = &root
                    && let Some((n, _)) = self.sessions.iter().find(|(_, s)| s.root.as_ref() == Some(root))
                {
                    return Err(format!("session '{n}' already exists for this worktree"));
                }
                let base = root
                    .as_deref()
                    .and_then(Path::file_name)
                    .map(|n| sanitize(&n.to_string_lossy()))
                    .filter(|n| valid_name(n));
                match base {
                    Some(b) if !self.sessions.contains_key(&b) => b,
                    Some(b) => (2..).map(|i| format!("{b}-{i}")).find(|n| !self.sessions.contains_key(n)).unwrap(),
                    None => (1..).map(|i| format!("s{i}")).find(|n| !self.sessions.contains_key(n)).unwrap(),
                }
            }
        };
        if self.sessions.contains_key(&name) {
            return Err(format!("session '{name}' already exists"));
        }
        let [w, h] = size.unwrap_or(DEFAULT_SIZE);
        if !(1..=16384).contains(&w) || !(1..=16384).contains(&h) {
            return Err("size must be 1x1 to 16384x16384".into());
        }
        let scale = scale.unwrap_or(1.0);
        if !(0.25..=8.0).contains(&scale) {
            return Err("scale must be 0.25 to 8".into());
        }
        let dir = self.sessions_dir.join(&name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let cfg = SessionConfig { name: name.clone(), dir: dir.clone(), size: (w as i32, h as i32), scale, xwayland };
        let (session, el) = match Session::new(cfg, self.gpu.clone()) {
            Ok(s) => s,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("{e:#}"));
            }
        };
        self.insert_session(session, el, root, ttl.unwrap_or(DEFAULT_TTL)).map_err(|e| format!("{e:#}"))?;
        Ok(name)
    }

    /// Answers `new -x` once the session's Xwayland is ready; ends the session if it failed.
    pub(crate) fn check_news(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.news.len() {
            let n = &self.news[i];
            let state = self.sessions.get(&n.session).map(|s| s.session.x_state().clone());
            let failed = match state {
                None => Some(format!("session {} ended", n.session)),
                Some(XState::Ready(_)) => None,
                Some(XState::Failed(e)) => Some(e),
                Some(_) if now >= n.deadline => Some("Xwayland didn't start in time".into()),
                Some(_) => {
                    i += 1;
                    continue;
                }
            };
            let n = self.news.remove(i);
            let resp = match failed {
                None => Response::ok(session_info(&n.session, &self.sessions[&n.session])),
                Some(e) => {
                    self.end_session(&n.session);
                    Response::err(e)
                }
            };
            send(&n.stream, &resp);
        }
    }

    /// Answers screenshot requests whose session settled or whose wait ran out.
    pub(crate) fn check_waiters(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.waiters.len() {
            let w = &self.waiters[i];
            let Some(slot) = self.sessions.get_mut(&w.session) else {
                i += 1;
                continue;
            };
            let s = &mut slot.session;
            let waited = now.duration_since(w.start);
            let state = s.settle_state(now);
            let settled = match state {
                Settle::Settled => Some(true),
                // Nothing that could ever draw: don't wait.
                Settle::NoWindow if s.running_apps() == 0 => Some(true),
                Settle::NoWindow => (waited >= MAP_CAP).then_some(false),
                Settle::Busy => (waited >= SETTLE_CAP).then_some(false),
            };
            let settled = if w.settle { settled } else { Some(state == Settle::Settled) };
            let Some(settled) = settled else {
                i += 1;
                continue;
            };
            let w = self.waiters.remove(i);
            let resp = match s.write_png(&w.path) {
                Ok((width, height)) => {
                    s.log.push(EventData::Screenshot { path: w.path.clone() }, Some(&w.key));
                    Response::ok(ShotInfo { path: w.path, width, height, settled })
                }
                Err(e) => Response::err(format!("screenshot: {e:#}")),
            };
            let resp = self.attach(&w.session, Some(&w.key), resp);
            send(&w.stream, &resp);
        }
    }
}

impl Server {
    /// Runs due input chains; one chain per session at a time, in request order.
    pub(crate) fn run_inputs(&mut self, now: Instant) {
        let mut busy: Vec<String> = Vec::new();
        let mut i = 0;
        while i < self.inputs.len() {
            let name = self.inputs[i].session.clone();
            if busy.contains(&name) || self.inputs[i].resume_at > now {
                busy.push(name);
                i += 1;
                continue;
            }
            let Some(slot) = self.sessions.get_mut(&name) else {
                let j = self.inputs.remove(i);
                send(&j.stream, &Response::err(format!("session {name} ended")));
                continue;
            };
            slot.last_active = now;
            let job = &mut self.inputs[i];
            match slot.session.run_chain(&mut job.chain) {
                Ok(Some(delay)) => {
                    // From the scheduled time, so 16.7 ms glide steps don't stretch to two
                    // 16 ms ticks each.
                    job.resume_at = (job.resume_at + delay).max(now);
                    busy.push(name);
                    i += 1;
                }
                Ok(None) => {
                    let j = self.inputs.remove(i);
                    let r = Response::ok(slot.session.input_info());
                    let r = self.attach(&name, j.key.as_deref(), r);
                    send(&j.stream, &r);
                }
                Err(e) => {
                    let j = self.inputs.remove(i);
                    let r = self.attach(&name, j.key.as_deref(), Response::err(e));
                    send(&j.stream, &r);
                }
            }
        }
    }

    /// Reads clipboard pipes; answers at EOF or timeout.
    pub(crate) fn check_clips(&mut self, now: Instant) {
        use std::io::Read;
        let mut i = 0;
        while i < self.clips.len() {
            let c = &mut self.clips[i];
            let mut tmp = [0u8; 65536];
            let done = loop {
                match c.pipe.read(&mut tmp) {
                    Ok(0) => break Some(Ok(())),
                    Ok(n) => c.buf.extend_from_slice(&tmp[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break None,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => break Some(Err(e.to_string())),
                }
            };
            let done = done.or_else(|| (now >= c.deadline).then(|| Err("the app owning the clipboard didn't send it".to_string())));
            match done {
                None => i += 1,
                Some(r) => {
                    let c = self.clips.remove(i);
                    let resp = match r {
                        Ok(()) => Response::ok(String::from_utf8_lossy(&c.buf)),
                        Err(e) => Response::err(e),
                    };
                    let resp = self.attach(&c.session, Some(&c.key), resp);
                    send(&c.stream, &resp);
                }
            }
        }
    }
}

/// Whether the client of a waiting request hung up (it sends nothing after its request).
fn peer_gone(stream: &mut UnixStream) -> bool {
    use std::io::Read;
    let mut b = [0u8; 64];
    match stream.read(&mut b) {
        Ok(n) => n == 0,
        Err(e) => !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted),
    }
}

/// An image attached to a human message.
pub(crate) enum MsgImage {
    File(std::path::PathBuf),
    /// Opaque RGBA8 pixels (from the viewer).
    Rgba(u32, u32, Vec<u8>),
}

/// Logs a human message (wakes polls). Images are stored in the session dir as
/// `msg-SEQ-N.EXT` (files copied, pixels written as PNG).
pub(crate) fn human_message(s: &mut Session, text: String, images: Vec<MsgImage>, annotations: Vec<guilab_proto::Annotation>) -> Result<(), String> {
    if let Some(a) = annotations.iter().find(|a| a.image.is_some_and(|i| i >= images.len())) {
        return Err(format!("annotation {:?} is on a missing image", a.text));
    }
    let seq = s.log.end();
    let mut paths = Vec::new();
    for (i, img) in images.into_iter().enumerate() {
        match img {
            MsgImage::File(src) => {
                let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png");
                let dst = s.dir.join(format!("msg-{seq}-{}.{ext}", i + 1));
                std::fs::copy(&src, &dst).map_err(|e| format!("image {}: {e}", src.display()))?;
                paths.push(dst);
            }
            MsgImage::Rgba(w, h, px) => {
                let dst = s.dir.join(format!("msg-{seq}-{}.png", i + 1));
                guilab_compositor::write_rgba_png(&dst, w, h, &px).map_err(|e| format!("writing {}: {e:#}", dst.display()))?;
                paths.push(dst);
            }
        }
    }
    s.log.push(EventData::Message { text, images: paths, annotations }, None);
    Ok(())
}

/// Copies message attachments into the session dir as `PREFIX-SEQ-N.EXT`.
fn copy_images(dir: &Path, prefix: &str, seq: u64, images: &[std::path::PathBuf]) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = Vec::new();
    for (i, src) in images.iter().enumerate() {
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let dst = dir.join(format!("{prefix}-{seq}-{}.{ext}", i + 1));
        std::fs::copy(src, &dst).map_err(|e| format!("image {}: {e}", src.display()))?;
        out.push(dst);
    }
    Ok(out)
}

fn result<T: serde::Serialize>(r: anyhow::Result<T>) -> Response {
    match r {
        Ok(v) => Response::ok(v),
        Err(e) => Response::err(format!("{e:#}")),
    }
}
