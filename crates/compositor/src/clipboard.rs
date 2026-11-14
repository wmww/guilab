//! `clip get` / `clip set`: the session's clipboard (wl_data_device selection), text only.
//!
//! Set: the server owns the selection and writes the text to whoever pastes. Get: our own
//! text directly, or a pipe the owning client writes into (read by the server, async).
//! X clients' selections are bridged by the X11Wm (xwayland.rs), so this covers them too.

use std::{
    io::Write,
    os::fd::OwnedFd,
    sync::Arc,
};

use smithay::wayland::selection::{
    SelectionTarget,
    data_device::{current_data_device_selection_userdata, request_data_device_client_selection, set_data_device_selection},
};

use crate::{handlers::SelData, session::Session};

/// Text mime types, preferred first.
const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"];

/// The clipboard as the viewer bridge and `clip get` see it. Every change of owner or
/// content goes through `set_owner`, which bumps the seq.
#[derive(Default)]
pub struct Clipboard {
    /// Mime types of a client-owned selection (None: empty or ours).
    mimes: Option<Vec<String>>,
    seq: u64,
}

impl Clipboard {
    /// Changes with every new owner or content.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// A new owner: a client with these mime types, or None (ours, or empty).
    pub(crate) fn set_owner(&mut self, mimes: Option<Vec<String>>) {
        self.mimes = mimes;
        self.seq += 1;
    }
}

pub enum ClipRead {
    Text(String),
    /// Read this pipe to EOF.
    Pipe(OwnedFd),
}

impl Session {
    pub fn clip_set(&mut self, text: String) {
        let mimes: Vec<String> = TEXT_MIMES.iter().map(|s| s.to_string()).collect();
        set_data_device_selection(&self.dh, &self.seat, mimes.clone(), SelData::Text(Arc::from(text)));
        self.clip.set_owner(None);
        crate::xwayland::announce_selection(self, SelectionTarget::Clipboard, Some(mimes));
        self.flush();
    }

    pub fn clip_get(&mut self) -> Result<ClipRead, String> {
        let x_owned = match current_data_device_selection_userdata(&self.seat).as_deref() {
            Some(SelData::Text(t)) => return Ok(ClipRead::Text(t.to_string())),
            Some(SelData::X11) => true,
            None => false,
        };
        let Some(mimes) = &self.clip.mimes else { return Err("the clipboard is empty".into()) };
        let Some(mime) = TEXT_MIMES.iter().find(|m| mimes.iter().any(|x| x == *m)) else {
            return Err(format!("the clipboard has no text (types: {})", mimes.join(", ")));
        };
        let (r, w) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(|e| e.to_string())?;
        if x_owned {
            crate::xwayland::read_x_selection(self, mime.to_string(), w)?;
        } else {
            request_data_device_client_selection(&self.seat, mime.to_string(), w).map_err(|e| format!("reading the clipboard: {e}"))?;
        }
        self.flush();
        Ok(ClipRead::Pipe(r))
    }
}

/// Writes server-owned clipboard text to a pasting client, off the event loop
/// (a client that doesn't read must not block the session).
pub(crate) fn send_text(text: Arc<str>, fd: OwnedFd) {
    std::thread::spawn(move || {
        let mut f = std::fs::File::from(fd);
        let _ = f.write_all(text.as_bytes());
    });
}
