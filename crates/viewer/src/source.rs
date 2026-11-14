//! A calloop source for a host Wayland connection whose failures stay local.
//!
//! Based on calloop-wayland-source 0.4 (MIT). The original returns host I/O and protocol
//! errors from `EventLoop::dispatch`, which would stop the whole server. Here any error
//! ends the source instead: the callback gets [`HostEvent::Lost`] once and the source
//! removes itself.

use std::io;

use calloop::{EventSource, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory, generic::Generic};
use wayland_backend::client::{ReadEventsGuard, WaylandError};
use wayland_client::{Connection, DispatchError, EventQueue};

pub enum HostEvent {
    /// Dispatch pending events on the queue (`queue.dispatch_pending(state)`).
    Dispatch,
    /// The connection failed; the source is gone after this.
    Lost(String),
}

pub struct HostSource<D> {
    queue: EventQueue<D>,
    conn: Generic<Connection>,
    guard: Option<ReadEventsGuard>,
    /// Returned from `before_sleep` to get `process_events` called without fd readiness.
    fake_token: Option<Token>,
    lost: Option<String>,
}

impl<D> HostSource<D> {
    pub fn new(conn: Connection, queue: EventQueue<D>) -> Self {
        HostSource { queue, conn: Generic::new(conn, Interest::READ, Mode::Level), guard: None, fake_token: None, lost: None }
    }

    fn flush(&mut self) {
        if self.lost.is_some() {
            return;
        }
        if let Err(e) = self.queue.flush() {
            match e {
                WaylandError::Io(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                e => self.lost = Some(format!("flush: {e}")),
            }
        }
    }
}

impl<D> EventSource for HostSource<D> {
    type Event = HostEvent;
    type Metadata = EventQueue<D>;
    type Ret = Result<usize, DispatchError>;
    type Error = io::Error;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    fn process_events<F>(&mut self, _: Readiness, _: Token, mut callback: F) -> Result<PostAction, io::Error>
    where
        F: FnMut(HostEvent, &mut EventQueue<D>) -> Result<usize, DispatchError>,
    {
        if self.lost.is_none() {
            loop {
                match callback(HostEvent::Dispatch, &mut self.queue) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => {
                        self.lost = Some(format!("dispatch: {e}"));
                        break;
                    }
                }
            }
            self.flush();
        }
        // A protocol error leaves nothing to read; ask the connection directly.
        if self.lost.is_none()
            && let Some(e) = self.conn.get_ref().protocol_error()
        {
            self.lost = Some(format!("protocol error: {}", e.message));
        }
        match self.lost.take() {
            Some(why) => {
                let _ = callback(HostEvent::Lost(why), &mut self.queue);
                Ok(PostAction::Remove)
            }
            None => Ok(PostAction::Continue),
        }
    }

    fn register(&mut self, poll: &mut Poll, f: &mut TokenFactory) -> calloop::Result<()> {
        self.fake_token = Some(f.token());
        self.conn.register(poll, f)
    }

    fn reregister(&mut self, poll: &mut Poll, f: &mut TokenFactory) -> calloop::Result<()> {
        self.conn.reregister(poll, f)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        self.conn.unregister(poll)
    }

    fn before_sleep(&mut self) -> calloop::Result<Option<(Readiness, Token)>> {
        self.guard = None;
        self.flush();
        if self.lost.is_none() {
            self.guard = self.queue.prepare_read();
            if self.guard.is_some() {
                return Ok(None);
            }
        }
        // Events already queued, or an error to report: process right away.
        Ok(self.fake_token.map(|t| (Readiness::EMPTY, t)))
    }

    fn before_handle_events(&mut self, events: calloop::EventIterator<'_>) {
        let guard = self.guard.take();
        if events.count() > 0
            && let Some(Err(e)) = guard.map(ReadEventsGuard::read)
        {
            match e {
                WaylandError::Io(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                e => self.lost = Some(format!("read: {e}")),
            }
        }
    }
}
