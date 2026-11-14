//! Request handling and session resolution.

use std::{
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

use guilab_compositor::{Chain, ClipRead, InputSource, Session, SessionConfig, Settle, XState};
use guilab_image::{Made, MakeReq, Region, RenderOpts};
use guilab_proto::{
    Command, Delivery, DoneInfo, EventData, HumanOp, ImageInfo, LockInfo, Request, Response, SessionInfo, ShotInfo, Target, events::PollInfo,
    log::ANON,
};

use crate::{ClipWait, DEFAULT_TTL, InputJob, KillWait, NewWait, PollWait, Server, ShotWait, Slot, Working, images::Images, send};

/// How long the app owning the clipboard gets to send it.
const CLIP_TIMEOUT: Duration = Duration::from_secs(3);

/// How long `kill` waits for the apps to exit (they get SIGKILL after 2 s).
const KILL_WAIT: Duration = Duration::from_secs(3);

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
        working: slot.working.as_ref().map(|w| w.status.clone()),
        in_viewer: false,
    }
}

/// Status set when a poll hands the agent a human message.
const READ_STATUS: &str = "read your message";

const NOT_WORKING: &str = "not marked working; run `guilab work \"what you're doing\"` first (the human sees it), \
     and `guilab done \"...\"` when you're finished";

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
        let Request { target, cmd, .. } = req;
        let r = match cmd {
            Command::Ping => {
                let mut info = self.info.clone();
                info.viewer = self.viewer_info();
                Response::ok(info)
            }
            Command::Show { display } => {
                // `-S NAME show` also picks the tab.
                if let Some(n) = &target.name {
                    if !self.sessions.contains_key(n) {
                        return Some(Response::err(format!("no session '{n}' (see guilab ls)")));
                    }
                    self.viewer_tab = Some(n.clone());
                }
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
            Command::Ls => {
                let shown = self.viewer_current();
                let mut list: Vec<SessionInfo> = self.sessions.iter().map(|(n, s)| session_info(n, s)).collect();
                list.iter_mut().for_each(|i| i.in_viewer = shown.as_deref() == Some(i.name.as_str()));
                Response::ok(list)
            }
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
                // One delivery cursor per worktree (one agent each); see guilab_proto::log.
                let key = target.root.as_ref().map_or(ANON.to_string(), |r| r.display().to_string());
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
        if matches!(cmd, Command::Lock { .. } | Command::Input { .. }) && slot.working.is_none() {
            return Some(Response::err(NOT_WORKING));
        }
        if let Err(e) = slot.lock.check(name, cmd.access(), token) {
            return Some(Response::err(e));
        }
        slot.last_active = Instant::now();
        // Commands may move windows, change the output, focus...: render the next tick.
        slot.session.dirty = true;
        if !matches!(cmd, Command::Human { .. }) {
            slot.last_agent = Instant::now();
        }
        if let Some(text) = cmd.log_text() {
            slot.session.log.push(EventData::Command { cmd: text }, Some(key));
        }
        let clone = || stream.try_clone().map_err(|_| Box::new(Response::err("internal: clone stream")));
        let s = &mut slot.session;
        let r = match cmd {
            Command::End => Response::ok(self.end_session(name)),
            Command::Lock { force } => {
                let had = slot.lock.current.as_ref().map(|l| l.token.clone());
                match slot.lock.take(name, token, force) {
                    Ok(token) => {
                        if had.as_deref() != Some(token.as_str()) {
                            s.log.push(EventData::Lock { forced: had.is_some() }, Some(key));
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
                    Err(e) => return Some(*e),
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
                        Err(e) => return Some(*e),
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
            Command::Kill { id } => match s.kill(id) {
                Ok(killed) if killed.is_empty() => Response::ok(killed),
                // Answered once they're gone (see check_kills), so `kill && run` doesn't race
                // a single-instance app's old copy.
                Ok(killed) => {
                    let stream = match clone() {
                        Ok(s) => s,
                        Err(e) => return Some(*e),
                    };
                    self.kills.push(KillWait { session: name.to_string(), key: key.to_string(), stream, killed, deadline: Instant::now() + KILL_WAIT });
                    return None;
                }
                Err(e) => Response::err(format!("{e:#}")),
            },
            Command::Ps => Response::ok(s.ps()),
            Command::Tree => Response::ok(s.tree()),
            Command::Output { size, scale } => {
                if (size.is_some() || scale.is_some())
                    && let Err(e) = set_output(s, size.map(|[w, h]| (w as i32, h as i32)), scale, Some(key))
                {
                    return Some(Response::err(format!("{e:#}")));
                }
                Response::ok(s.output_info())
            }
            Command::Screenshot { path, settle, cursor, crop, max } => {
                let crop = match crop.as_deref().map(guilab_image::format::parse_crop).transpose() {
                    Ok(c) => c.flatten(),
                    Err(e) => return Some(Response::err(format!("--crop: {e}"))),
                };
                let stream = match clone() {
                    Ok(s) => s,
                    Err(e) => return Some(*e),
                };
                // Answered after the next frame (see check_waiters).
                let start = Instant::now();
                self.waiters.push(ShotWait { session: name.to_string(), key: key.to_string(), stream, path, settle, cursor, crop, max, start });
                return None;
            }
            Command::Image { id, path, crop, max, marks, list, points } => {
                let img = match self.images.get(&id) {
                    Ok(i) => i,
                    Err(e) => return Some(Response::err(e)),
                };
                if list {
                    return Some(Response::ok(ImageInfo { id, listing: Some(self.images.listing(&img, points)), render: None }));
                }
                let region = match crop.as_deref().map(guilab_image::format::parse_crop).transpose() {
                    Ok(None) => Region::ImageCrop,
                    Ok(Some(None)) => Region::Canvas,
                    Ok(Some(Some(r))) => Region::Rect(r),
                    Err(e) => return Some(Response::err(format!("--crop: {e}"))),
                };
                match self.images.render_file(&img, &RenderOpts { region, max, marks }, &s.dir, path.as_deref()) {
                    Ok(r) => Response::ok(ImageInfo { id, listing: None, render: Some(r) }),
                    Err(e) => Response::err(e),
                }
            }
            Command::ImageMake { from, canvas, color, crop, drop, elements, cwd } => {
                let req = MakeReq { from, canvas, color, crop, drop, elements, cwd };
                let made = self.images.make(&req, slot.group).and_then(|id| self.images.get(id.as_str()));
                let img = match made {
                    Ok(i) => i,
                    Err(e) => return Some(Response::err(e)),
                };
                s.log.push(EventData::ImageMade { image: img.id.to_string() }, Some(key));
                match self.images.render_file(&img, &RenderOpts::default(), &s.dir, None) {
                    Ok(r) => Response::ok(ImageInfo { id: img.id.to_string(), listing: Some(self.images.listing(&img, false)), render: Some(r) }),
                    Err(e) => Response::err(e),
                }
            }
            Command::Poll { timeout_ms, follow, tiers } => {
                if let Err(e) = tiers.check() {
                    return Some(Response::err(e));
                }
                let pending = s.log.has_wake(key, &tiers);
                // Waiting for the human while marked working would leave them waiting too.
                if !follow && !pending && timeout_ms != Some(0) && slot.working.is_some() {
                    return Some(Response::err(
                        "you're still marked working, so the human isn't expecting to answer. Run `guilab done \"...\"` \
                         first (tell them what you did or need), then poll",
                    ));
                }
                if pending {
                    let d = s.log.deliver(key, &tiers, None, true);
                    let mut r = Response::ok(PollInfo { timed_out: false });
                    read_messages(slot, key, &d);
                    r.events = Some(d);
                    if !follow {
                        return Some(r);
                    }
                    send(stream, &r);
                }
                let log = &mut slot.session.log;
                // Motion going on now must not hide in an older event.
                log.seal();
                let stream = match clone() {
                    Ok(s) => s,
                    Err(e) => return Some(*e),
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
            Command::Work { status, message, images } => {
                let status = status.trim().to_string();
                if status.is_empty() {
                    return Some(Response::err("say what you're doing: guilab work \"running the tests\""));
                }
                if let Err(e) = agent_message(&mut self.images, slot, key, message, &images) {
                    return Some(Response::err(e));
                }
                let s = &mut slot.session;
                s.log.push(EventData::Work { status: status.clone() }, Some(key));
                match &mut slot.working {
                    Some(w) => w.status = status,
                    None => slot.working = Some(Working { status, since: Instant::now() }),
                }
                Response::ok(())
            }
            Command::Done { message, images } => {
                // Answering without reading what the human just said would ignore it.
                if let Some(first) = s.log.unread().next() {
                    let at = guilab_proto::text::Fmt { tz: crate::tz_offset() }.clock(first.t);
                    return Some(Response::err(format!(
                        "a human message arrived at {at} that you haven't read. Read it first: guilab poll (it returns at \
                         once), then finish with guilab done"
                    )));
                }
                // Done means hands off: release the lock, with or without the token (a
                // wrong token is still an error, so a stale holder learns it lost the lock).
                let unlocked = if slot.lock.current.is_some() {
                    match slot.lock.release(name, token, token.is_none()) {
                        Ok(was) => was,
                        Err(e) => return Some(Response::err(e)),
                    }
                } else {
                    false
                };
                if let Err(e) = agent_message(&mut self.images, slot, key, message, &images) {
                    return Some(Response::err(e));
                }
                let s = &mut slot.session;
                if unlocked {
                    s.log.push(EventData::Unlock, Some(key));
                    s.release_all(InputSource::Agent);
                }
                s.log.push(EventData::Done, Some(key));
                slot.working = None;
                if unlocked {
                    self.abort_inputs(name, &Response::err("the lock was released (guilab done); input stopped"));
                }
                Response::ok(DoneInfo { unlocked })
            }
            Command::Human { op } => match op {
                HumanOp::Message { text, images } => {
                    match human_message(&mut self.images, slot, text, images.into_iter().map(MsgImage::Spec).collect()) {
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
                        Err(e) => return Some(*e),
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
                let d = log.deliver(&p.key, &p.tiers, Some(w), true);
                p.watch_from = log.end();
                read_messages(slot, &p.key, &d);
                r.events = Some(d);
                (r, !p.follow)
            } else if p.deadline.is_some_and(|d| now >= d) {
                let mut r = Response::ok(PollInfo { timed_out: true });
                let d = log.deliver(&p.key, &p.tiers, None, true);
                read_messages(slot, &p.key, &d);
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
                Settle::Animating => Some(false),
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
            let shot = self.images.capture(s, slot.group, w.cursor, true, w.crop).and_then(|id| {
                let img = self.images.get(id.as_str())?;
                let r = self.images.render_file(&img, &RenderOpts { max: w.max, ..Default::default() }, &s.dir, w.path.as_deref())?;
                Ok((img, r))
            });
            let resp = match shot {
                Ok((img, r)) => {
                    s.log.push(EventData::Screenshot { image: img.id.to_string(), path: r.path.clone() }, Some(&w.key));
                    let animating = state == Settle::Animating;
                    let [cw, ch] = img.body.canvas;
                    let cropped = cropped_windows(&s.tree(), cw as i32, ch as i32);
                    let header = self.images.header(&img);
                    let (path, width, height, shows, zoom) = (r.path, r.width, r.height, r.shows, r.zoom);
                    Response::ok(ShotInfo { image: img.id.to_string(), header, path, width, height, shows, zoom, settled, animating, cropped })
                }
                Err(e) => Response::err(e),
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
                    let r = Response::ok(slot.session.input_info(&j.chain));
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

    /// Answers `kill`s whose apps are gone (or that waited long enough).
    pub(crate) fn check_kills(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.kills.len() {
            let k = &self.kills[i];
            let running = self.sessions.get(&k.session).is_some_and(|slot| {
                slot.session.ps().iter().any(|p| matches!(p.state, guilab_proto::ProcState::Running) && k.killed.iter().any(|q| q.id == p.id))
            });
            if running && now < k.deadline {
                i += 1;
                continue;
            }
            let k = self.kills.remove(i);
            let r = self.attach(&k.session, Some(&k.key), Response::ok(&k.killed));
            send(&k.stream, &r);
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
    /// An image id, or an absolute path of a file that becomes an image.
    Spec(String),
    /// A drawing from the current viewer: the frozen screenshot's image id and the marks.
    Drawing { base: String, marks: Vec<guilab_image::Element> },
}

/// Logs a human message (wakes polls). Its images are held while the session lives. A
/// message without images gets the current screen, since it refers to what they look at.
pub(crate) fn human_message(images: &mut Images, slot: &mut Slot, text: String, imgs: Vec<MsgImage>) -> Result<(), String> {
    let mut refs = Vec::new();
    for (i, m) in imgs.into_iter().enumerate() {
        let img = match m {
            MsgImage::Spec(spec) => images.attach(&spec, Made::Human, slot.group),
            MsgImage::Drawing { base, marks } => images.drawing(&base, marks, slot.group),
        }
        .map_err(|e| format!("image {}: {e}", i + 1))?;
        images.hold(&img);
        refs.push(images.image_ref(&img));
    }
    let screen = if refs.is_empty() {
        let id = images.capture(&mut slot.session, slot.group, false, false, None)?;
        let img = images.get(id.as_str())?;
        images.hold(&img);
        Some(id.to_string())
    } else {
        None
    };
    slot.session.log.push(EventData::Message { text, images: refs, screen }, None);
    Ok(())
}

/// Toplevels that stick out of the output (screenshots crop them). One pixel of slack: at
/// fractional scales a tiled window can round one pixel larger.
fn cropped_windows(tree: &[guilab_proto::TreeNode], w: i32, h: i32) -> Vec<guilab_proto::WindowSize> {
    tree.iter()
        .filter(|n| matches!(n.kind, guilab_proto::NodeKind::Toplevel | guilab_proto::NodeKind::X11))
        .filter(|n| n.x < -1 || n.y < -1 || n.x + n.width > w + 1 || n.y + n.height > h + 1)
        .map(|n| guilab_proto::WindowSize { id: n.id, width: n.width, height: n.height })
        .collect()
}

/// A poll showed the agent human messages: it's on them now (the human sees it was picked
/// up, and the agent still ends with `done`).
fn read_messages(slot: &mut Slot, key: &str, d: &Delivery) {
    if !d.events.iter().any(|e| matches!(e.data, EventData::Message { .. })) {
        return;
    }
    slot.session.log.push(EventData::Work { status: READ_STATUS.into() }, Some(key));
    match &mut slot.working {
        Some(w) => w.status = READ_STATUS.into(),
        None => slot.working = Some(Working { status: READ_STATUS.into(), since: Instant::now() }),
    }
}

/// Logs an agent message (`work -m`, `done MSG`); images are ids or file paths.
fn agent_message(images: &mut Images, slot: &mut Slot, key: &str, text: Option<String>, specs: &[String]) -> Result<(), String> {
    let text = text.map(|t| t.trim().to_string()).unwrap_or_default();
    if text.is_empty() && specs.is_empty() {
        return Ok(());
    }
    let mut refs = Vec::new();
    for spec in specs {
        let img = images.attach(spec, Made::Agent, slot.group).map_err(|e| format!("--image {spec}: {e}"))?;
        images.hold(&img);
        refs.push(images.image_ref(&img));
    }
    slot.session.log.push(EventData::AgentMessage { text, images: refs }, Some(key));
    Ok(())
}

/// Changes a session's output and logs it. `key`: the agent's delivery key; None = the
/// human (viewer).
pub(crate) fn set_output(s: &mut Session, size: Option<(i32, i32)>, scale: Option<f64>, key: Option<&str>) -> anyhow::Result<()> {
    let before = s.output_info();
    s.set_output(size, scale)?;
    let o = s.output_info();
    if (o.width, o.height, o.scale) == (before.width, before.height, before.scale) {
        return Ok(());
    }
    s.log.push(EventData::Output { width: o.width, height: o.height, scale: o.scale, human: key.is_none() }, key);
    // The overlay notes agent changes; the human sees their own.
    if key.is_some() {
        let note = if o.scale == 1.0 { format!("{}×{}", o.width, o.height) } else { format!("{}×{} @{}x", o.width, o.height, o.scale) };
        s.viz.push(true, guilab_proto::viz::VizKind::Note(note));
    }
    Ok(())
}
