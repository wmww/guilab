//! The host's clipboard (`wl_data_device` selection), text only: what the owner of the
//! viewer bridges to and from a session. Reads and writes run on threads, so a peer that
//! never closes its pipe can't stall the server.

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
};

use sctk::{
    data_device_manager::{
        DataDeviceManagerState, WritePipe,
        data_device::{DataDevice, DataDeviceHandler},
        data_offer::{DataOfferHandler, DragOffer},
        data_source::{CopyPasteSource, DataSourceHandler},
    },
    reexports::client::protocol::{wl_data_device::WlDataDevice, wl_data_device_manager::DndAction, wl_data_source::WlDataSource, wl_seat::WlSeat, wl_surface::WlSurface},
};
use wayland_client::{Connection, QueueHandle};

use crate::host::HostState;

/// Text mime types, preferred first.
const TEXT_MIMES: [&str; 4] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING"];
/// Clipboard text larger than this is ignored.
const MAX_TEXT: u64 = 4 << 20;

#[derive(Default)]
pub(crate) struct HostClipboard {
    manager: Option<DataDeviceManagerState>,
    device: Option<DataDevice>,
    /// Our selection on the host, and its text.
    source: Option<(CopyPasteSource, Arc<str>)>,
    /// Text to put on the host once we have a serial (focus).
    pending: Option<String>,
    /// Text read from a new host selection, for the owner to take.
    incoming: Arc<Mutex<Option<String>>>,
    /// The host clipboard's text as last known (ours or read from someone else's): what
    /// pasting into the viewer's own text fields inserts.
    known: Arc<Mutex<Option<Arc<str>>>>,
}

impl HostClipboard {
    pub(crate) fn new(globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<HostState>) -> HostClipboard {
        HostClipboard { manager: DataDeviceManagerState::bind(globals, qh).ok(), ..Default::default() }
    }

    pub(crate) fn add_seat(&mut self, qh: &QueueHandle<HostState>, seat: &WlSeat) {
        if self.device.is_none()
            && let Some(m) = &self.manager
        {
            self.device = Some(m.get_data_device(qh, seat));
        }
    }
}

impl HostState {
    /// Puts `text` on the host clipboard (now if the window has had input, else on focus).
    pub fn set_clipboard(&mut self, text: String) {
        self.clipboard.pending = Some(text);
        self.flush_clipboard();
    }

    /// Applies a pending `set_clipboard` once there's an input serial to do it with.
    pub(crate) fn flush_clipboard(&mut self) {
        let Some(serial) = self.input_serial else { return };
        let c = &mut self.clipboard;
        let (Some(m), Some(dev)) = (&c.manager, &c.device) else { return };
        let Some(text) = c.pending.take() else { return };
        let source = m.create_copy_paste_source(&self.qh, TEXT_MIMES);
        source.set_selection(dev, serial);
        let text: Arc<str> = Arc::from(text);
        if let Ok(mut k) = c.known.lock() {
            *k = Some(text.clone());
        }
        c.source = Some((source, text));
    }

    /// Text the host clipboard got from someone else since the last call.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.incoming.lock().ok()?.take()
    }

    /// The host clipboard's text as last known.
    pub(crate) fn clipboard_text(&self) -> Option<String> {
        self.clipboard.known.lock().ok()?.as_deref().map(str::to_string)
    }
}

impl DataDeviceHandler for HostState {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64, _: &WlSurface) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {
        let c = &self.clipboard;
        // Our own selection coming back.
        if c.source.is_some() {
            return;
        }
        let forget = || {
            if let Ok(mut k) = c.known.lock() {
                *k = None;
            }
        };
        let Some(offer) = c.device.as_ref().and_then(|d| d.data().selection_offer()) else { return forget() };
        let Some(mime) = offer.with_mime_types(|m| TEXT_MIMES.iter().find(|t| m.iter().any(|x| x == *t)).map(|t| t.to_string())) else { return forget() };
        forget();
        let Ok(pipe) = offer.receive(mime) else { return };
        let (incoming, known) = (c.incoming.clone(), c.known.clone());
        let spawned = std::thread::Builder::new().name("host-clipboard-read".into()).spawn(move || {
            let mut buf = Vec::new();
            if std::fs::File::from(std::os::fd::OwnedFd::from(pipe)).take(MAX_TEXT).read_to_end(&mut buf).is_ok()
                && let Ok(text) = String::from_utf8(buf)
            {
                if let Ok(mut k) = known.lock() {
                    *k = Some(Arc::from(text.as_str()));
                }
                if let Ok(mut i) = incoming.lock() {
                    *i = Some(text);
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("host clipboard reader: {e}");
        }
    }
}

impl DataOfferHandler for HostState {
    fn source_actions(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
    fn selected_action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
}

impl DataSourceHandler for HostState {
    fn accept_mime(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: Option<String>) {}

    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource, _: String, fd: WritePipe) {
        let Some((s, text)) = &self.clipboard.source else { return };
        if s.inner() != source {
            return;
        }
        let text = text.clone();
        let _ = std::thread::Builder::new().name("host-clipboard-write".into()).spawn(move || {
            let _ = std::fs::File::from(std::os::fd::OwnedFd::from(fd)).write_all(text.as_bytes());
        });
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self.clipboard.source.as_ref().is_some_and(|(s, _)| s.inner() == source) {
            self.clipboard.source = None;
        }
        source.destroy();
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

sctk::delegate_data_device!(HostState);
