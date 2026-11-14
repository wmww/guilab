//! The human's actions on a session, one API for the viewer and `guilab debug` (tests act
//! as the human through it). The rules live here once: human input is blocked while an
//! agent holds the lock (releases still go through, so nothing stays stuck), human activity
//! keeps the session alive, and message images are resolved before anything is held or
//! logged.

use std::{sync::Arc, time::Instant};

use guilab_compositor::{InputEvent, InputSource};
use guilab_image::{Doc, Image, ImageId, Made, MakeReq};
use guilab_proto::EventData;

use crate::{Server, lock::LockState};

/// Why the human's input didn't go through.
pub(crate) const BLOCKED: &str = "the session is locked: human input is blocked";

/// An image attached to a human message.
pub(crate) enum MsgImage {
    /// An image id, or an absolute path of a file that becomes an image.
    Spec(String),
    /// A viewer draft: frozen into a new image (its doc carries the assets it uses).
    Draft { parent: Option<ImageId>, doc: Doc },
    /// Element lines to draw on a screenshot taken now (`debug message --draw`).
    Draw(Vec<String>),
}

/// Whether human input may reach the session now (`release`: it only lets go of something).
pub(crate) fn may_input(lock: &LockState, release: bool) -> Result<(), &'static str> {
    if lock.is_locked() && !release { Err(BLOCKED) } else { Ok(()) }
}

impl Server {
    /// One input event from the human (the viewer; `debug input` chains run through the
    /// same check per step, see `pending.rs`).
    pub(crate) fn human_input(&mut self, session: &str, ev: InputEvent, release: bool) -> Result<(), &'static str> {
        let Some(slot) = self.sessions.get_mut(session) else { return Ok(()) };
        may_input(&slot.lock, release)?;
        slot.last_active = Instant::now();
        let touch = matches!(ev, InputEvent::TouchDown { .. } | InputEvent::TouchMotion { .. } | InputEvent::TouchUp { .. });
        slot.session.inject(ev, InputSource::Human);
        if touch {
            slot.session.inject(InputEvent::TouchFrame, InputSource::Human);
        }
        Ok(())
    }

    /// The human resizes the output (viewer). Not while an agent holds the lock: that
    /// freezes the size for the human.
    pub(crate) fn human_resize(&mut self, session: &str, size: Option<(u32, u32)>, scale: Option<f64>) -> Result<(), String> {
        let slot = self.sessions.get_mut(session).ok_or_else(|| format!("no session '{session}'"))?;
        if slot.lock.is_locked() {
            return Err("the agent holds the lock; the size is frozen".into());
        }
        slot.last_active = Instant::now();
        let size = size.map(|(w, h)| (w as i32, h as i32));
        crate::requests::set_output(&mut slot.session, size, scale, None).map_err(|e| format!("{e:#}"))
    }

    /// A picture file the human picked to insert in the viewer's editor (File > Insert
    /// image). The viewer's tick loads it.
    pub(crate) fn human_insert(&mut self, session: &str, path: std::path::PathBuf) {
        if let Ok(mut p) = self.viewer_picks.lock() {
            p.push((session.to_string(), path));
        }
    }

    /// The human's Stop: drops the agent's lock. Err if it isn't locked.
    pub(crate) fn human_revoke(&mut self, session: &str) -> Result<(), String> {
        self.revoke_lock(session)
    }

    /// A screenshot for the human to draw on (whole output, no cursor). Held while the
    /// session lives: the drawing may be sent much later, and its listing names it.
    pub(crate) fn human_capture(&mut self, session: &str) -> Result<Arc<Image>, String> {
        let slot = self.sessions.get_mut(session).ok_or_else(|| format!("no session '{session}'"))?;
        slot.last_active = Instant::now();
        let id = self.images.capture(&mut slot.session, slot.group, false, false, None)?;
        let img = self.images.get(id.as_str())?;
        self.images.hold(&img);
        Ok(img)
    }

    /// Logs a human message (wakes polls). Its images are held while the session lives. A
    /// message without images gets the current screen with the pointer drawn in, since it
    /// refers to what they look at (or point at). `viewing`: an image open in their editor,
    /// not attached.
    pub(crate) fn human_message(&mut self, session: &str, text: String, imgs: Vec<MsgImage>, viewing: Option<ImageId>) -> Result<(), String> {
        let Server { sessions, images, .. } = self;
        let slot = sessions.get_mut(session).ok_or_else(|| format!("no session '{session}'"))?;
        slot.last_active = Instant::now();
        let g = slot.group;
        // Resolve every image before holding any, so a failure holds nothing.
        let mut resolved = Vec::new();
        for (i, m) in imgs.into_iter().enumerate() {
            let img = match m {
                MsgImage::Spec(spec) => images.attach(&spec, Made::Human, g),
                MsgImage::Draft { parent, doc } => images.freeze(parent, &doc, g),
                MsgImage::Draw(lines) => images.capture(&mut slot.session, g, false, false, None).and_then(|base| {
                    let req = MakeReq { from: Some(base.to_string()), elements: lines, cwd: "/".into(), ..Default::default() };
                    let id = guilab_image::make(&mut images.store, &images.fonts, &req, Made::Human, g)?;
                    images.get(id.as_str())
                }),
            }
            .map_err(|e| format!("image {}: {e}", i + 1))?;
            resolved.push(img);
        }
        let viewing = viewing.and_then(|id| images.store.image(id).ok());
        let screen = if resolved.is_empty() {
            let id = images.capture(&mut slot.session, g, true, false, None)?;
            Some(images.get(id.as_str())?)
        } else {
            None
        };
        let refs = resolved.iter().map(|img| images.image_ref(img)).collect();
        for img in resolved.iter().chain(&screen).chain(&viewing) {
            images.hold(img);
        }
        let screen = screen.map(|i| i.id.to_string());
        let viewing = viewing.map(|i| i.id.to_string());
        slot.session.log.push(EventData::Message { text, images: refs, screen, viewing }, None);
        Ok(())
    }
}
