//! Session locks. One holder per session, identified by a random token.
//!
//! - `input` always needs the lock: it competes with the human's input.
//! - Other changing commands (run, kill, output changes, clip set, end) need the token only
//!   while the session is locked, so an unlocked session stays easy to script.
//! - Read-only commands never need it.
//!
//! The viewer's Stop button goes through `Server::revoke_lock` (which logs the event); its
//! input is gated by [`LockState::human_input_allowed`].

use std::time::Instant;

use guilab_proto::Access;

pub struct Lock {
    pub token: String,
    pub since: Instant,
    pub last_used: Instant,
}

#[derive(Default)]
pub struct LockState {
    pub current: Option<Lock>,
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
    /// `guilab lock`. Returns the token (the same one if `token` already holds it).
    pub fn take(&mut self, session: &str, token: Option<&str>, force: bool) -> Result<String, String> {
        if let Some(l) = &mut self.current {
            if token == Some(l.token.as_str()) {
                l.last_used = Instant::now();
                return Ok(l.token.clone());
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
        let now = Instant::now();
        let token = new_token();
        self.current = Some(Lock { token: token.clone(), since: now, last_used: now });
        Ok(token)
    }

    /// `guilab unlock`. Ok(false) if it wasn't locked.
    pub fn release(&mut self, session: &str, token: Option<&str>, force: bool) -> Result<bool, String> {
        let Some(l) = &self.current else { return Ok(false) };
        if !force && token != Some(l.token.as_str()) {
            return Err(self.bad_token(session, token));
        }
        self.current = None;
        Ok(true)
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

    /// Viewer's Revoke button: drops the lock; its token stops working.
    pub fn revoke(&mut self) {
        if let Some(l) = self.current.take() {
            self.revoked = Some(l.token);
        }
    }

    pub fn human_input_allowed(&self) -> bool {
        self.current.is_none()
    }
}
