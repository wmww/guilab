//! Requests answered later: screenshots waiting to settle, input chains, clipboard pipes,
//! `kill`, `new -x`, polls. One list, stepped from the tick, so the rules that apply to all
//! of them live here once: a client that hung up drops its request, replies to agent
//! commands carry their events, and an ending session answers everything still waiting.

use std::{
    fs::File,
    io::{ErrorKind, Read},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Instant,
};

use guilab_compositor::{Chain, XState};
use guilab_image::RenderOpts;
use guilab_proto::{ProcInfo, Response, ShotInfo, Tiers, events::PollInfo};

use crate::{Server, requests::read_messages};

pub(crate) struct Pending {
    pub session: String,
    /// The requester's delivery key (see `guilab_proto::log`); None for the human and `new`.
    pub key: Option<String>,
    pub stream: UnixStream,
    pub wait: Wait,
}

pub(crate) enum Wait {
    /// A screenshot waiting for the session to settle.
    Shot { path: Option<PathBuf>, settle: bool, cursor: bool, crop: Option<guilab_image::IRect>, max: Option<[u32; 2]>, start: Instant },
    /// An input chain paused at a sleep, or queued behind its session's running chain
    /// (one chain per session at a time, in request order).
    Input { chain: Chain, resume_at: Instant, started: bool },
    /// `clip get` reading the owning client's pipe.
    Clip { pipe: File, buf: Vec<u8>, deadline: Instant },
    /// `kill` waiting for its apps to exit.
    Kill { killed: Vec<ProcInfo>, deadline: Instant },
    /// `new -x` waiting for Xwayland.
    New { deadline: Instant },
    /// `poll` waiting for a wake event, or streaming them (`follow`).
    Poll { deadline: Option<Instant>, tiers: Tiers, follow: bool, watch_from: u64 },
}

/// How a request handler answers.
pub(crate) enum Reply {
    Now(Response),
    Later(Wait),
    /// Send this, then keep waiting (`poll --follow` with events already pending).
    Then(Response, Wait),
}

enum Step {
    Wait,
    Reply(Response),
    /// One `poll --follow` line; keep going.
    Stream(Response),
    /// Reply, and end the session (`new -x` whose Xwayland failed).
    ReplyEnd(Response),
}

impl Wait {
    /// Whether the reply carries the requester's undelivered events (polls deliver
    /// themselves, with their own tiers).
    fn delivers(&self) -> bool {
        !matches!(self, Wait::Poll { .. })
    }
}

/// Whether the client of a waiting request hung up (it sends nothing after its request).
fn peer_gone(stream: &UnixStream) -> bool {
    let mut b = [0u8; 64];
    match (&*stream).read(&mut b) {
        Ok(n) => n == 0,
        Err(e) => !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted),
    }
}

impl Server {
    /// Adds a waiting request. Input chains start right away (most never sleep).
    pub(crate) fn add_pending(&mut self, p: Pending) {
        let input = matches!(p.wait, Wait::Input { .. });
        self.pending.push(p);
        if input {
            self.step_pending(Instant::now(), true);
        }
    }

    /// Steps every waiting request (only input chains with `inputs_only`).
    pub(crate) fn step_pending(&mut self, now: Instant, inputs_only: bool) {
        // Sessions whose input chain earlier in the list is still running.
        let mut busy: Vec<String> = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            if inputs_only && !matches!(self.pending[i].wait, Wait::Input { .. }) {
                i += 1;
                continue;
            }
            if peer_gone(&self.pending[i].stream) {
                let p = self.pending.remove(i);
                self.abandoned(p);
                continue;
            }
            let step = self.step(i, now, &mut busy);
            let end = match step {
                Step::Wait => {
                    i += 1;
                    continue;
                }
                Step::Stream(r) => {
                    self.out.send(&self.pending[i].stream, &r);
                    i += 1;
                    continue;
                }
                Step::Reply(r) => (r, false),
                Step::ReplyEnd(r) => (r, true),
            };
            let p = self.pending.remove(i);
            let (r, end) = end;
            if end {
                self.end_session(&p.session);
            }
            let r = if p.wait.delivers() { self.attach(&p.session, p.key.as_deref(), r) } else { r };
            self.out.send(&p.stream, &r);
        }
    }

    /// The client gave up (Ctrl-C). Dropping the request stops its work; a chain that
    /// already ran lets go of what it held, like an unlock.
    fn abandoned(&mut self, p: Pending) {
        if let Wait::Input { chain, started: true, .. } = &p.wait
            && let Some(slot) = self.sessions.get_mut(&p.session)
        {
            slot.session.release_all(chain.source());
        }
    }

    fn step(&mut self, i: usize, now: Instant, busy: &mut Vec<String>) -> Step {
        let Server { pending, sessions, images, .. } = self;
        let p = &mut pending[i];
        let Some(slot) = sessions.get_mut(&p.session) else { return Step::Reply(Response::err(format!("session {} ended", p.session))) };
        let s = &mut slot.session;
        match &mut p.wait {
            Wait::Shot { path, settle, cursor, crop, max, start } => {
                let Some(ready) = s.shot_ready(now, now.duration_since(*start), *settle) else { return Step::Wait };
                let shot = images.capture(s, slot.group, *cursor, true, *crop).and_then(|id| {
                    let img = images.get(id.as_str())?;
                    let r = images.render_file(&img, &RenderOpts { max: *max, ..Default::default() }, &s.dir, path.as_deref())?;
                    Ok((img, r))
                });
                Step::Reply(match shot {
                    Ok((img, r)) => {
                        s.log.push(guilab_proto::EventData::Screenshot { image: img.id.to_string(), path: r.path.clone() }, p.key.as_deref());
                        let [cw, ch] = img.body.canvas;
                        let cropped = cropped_windows(&s.tree(), cw as i32, ch as i32);
                        let header = images.header(&img);
                        let (path, width, height, shows, zoom) = (r.path, r.width, r.height, r.shows, r.zoom);
                        Response::ok(ShotInfo { image: img.id.to_string(), header, path, width, height, shows, zoom, settled: ready.settled, animating: ready.animating, cropped })
                    }
                    Err(e) => Response::err(e),
                })
            }
            Wait::Input { chain, resume_at, started } => {
                if busy.contains(&p.session) || *resume_at > now {
                    busy.push(p.session.clone());
                    return Step::Wait;
                }
                slot.last_active = now;
                *started = true;
                match s.run_chain(chain) {
                    Ok(Some(delay)) => {
                        // From the scheduled time, so 16.7 ms glide steps don't stretch to two
                        // 16 ms ticks each.
                        *resume_at = (*resume_at + delay).max(now);
                        busy.push(p.session.clone());
                        Step::Wait
                    }
                    Ok(None) => Step::Reply(Response::ok(s.input_info(chain))),
                    Err(e) => Step::Reply(Response::err(e)),
                }
            }
            Wait::Clip { pipe, buf, deadline } => {
                let mut tmp = [0u8; 65536];
                let done = loop {
                    match pipe.read(&mut tmp) {
                        Ok(0) => break Some(Ok(())),
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break None,
                        Err(e) if e.kind() == ErrorKind::Interrupted => {}
                        Err(e) => break Some(Err(e.to_string())),
                    }
                };
                match done.or_else(|| (now >= *deadline).then(|| Err("the app owning the clipboard didn't send it".to_string()))) {
                    None => Step::Wait,
                    Some(Ok(())) => Step::Reply(Response::ok(String::from_utf8_lossy(buf))),
                    Some(Err(e)) => Step::Reply(Response::err(e)),
                }
            }
            Wait::Kill { killed, deadline } => {
                let running = s.ps().iter().any(|q| matches!(q.state, guilab_proto::ProcState::Running) && killed.iter().any(|k| k.id == q.id));
                if running && now < *deadline { Step::Wait } else { Step::Reply(Response::ok(&*killed)) }
            }
            Wait::New { deadline } => match s.x_state() {
                XState::Ready(_) => Step::Reply(Response::ok(slot.info(&p.session))),
                XState::Failed(e) => Step::ReplyEnd(Response::err(e.clone())),
                _ if now >= *deadline => Step::ReplyEnd(Response::err("Xwayland didn't start in time")),
                _ => Step::Wait,
            },
            Wait::Poll { deadline, tiers, follow, watch_from } => {
                let key = p.key.as_deref().unwrap_or_default();
                let log = &mut s.log;
                if let Some(w) = log.first_wake(key, tiers, *watch_from) {
                    let d = log.deliver(key, tiers, Some(w), true);
                    *watch_from = log.end();
                    read_messages(slot, key, &d);
                    let mut r = Response::ok(PollInfo { timed_out: false });
                    r.events = Some(d);
                    if *follow { Step::Stream(r) } else { Step::Reply(r) }
                } else if deadline.is_some_and(|d| now >= d) {
                    let d = log.deliver(key, tiers, None, true);
                    read_messages(slot, key, &d);
                    let mut r = Response::ok(PollInfo { timed_out: true });
                    r.events = (!d.events.is_empty()).then_some(d);
                    Step::Reply(r)
                } else {
                    Step::Wait
                }
            }
        }
    }

    /// Answers everything still waiting on an ended session.
    pub(crate) fn end_pending(&mut self, name: &str) {
        let ended = Response::err(format!("session {name} ended"));
        let gone: Vec<Pending> = self.pending.extract_if(.., |p| p.session == name).collect();
        for p in gone {
            let r = match &p.wait {
                Wait::Kill { killed, .. } => Response::ok(killed),
                _ => ended.clone(),
            };
            self.out.send(&p.stream, &r);
        }
    }

    /// Fails the session's running and queued input chains.
    pub(crate) fn abort_inputs(&mut self, name: &str, resp: &Response) {
        let jobs: Vec<Pending> = self.pending.extract_if(.., |p| p.session == name && matches!(p.wait, Wait::Input { .. })).collect();
        for j in jobs {
            let r = self.attach(name, j.key.as_deref(), resp.clone());
            self.out.send(&j.stream, &r);
        }
    }
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
