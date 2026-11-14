//! Request handling and session resolution.

use std::{
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

use guilab_compositor::{Chain, ClipRead, InputSource, Session, SessionConfig, Settle};
use guilab_proto::{Command, LockInfo, Request, Response, SessionInfo, ShotInfo, Target};

use crate::{ClipWait, DEFAULT_TTL, InputJob, Server, ShotWait, Slot, send};

/// How long the app owning the clipboard gets to send it.
const CLIP_TIMEOUT: Duration = Duration::from_secs(3);

/// Settle wait caps: once a window is up, and while waiting for the first window.
const SETTLE_CAP: Duration = Duration::from_secs(3);
const MAP_CAP: Duration = Duration::from_secs(10);

const DEFAULT_SIZE: [u32; 2] = [1280, 720];

pub(crate) fn session_info(name: &str, slot: &Slot) -> SessionInfo {
    let s = &slot.session;
    SessionInfo {
        name: name.to_string(),
        wayland_display: s.socket_path(),
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
            Command::Ping => Response::ok(&self.info),
            Command::Shutdown => {
                self.stop = true;
                Response::ok(())
            }
            Command::Ls => Response::ok(self.sessions.iter().map(|(n, s)| session_info(n, s)).collect::<Vec<_>>()),
            Command::New { name, size, scale, ttl } => match self.new_session(&target, name, size, scale, ttl) {
                Ok(info) => Response::ok(info),
                Err(e) => Response::err(e),
            },
            cmd => {
                let name = match self.resolve(&target) {
                    Ok(n) => n,
                    Err(e) => return Some(Response::err(e)),
                };
                let token = target.token.as_deref();
                let slot = self.sessions.get_mut(&name).unwrap();
                if let Err(e) = slot.lock.check(&name, cmd.access(), token) {
                    return Some(Response::err(e));
                }
                slot.last_active = Instant::now();
                let s = &mut slot.session;
                match cmd {
                    Command::End => Response::ok(self.end_session(&name)),
                    Command::Lock { force } => match slot.lock.take(&name, token, force) {
                        Ok(token) => {
                            // Taking over: the old holder's chains stop.
                            if force {
                                self.abort_inputs(&name, &Response::err("your lock was taken over (guilab lock --force)"));
                            }
                            Response::ok(LockInfo { session: name, token })
                        }
                        Err(e) => Response::err(e),
                    },
                    Command::Unlock { force } => match slot.lock.release(&name, token, force) {
                        Ok(_) => {
                            s.release_all(InputSource::Agent);
                            self.abort_inputs(&name, &Response::err("the lock was released; input stopped"));
                            Response::ok(())
                        }
                        Err(e) => Response::err(e),
                    },
                    Command::Input { actions } => {
                        if let Err(e) = s.check_chain(&actions) {
                            return Some(Response::err(e));
                        }
                        let Ok(stream) = stream.try_clone() else { return Some(Response::err("internal: clone stream")) };
                        // Queued behind the session's running chain, if any; else runs now.
                        let now = Instant::now();
                        self.inputs.push(InputJob { session: name, stream, chain: Chain::new(actions), resume_at: now });
                        self.run_inputs(now);
                        return None;
                    }
                    Command::ClipGet => match s.clip_get() {
                        Ok(ClipRead::Text(t)) => Response::ok(t),
                        Ok(ClipRead::Pipe(fd)) => {
                            let pipe = std::fs::File::from(fd);
                            let _ = rustix::fs::fcntl_setfl(&pipe, rustix::fs::OFlags::NONBLOCK);
                            let Ok(stream) = stream.try_clone() else { return Some(Response::err("internal: clone stream")) };
                            let deadline = Instant::now() + CLIP_TIMEOUT;
                            self.clips.push(ClipWait { session: name, stream, pipe, buf: Vec::new(), deadline });
                            return None;
                        }
                        Err(e) => Response::err(e),
                    },
                    Command::ClipSet { text } => {
                        s.clip_set(text);
                        Response::ok(())
                    }
                    Command::Run { argv, cwd, env } => result(s.spawn(&argv, &cwd, &env)),
                    Command::Kill { id } => result(s.kill(id)),
                    Command::Ps => Response::ok(s.ps()),
                    Command::Tree => Response::ok(s.tree()),
                    Command::Output { size, scale } => {
                        if (size.is_some() || scale.is_some())
                            && let Err(e) = s.set_output(size.map(|[w, h]| (w as i32, h as i32)), scale)
                        {
                            return Some(Response::err(format!("{e:#}")));
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
                        let Ok(stream) = stream.try_clone() else { return Some(Response::err("internal: clone stream")) };
                        // Answered after the next frame (see check_waiters).
                        self.waiters.push(ShotWait { session: name, stream, path, settle, start: Instant::now() });
                        return None;
                    }
                    Command::Ping | Command::Shutdown | Command::Ls | Command::New { .. } => unreachable!(),
                }
            }
        };
        Some(r)
    }

    fn new_session(
        &mut self,
        target: &Target,
        name: Option<String>,
        size: Option<[u32; 2]>,
        scale: Option<f64>,
        ttl: Option<u64>,
    ) -> Result<SessionInfo, String> {
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
        let cfg = SessionConfig { name: name.clone(), dir: dir.clone(), size: (w as i32, h as i32), scale };
        let (session, el) = Session::new(cfg, self.gpu.clone()).map_err(|e| format!("{e:#}"))?;
        self.insert_session(session, el, root, ttl.unwrap_or(DEFAULT_TTL)).map_err(|e| format!("{e:#}"))?;
        Ok(session_info(&name, &self.sessions[&name]))
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
                Ok((width, height)) => Response::ok(ShotInfo { path: w.path, width, height, settled }),
                Err(e) => Response::err(format!("screenshot: {e:#}")),
            };
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
                    job.resume_at = now + delay;
                    busy.push(name);
                    i += 1;
                }
                Ok(None) => {
                    let j = self.inputs.remove(i);
                    send(&j.stream, &Response::ok(slot.session.input_info()));
                }
                Err(e) => {
                    let j = self.inputs.remove(i);
                    send(&j.stream, &Response::err(e));
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
                    send(&c.stream, &resp);
                }
            }
        }
    }
}

fn result<T: serde::Serialize>(r: anyhow::Result<T>) -> Response {
    match r {
        Ok(v) => Response::ok(v),
        Err(e) => Response::err(format!("{e:#}")),
    }
}
