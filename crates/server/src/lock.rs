//! Session locks. One holder per session, identified by a random token.
//!
//! - `input` always needs the lock: it competes with the human's input.
//! - Other changing commands (run, kill, output changes, clip set, end) need the token only
//!   while the session is locked, so an unlocked session stays easy to script.
//! - Read-only commands never need it.
//!
//! Pure token logic. Ending a lock has side effects (event, held input, input chains), so
//! callers go through `Server::end_lock`, never just these methods.

use std::time::Instant;

use guilab_proto::Access;

struct Lock {
    token: String,
    since: Instant,
    last_used: Instant,
}

/// How `take` got the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Took {
    /// The token already held it.
    Same,
    /// It was free.
    New,
    /// `--force` took it from another holder.
    TakenOver,
}

#[derive(Default)]
pub struct LockState {
    current: Option<Lock>,
    /// Token of the last revoked lock, to tell its holder what happened.
    revoked: Option<String>,
}

fn new_token() -> String {
    use std::io::Read;
    let mut b = [0u8; 8];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        b = (t as u64 ^ std::process::id() as u64).to_le_bytes();
    }
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    b.iter().map(|x| ALPHABET[(*x as usize) % ALPHABET.len()] as char).collect()
}

fn ago(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    if s < 120 { format!("{s}s ago") } else { format!("{}m ago", s / 60) }
}

impl LockState {
    pub fn is_locked(&self) -> bool {
        self.current.is_some()
    }

    /// `guilab lock`. Returns the token (the same one if `token` already holds it).
    pub fn take(&mut self, session: &str, token: Option<&str>, force: bool) -> Result<(String, Took), String> {
        if let Some(l) = &mut self.current {
            if token == Some(l.token.as_str()) {
                l.last_used = Instant::now();
                return Ok((l.token.clone(), Took::Same));
            }
            if !force {
                return Err(format!(
                    "session '{session}' is already locked (since {}, last used {}). Wait for the holder to \
                     `guilab unlock`, or take it over with `guilab lock --force` if they're gone. If it's your own \
                     lock and you lost the token: `guilab lock --force` gives you a new one.",
                    ago(l.since),
                    ago(l.last_used)
                ));
            }
        }
        let took = if self.current.is_some() { Took::TakenOver } else { Took::New };
        let now = Instant::now();
        let token = new_token();
        self.current = Some(Lock { token: token.clone(), since: now, last_used: now });
        Ok((token, took))
    }

    /// Whether `token` may release the lock (any token with `force`). Ok(false) if it isn't
    /// locked. Changes nothing, so callers can do their other fallible work before `release`.
    pub fn may_release(&self, session: &str, token: Option<&str>, force: bool) -> Result<bool, String> {
        match &self.current {
            None => Ok(false),
            Some(l) if force || token == Some(l.token.as_str()) => Ok(true),
            Some(_) => Err(self.bad_token(session, token)),
        }
    }

    pub fn release(&mut self) {
        self.current = None;
    }

    /// Whether a command with this access may run with this token.
    pub fn check(&mut self, session: &str, access: Access, token: Option<&str>) -> Result<(), String> {
        if access == Access::Read {
            return Ok(());
        }
        match &mut self.current {
            Some(l) if token == Some(l.token.as_str()) => {
                l.last_used = Instant::now();
                Ok(())
            }
            Some(_) => Err(self.bad_token(session, token)),
            None if token.is_some() => Err(self.bad_token(session, token)),
            None if access == Access::Drive => Err("input needs the session lock. Run `guilab lock` (prints a token), \
                 then pass it: guilab -t TOKEN input ... (or set GUILAB_TOKEN)"
                .into()),
            None => Ok(()),
        }
    }

    fn bad_token(&self, session: &str, token: Option<&str>) -> String {
        match (token, &self.current) {
            (Some(t), _) if self.revoked.as_deref() == Some(t) => format!(
                "the human revoked your lock on session '{session}'. Don't take it back until they hand control \
                 back; then run `guilab lock` for a new token."
            ),
            (None, Some(_)) => format!(
                "session '{session}' is locked; pass the token from `guilab lock`: guilab -t TOKEN ... (or set \
                 GUILAB_TOKEN). If you didn't lock it, someone else holds it: wait, or take it over with \
                 `guilab lock --force` if they're gone."
            ),
            (Some(t), Some(_)) => format!(
                "token {t} doesn't hold the lock on session '{session}' (someone else locked it). Wait, or take it \
                 over with `guilab lock --force` if they're gone."
            ),
            (Some(t), None) => format!(
                "token {t} doesn't hold the lock on session '{session}' (it was unlocked). Run `guilab lock` for a \
                 new token, or drop -t / GUILAB_TOKEN."
            ),
            (None, None) => "not locked".into(),
        }
    }

    /// The human's Stop: drops the lock; its token stops working. False if it wasn't locked.
    pub fn revoke(&mut self) -> bool {
        let Some(l) = self.current.take() else { return false };
        self.revoked = Some(l.token);
        true
    }
}
