//! A session as the server sees it: the compositor session plus the agent's state (lock,
//! working status, activity). Every lock and status transition goes through here, so each
//! logs its event and cleans up held input in one place.

use std::{path::PathBuf, time::Instant};

use calloop::RegistrationToken;
use guilab_compositor::{InputSource, Session};
use guilab_proto::{EventData, SessionInfo};

use crate::{Server, lock::LockState};

pub(crate) struct Slot {
    pub session: Session,
    pub root: Option<PathBuf>,
    pub ttl: u64,
    pub last_active: Instant,
    pub lock: LockState,
    /// `last_active` when the expiry warning was logged (once per idle stretch).
    pub warned: Option<Instant>,
    /// The agent's status while it works (`guilab work`); None while it waits for the human.
    pub working: Option<Working>,
    /// The agent's last command (the viewer shows how long it's been quiet).
    pub last_agent: Instant,
    /// The session's images in the store (dropped with it).
    pub group: guilab_image::Group,
    pub token: RegistrationToken,
    /// The screen as the human started typing a message in the viewer (pointer drawn in):
    /// what a message without images refers to. Taken by the next viewer message.
    pub typing_screen: Option<guilab_image::ImageId>,
}

pub(crate) struct Working {
    pub status: String,
    /// When this stretch of work began (the first `work` after `done`).
    pub since: Instant,
}

/// Why a lock ended. The lock state itself has already changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockEnd {
    Unlock,
    Done,
    /// The human's Stop.
    Revoke,
    /// `lock --force` by someone else.
    TakenOver,
}

impl LockEnd {
    /// The error the ended holder's input chains fail with.
    fn abort_msg(self) -> &'static str {
        match self {
            LockEnd::Unlock => "the lock was released; input stopped",
            LockEnd::Done => "the lock was released (guilab done); input stopped",
            LockEnd::Revoke => "the human revoked your lock; input stopped",
            LockEnd::TakenOver => "your lock was taken over (guilab lock --force)",
        }
    }
}

impl Slot {
    pub fn new(session: Session, root: Option<PathBuf>, ttl: u64, group: guilab_image::Group, token: RegistrationToken) -> Slot {
        let now = Instant::now();
        Slot { session, root, ttl, last_active: now, lock: LockState::default(), warned: None, working: None, last_agent: now, group, token, typing_screen: None }
    }

    pub fn info(&self, name: &str) -> SessionInfo {
        let s = &self.session;
        SessionInfo {
            name: name.to_string(),
            wayland_display: s.socket_path(),
            x_display: s.x_display(),
            dir: s.dir.clone(),
            root: self.root.clone(),
            output: s.output_info(),
            apps: s.running_apps(),
            windows: s.windows(),
            ttl: self.ttl,
            expires_in: (self.ttl > 0).then(|| self.ttl.saturating_sub(self.last_active.elapsed().as_secs())),
            working: self.working.as_ref().map(|w| w.status.clone()),
            in_viewer: false,
        }
    }

    /// Logs the end of a lock and releases what the agent still held (keys, buttons,
    /// touches, a gesture). Only `Server::end_lock` calls this; it also stops the chains.
    fn lock_ended(&mut self, why: LockEnd, key: Option<&str>) {
        let data = match why {
            LockEnd::Unlock | LockEnd::Done => EventData::Unlock,
            LockEnd::Revoke => EventData::Revoke,
            LockEnd::TakenOver => EventData::Lock { forced: true },
        };
        self.session.log.push(data, key);
        self.session.release_all(InputSource::Agent);
    }

    /// The agent works on `status` (`work`, or a poll that showed it a human message).
    pub fn set_working(&mut self, status: String, key: &str) {
        self.session.log.push(EventData::Work { status: status.clone() }, Some(key));
        match &mut self.working {
            Some(w) => w.status = status,
            None => self.working = Some(Working { status, since: Instant::now() }),
        }
    }

    /// The agent waits for the human (`done`; its lock is handled by the caller).
    pub fn set_done(&mut self, key: &str) {
        self.session.log.push(EventData::Done, Some(key));
        self.working = None;
    }
}

impl Server {
    /// The one path for a lock that ended (its `LockState` already changed): logs it,
    /// releases the agent's held input and fails the session's input chains.
    pub(crate) fn end_lock(&mut self, name: &str, why: LockEnd, key: Option<&str>) {
        let Some(slot) = self.sessions.get_mut(name) else { return };
        slot.lock_ended(why, key);
        self.abort_inputs(name, &guilab_proto::Response::err(why.abort_msg()));
    }

    /// The human's Stop (viewer or `debug revoke`). Err if the session isn't locked.
    pub(crate) fn revoke_lock(&mut self, name: &str) -> Result<(), String> {
        let slot = self.sessions.get_mut(name).ok_or_else(|| format!("no session '{name}'"))?;
        if !slot.lock.revoke() {
            return Err("not locked".into());
        }
        self.end_lock(name, LockEnd::Revoke, None);
        Ok(())
    }
}
