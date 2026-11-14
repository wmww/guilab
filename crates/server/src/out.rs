//! Replies to clients without blocking the server's only thread. A reply that doesn't fit
//! in the socket buffer is kept and flushed from the tick; a client that stops reading
//! (e.g. `poll --follow` piped into a paused process) is cut off once too much piles up.

use std::{
    io::{ErrorKind, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use guilab_proto::Response;

/// Unsent bytes per client before it's cut off.
const CAP: usize = 8 << 20;
/// Time without any progress before a client is cut off.
const STUCK: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct Outbox {
    queued: Vec<Queued>,
}

struct Queued {
    /// The socket's inode: the same for every clone of a connection's stream, so replies
    /// sent through different clones stay in order.
    ino: u64,
    stream: UnixStream,
    buf: Vec<u8>,
    /// Last time any of it was written.
    progress: Instant,
}

fn ino(s: &UnixStream) -> u64 {
    rustix::fs::fstat(s).map(|st| st.st_ino).unwrap_or(0)
}

/// Writes what fits without blocking (the streams are non-blocking).
fn write_some(mut s: &UnixStream, buf: &[u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match s.write(&buf[n..]) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(k) => n += k,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Gives up on a client: the shutdown also makes `pending::peer_gone` see it as gone.
fn cut_off(s: &UnixStream) {
    tracing::debug!("client stopped reading its replies; disconnecting it");
    let _ = s.shutdown(std::net::Shutdown::Both);
}

impl Outbox {
    /// Sends one response line.
    pub fn send(&mut self, stream: &UnixStream, resp: &Response) {
        let mut line = serde_json::to_vec(resp).unwrap_or_else(|e| {
            serde_json::to_vec(&Response::err(format!("internal: encoding the reply: {e}"))).expect("plain error encodes")
        });
        line.push(b'\n');
        let ino = ino(stream);
        if let Some(q) = self.queued.iter_mut().find(|q| q.ino == ino) {
            q.buf.extend_from_slice(&line);
            if q.buf.len() > CAP {
                cut_off(&q.stream);
                self.queued.retain(|q| q.ino != ino);
            }
            return;
        }
        match write_some(stream, &line) {
            Ok(n) if n == line.len() => {}
            Ok(n) => match stream.try_clone() {
                Ok(stream) => self.queued.push(Queued { ino, stream, buf: line[n..].to_vec(), progress: Instant::now() }),
                Err(e) => tracing::debug!("reply: {e}"),
            },
            Err(e) => tracing::debug!("reply: {e}"),
        }
    }

    /// From the tick: writes what the clients take now.
    pub fn flush(&mut self, now: Instant) {
        self.queued.retain_mut(|q| match write_some(&q.stream, &q.buf) {
            Ok(n) => {
                q.buf.drain(..n);
                if n > 0 {
                    q.progress = now;
                }
                if !q.buf.is_empty() && now.duration_since(q.progress) >= STUCK {
                    cut_off(&q.stream);
                    return false;
                }
                !q.buf.is_empty()
            }
            Err(e) => {
                tracing::debug!("reply: {e}");
                false
            }
        });
    }
}
