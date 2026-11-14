//! Request handling and session resolution.

use std::{
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

use guilab_compositor::{Chain, ClipRead, InputSource, Session, SessionConfig};
use guilab_image::{Made, MakeReq, Region, RenderOpts};
use guilab_proto::{
    Command, Delivery, GlobalCmd, SessionCmd, DoneInfo, EventData, HumanOp, ImageInfo, LockInfo, Request, Response, SessionInfo, Target, events::PollInfo,
    log::ANON,
};

use crate::{
    DEFAULT_TTL, Server,
    images::Images,
    lock::Took,
    pending::{Pending, Reply, Wait},
    slot::{LockEnd, Slot},
};

/// How long the app owning the clipboard gets to send it.
const CLIP_TIMEOUT: Duration = Duration::from_secs(3);

/// How long `kill` waits for the apps to exit (they get SIGKILL after 2 s; a big process
/// on a loaded machine can take a while to go even then).
const KILL_WAIT: Duration = Duration::from_secs(5);

/// How long `new -x` waits for Xwayland.
const XWAYLAND_TIMEOUT: Duration = Duration::from_secs(10);

const DEFAULT_SIZE: [u32; 2] = [1280, 720];

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

    /// Handles a request: replies now, or registers it to be answered later.
    pub(crate) fn handle(&mut self, req: Request, stream: &UnixStream) {
        let Request { target, cmd, .. } = req;
        let cmd = match cmd {
            Command::Global(g) => return self.handle_global(&target, g, stream),
            Command::Session(c) => c,
        };
        let name = match self.resolve(&target) {
            Ok(n) => n,
            Err(e) => return self.out.send(stream, &Response::err(e)),
        };
        // One delivery cursor per agent (see guilab_proto::log): requests from the
        // session's own worktree or from outside git share the default cursor, so an
        // agent that changes directories doesn't see events twice; another worktree
        // (another agent, picking the session by name) gets its own.
        let key = match &target.root {
            Some(r) if self.sessions[&name].root.as_ref() != Some(r) => r.display().to_string(),
            _ => ANON.to_string(),
        };
        let policy = cmd.policy();
        // The human's chains aren't the agent's: no delivery key.
        let wait_key = (!policy.human).then(|| key.clone());
        match self.handle_session(&name, &key, cmd, target.token.as_deref()) {
            Reply::Now(r) => {
                let r = if policy.delivers { self.attach(&name, Some(&key), r) } else { r };
                self.out.send(stream, &r);
            }
            Reply::Later(w) => self.later(name, wait_key, stream, w),
            Reply::Then(r, w) => {
                self.out.send(stream, &r);
                self.later(name, wait_key, stream, w);
            }
        }
    }

    fn handle_global(&mut self, target: &Target, cmd: GlobalCmd, stream: &UnixStream) {
        let r = match cmd {
            GlobalCmd::Ping => {
                let mut info = self.info.clone();
                info.viewer = self.viewer_info();
                info.pending = self.pending_counts();
                Response::ok(info)
            }
            GlobalCmd::Show { display } => 'show: {
                // `-S NAME show` also picks the tab.
                if let Some(n) = &target.name {
                    if !self.sessions.contains_key(n) {
                        break 'show Response::err(format!("no session '{n}' (see guilab ls)"));
                    }
                    self.viewer_tab = Some(n.clone());
                }
                let Ok(clone) = stream.try_clone() else { break 'show Response::err("internal: clone stream") };
                match self.show(display, Some(clone)) {
                    Some(r) => r,
                    None => return,
                }
            }
            GlobalCmd::Hide => {
                self.hide_viewer(None);
                Response::ok(())
            }
            GlobalCmd::Shutdown => {
                self.stop = true;
                Response::ok(())
            }
            GlobalCmd::Ls => {
                let shown = self.viewer_current();
                let mut list: Vec<SessionInfo> = self.sessions.iter().map(|(n, s)| s.info(n)).collect();
                list.iter_mut().for_each(|i| i.in_viewer = shown.as_deref() == Some(i.name.as_str()));
                Response::ok(list)
            }
            GlobalCmd::New { name, size, scale, ttl, xwayland } => match self.new_session(target, name, size, scale, ttl, xwayland) {
                // Answered once Xwayland is up.
                Ok(name) if xwayland => return self.later(name, None, stream, Wait::New { deadline: Instant::now() + XWAYLAND_TIMEOUT }),
                Ok(name) => Response::ok(self.sessions[&name].info(&name)),
                Err(e) => Response::err(e),
            },
        };
        self.out.send(stream, &r);
    }

    /// Registers a request to answer later.
    fn later(&mut self, session: String, key: Option<String>, stream: &UnixStream, wait: Wait) {
        match stream.try_clone() {
            Ok(stream) => self.add_pending(Pending { session, key, stream, wait }),
            Err(e) => self.out.send(stream, &Response::err(format!("internal: clone stream: {e}"))),
        }
    }

    /// A command on session `name`.
    fn handle_session(&mut self, name: &str, key: &str, cmd: SessionCmd, token: Option<&str>) -> Reply {
        let policy = cmd.policy();
        let slot = self.sessions.get_mut(name).unwrap();
        if policy.needs_working && slot.working.is_none() {
            return Reply::Now(Response::err(NOT_WORKING));
        }
        if let Err(e) = slot.lock.check(name, policy.access, token) {
            return Reply::Now(Response::err(e));
        }
        slot.last_active = Instant::now();
        // Commands may move windows, change the output, focus...: render the next tick.
        slot.session.touch();
        if !policy.human {
            slot.last_agent = Instant::now();
        }
        if let Some(text) = cmd.log_text() {
            slot.session.log.push(EventData::Command { cmd: text }, Some(key));
        }
        let s = &mut slot.session;
        let r = match cmd {
            SessionCmd::End => Response::ok(self.end_session(name)),
            SessionCmd::Lock { force } => match slot.lock.take(name, token, force) {
                Ok((token, took)) => {
                    match took {
                        Took::Same => {}
                        Took::New => {
                            s.log.push(EventData::Lock { forced: false }, Some(key));
                        }
                        Took::TakenOver => self.end_lock(name, LockEnd::TakenOver, Some(key)),
                    }
                    Response::ok(LockInfo { session: name.to_string(), token })
                }
                Err(e) => Response::err(e),
            },
            SessionCmd::Unlock { force } => match slot.lock.may_release(name, token, force) {
                Ok(true) => {
                    slot.lock.release();
                    self.end_lock(name, LockEnd::Unlock, Some(key));
                    Response::ok(())
                }
                Ok(false) => Response::ok(()),
                Err(e) => Response::err(e),
            },
            SessionCmd::Input { actions } => {
                if let Err(e) = s.check_chain(&actions) {
                    return Reply::Now(Response::err(e));
                }
                return Reply::Later(Wait::Input { chain: Chain::new(actions, InputSource::Agent), resume_at: Instant::now(), started: false });
            }
            SessionCmd::ClipGet => match s.clip_get() {
                Ok(ClipRead::Text(t)) => Response::ok(t),
                Ok(ClipRead::Pipe(fd)) => {
                    let pipe = std::fs::File::from(fd);
                    let _ = rustix::fs::fcntl_setfl(&pipe, rustix::fs::OFlags::NONBLOCK);
                    return Reply::Later(Wait::Clip { pipe, buf: Vec::new(), deadline: Instant::now() + CLIP_TIMEOUT });
                }
                Err(e) => Response::err(e),
            },
            SessionCmd::ClipSet { text } => {
                s.clip_set(text);
                Response::ok(())
            }
            SessionCmd::Run { argv, cwd, env } => match s.spawn(&argv, &cwd, &env) {
                Ok(p) => {
                    s.log.push(EventData::AppStart { id: p.id, argv: p.argv.clone() }, Some(key));
                    Response::ok(p)
                }
                Err(e) => Response::err(format!("{e:#}")),
            },
            SessionCmd::Kill { id } => match s.kill(id) {
                Ok(killed) if killed.is_empty() => Response::ok(killed),
                // Answered once they're gone (a `Wait::Kill`), so `kill && run` doesn't race
                // a single-instance app's old copy.
                Ok(killed) => return Reply::Later(Wait::Kill { killed, deadline: Instant::now() + KILL_WAIT }),
                Err(e) => Response::err(format!("{e:#}")),
            },
            SessionCmd::Ps => Response::ok(s.ps()),
            SessionCmd::Tree => Response::ok(s.tree()),
            SessionCmd::Output { size, scale } => {
                if (size.is_some() || scale.is_some())
                    && let Err(e) = set_output(s, size.map(|[w, h]| (w as i32, h as i32)), scale, Some(key))
                {
                    return Reply::Now(Response::err(format!("{e:#}")));
                }
                Response::ok(s.output_info())
            }
            SessionCmd::Screenshot { path, settle, cursor, crop, max } => {
                let crop = match crop.as_deref().map(guilab_image::format::parse_crop).transpose() {
                    Ok(c) => c.flatten(),
                    Err(e) => return Reply::Now(Response::err(format!("--crop: {e}"))),
                };
                // Answered after the next frame at the earliest.
                return Reply::Later(Wait::Shot { path, settle, cursor, crop, max, start: Instant::now() });
            }
            SessionCmd::Image { id, path, crop, max, marks, list, points } => {
                let img = match self.images.get(&id) {
                    Ok(i) => i,
                    Err(e) => return Reply::Now(Response::err(e)),
                };
                if list {
                    return Reply::Now(Response::ok(ImageInfo { id, listing: Some(self.images.listing(&img, points)), render: None }));
                }
                let region = match crop.as_deref().map(guilab_image::format::parse_crop).transpose() {
                    Ok(None) => Region::ImageCrop,
                    Ok(Some(None)) => Region::Canvas,
                    Ok(Some(Some(r))) => Region::Rect(r),
                    Err(e) => return Reply::Now(Response::err(format!("--crop: {e}"))),
                };
                match self.images.render_file(&img, &RenderOpts { region, max, marks }, &s.dir, path.as_deref()) {
                    Ok(r) => Response::ok(ImageInfo { id, listing: None, render: Some(r) }),
                    Err(e) => Response::err(e),
                }
            }
            SessionCmd::ImageMake { from, canvas, color, crop, drop, elements, cwd } => {
                let req = MakeReq { from, canvas, color, crop, drop, elements, cwd };
                let made = self.images.make(&req, slot.group).and_then(|id| self.images.get(id.as_str()));
                let img = match made {
                    Ok(i) => i,
                    Err(e) => return Reply::Now(Response::err(e)),
                };
                s.log.push(EventData::ImageMade { image: img.id.to_string() }, Some(key));
                match self.images.render_file(&img, &RenderOpts::default(), &s.dir, None) {
                    Ok(r) => Response::ok(ImageInfo { id: img.id.to_string(), listing: Some(self.images.listing(&img, false)), render: Some(r) }),
                    Err(e) => Response::err(e),
                }
            }
            SessionCmd::Poll { timeout_ms, follow, tiers } => {
                let tiers = match tiers.resolve() {
                    Ok(t) => t,
                    Err(e) => return Reply::Now(Response::err(e)),
                };
                let pending = s.log.has_wake(key, &tiers);
                // Waiting for the human while marked working would leave them waiting too.
                if !follow && !pending && timeout_ms != Some(0) && slot.working.is_some() {
                    return Reply::Now(Response::err(
                        "you're still marked working, so the human isn't expecting to answer. Run `guilab done \"...\"` \
                         first (tell them what you did or need), then poll",
                    ));
                }
                let first = if pending {
                    let d = slot.session.log.deliver(key, &tiers, None, true);
                    let mut r = Response::ok(PollInfo { timed_out: false });
                    read_messages(slot, key, &d);
                    r.events = Some(d);
                    Some(r)
                } else {
                    None
                };
                if let Some(r) = &first
                    && !follow
                {
                    return Reply::Now(r.clone());
                }
                let log = &mut slot.session.log;
                // Motion going on now must not hide in an older event.
                log.seal();
                let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
                let wait = Wait::Poll { deadline, tiers, follow, watch_from: log.end() };
                return match first {
                    Some(r) => Reply::Then(r, wait),
                    None => Reply::Later(wait),
                };
            }
            SessionCmd::Log { after, since, kinds, last } => {
                let kinds = match guilab_proto::events::expand_kinds(&kinds) {
                    Ok(k) => k,
                    Err(e) => return Reply::Now(Response::err(e)),
                };
                return Reply::Now(Response::ok(s.log.query(after, since, &kinds, last)));
            }
            SessionCmd::Work { status, message, images } => {
                let status = status.trim().to_string();
                if status.is_empty() {
                    return Reply::Now(Response::err("say what you're doing: guilab work \"running the tests\""));
                }
                let msg = match agent_message(&mut self.images, slot, message, &images) {
                    Ok(m) => m,
                    Err(e) => return Reply::Now(Response::err(e)),
                };
                if let Some(m) = msg {
                    slot.session.log.push(m, Some(key));
                }
                slot.set_working(status, key);
                Response::ok(())
            }
            SessionCmd::Done { message, images } => {
                // Answering without reading what the human just said would ignore it.
                if let Some(first) = s.log.unread().next() {
                    let at = guilab_proto::text::Fmt::local().clock(first.t);
                    return Reply::Now(Response::err(format!(
                        "a human message arrived at {at} that you haven't read. Read it first: guilab poll (it returns at \
                         once), then finish with guilab done"
                    )));
                }
                // Done means hands off: release the lock, with or without the token (a
                // wrong token is still an error, so a stale holder learns it lost the lock).
                // Everything fallible comes before any change.
                let unlock = match slot.lock.may_release(name, token, token.is_none()) {
                    Ok(u) => u,
                    Err(e) => return Reply::Now(Response::err(e)),
                };
                let msg = match agent_message(&mut self.images, slot, message, &images) {
                    Ok(m) => m,
                    Err(e) => return Reply::Now(Response::err(e)),
                };
                if let Some(m) = msg {
                    slot.session.log.push(m, Some(key));
                }
                if unlock {
                    slot.lock.release();
                    self.end_lock(name, LockEnd::Done, Some(key));
                }
                let slot = self.sessions.get_mut(name).unwrap();
                slot.set_done(key);
                Response::ok(DoneInfo { unlocked: unlock })
            }
            SessionCmd::Human { op } => match op {
                HumanOp::Message { text, images, draw } => {
                    let mut imgs: Vec<MsgImage> = images.into_iter().map(MsgImage::Spec).collect();
                    if !draw.is_empty() {
                        imgs.push(MsgImage::Draw(draw));
                    }
                    match human_message(&mut self.images, slot, text, imgs) {
                        Ok(()) => Response::ok(()),
                        Err(e) => Response::err(e),
                    }
                }
                HumanOp::Input { actions } => {
                    if slot.lock.is_locked() {
                        return Reply::Now(Response::err("the session is locked: human input is blocked"));
                    }
                    if let Err(e) = s.check_chain(&actions) {
                        return Reply::Now(Response::err(e));
                    }
                    return Reply::Later(Wait::Input { chain: Chain::new(actions, InputSource::Human), resume_at: Instant::now(), started: false });
                }
                HumanOp::Revoke => match self.revoke_lock(name) {
                    Ok(()) => Response::ok(()),
                    Err(e) => Response::err(e),
                },
            },
        };
        Reply::Now(r)
    }
}

impl Server {
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
        let scale = scale.unwrap_or(1.0);
        guilab_proto::check_output(Some((w as i64, h as i64)), Some(scale))?;
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

}

/// An image attached to a human message.
pub(crate) enum MsgImage {
    /// An image id, or an absolute path of a file that becomes an image.
    Spec(String),
    /// A drawing from the current viewer: the frozen screenshot's image id and the marks.
    Drawing { base: String, marks: Vec<guilab_image::Element> },
    /// Element lines to draw on a screenshot taken now (`debug message --draw`).
    Draw(Vec<String>),
}

/// Logs a human message (wakes polls). Its images are held while the session lives. A
/// message without images gets the current screen with the pointer drawn in, since it
/// refers to what they look at (or point at).
pub(crate) fn human_message(images: &mut Images, slot: &mut Slot, text: String, imgs: Vec<MsgImage>) -> Result<(), String> {
    let mut refs = Vec::new();
    for (i, m) in imgs.into_iter().enumerate() {
        let img = match m {
            MsgImage::Spec(spec) => images.attach(&spec, Made::Human, slot.group),
            MsgImage::Drawing { base, marks } => images.drawing(&base, marks, slot.group),
            MsgImage::Draw(lines) => images.capture(&mut slot.session, slot.group, false, false, None).and_then(|base| {
                let req = MakeReq { from: Some(base.to_string()), elements: lines, cwd: "/".into(), ..Default::default() };
                let id = guilab_image::make(&mut images.store, &images.fonts, &req, Made::Human, slot.group)?;
                images.get(id.as_str())
            }),
        }
        .map_err(|e| format!("image {}: {e}", i + 1))?;
        images.hold(&img);
        refs.push(images.image_ref(&img));
    }
    let screen = if refs.is_empty() {
        let id = images.capture(&mut slot.session, slot.group, true, false, None)?;
        let img = images.get(id.as_str())?;
        images.hold(&img);
        Some(id.to_string())
    } else {
        None
    };
    slot.session.log.push(EventData::Message { text, images: refs, screen }, None);
    Ok(())
}

/// A poll showed the agent human messages: it's on them now (the human sees it was picked
/// up, and the agent still ends with `done`).
pub(crate) fn read_messages(slot: &mut Slot, key: &str, d: &Delivery) {
    if !d.events.iter().any(|e| matches!(e.data, EventData::Message { .. })) {
        return;
    }
    slot.set_working(READ_STATUS.into(), key);
}

/// The event for an agent message (`work -m`, `done MSG`; images are ids or file paths),
/// or None without text and images. Holds the images; logs nothing, so callers can fail
/// before changing anything.
fn agent_message(images: &mut Images, slot: &Slot, text: Option<String>, specs: &[String]) -> Result<Option<EventData>, String> {
    let text = text.map(|t| t.trim().to_string()).unwrap_or_default();
    if text.is_empty() && specs.is_empty() {
        return Ok(None);
    }
    let imgs = specs
        .iter()
        .map(|spec| images.attach(spec, Made::Agent, slot.group).map_err(|e| format!("--image {spec}: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let refs = imgs
        .iter()
        .map(|img| {
            images.hold(img);
            images.image_ref(img)
        })
        .collect();
    Ok(Some(EventData::AgentMessage { text, images: refs }))
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
        s.note(note);
    }
    Ok(())
}
