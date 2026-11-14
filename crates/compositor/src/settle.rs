//! Settle detection: whether a session's apps finished drawing, so a screenshot shows the
//! result of what came before it. The timestamps it needs are noted through [`Times`]
//! (commits, input, configures, spawns, maps, renders), each from one place.

use std::time::{Duration, Instant};

use smithay::desktop::PopupManager;

use crate::{
    session::Session,
    shell::{has_buffer, has_pending_configure},
};

/// No commits for this long counts as "done drawing" (a blinking cursor still passes).
const SETTLE_QUIET: Duration = Duration::from_millis(150);
/// Commits without a quiet gap for this long count as an animation (spinner, video): its
/// screenshots stop waiting for quiet (see [`Settle::Animating`]).
const ANIM_DETECT: Duration = Duration::from_secs(1);
/// While animating, how long after the last input a screenshot waits (the app reacting).
const ANIM_REACT: Duration = Duration::from_millis(300);
/// A window from the last `run` is expected this long; until then the session is busy.
const NEW_APP_WAIT: Duration = Duration::from_secs(3);
/// How long an unanswered configure (not acked, or acked but not committed) keeps a session
/// busy. Bounded because clients may ack without committing when nothing changes for them.
const CONFIGURE_WAIT: Duration = Duration::from_secs(1);

/// Screenshot wait caps: once a window is up, and while waiting for the first window.
const SETTLE_CAP: Duration = Duration::from_secs(3);
const MAP_CAP: Duration = Duration::from_secs(10);

/// What the settle decision needs to know, noted where it happens.
pub(crate) struct Times {
    commit: Instant,
    render: Instant,
    /// Last toplevel configure we sent.
    configure: Instant,
    /// Last input delivered (the app hasn't necessarily reacted yet).
    input: Instant,
    /// Start of the current run of commits without a `SETTLE_QUIET` gap (animation check).
    busy_since: Instant,
    /// The last `run`: when, and the app's id (settle waits for its first window).
    spawn: Option<(Instant, u32)>,
    /// When a toplevel last appeared.
    map: Instant,
}

impl Times {
    pub fn new() -> Times {
        let now = Instant::now();
        Times { commit: now, render: now, configure: now, input: now, busy_since: now, spawn: None, map: now }
    }

    pub fn note_commit(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.commit) >= SETTLE_QUIET {
            self.busy_since = now;
        }
        self.commit = now;
    }

    pub fn note_render(&mut self) {
        self.render = Instant::now();
    }

    pub fn note_configure(&mut self) {
        self.configure = Instant::now();
    }

    pub fn note_input(&mut self) {
        self.input = Instant::now();
    }

    pub fn note_spawn(&mut self, id: u32) {
        self.spawn = Some((Instant::now(), id));
    }

    pub fn note_map(&mut self) {
        self.map = Instant::now();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// No toplevel has a buffer yet.
    NoWindow,
    Busy,
    /// Done except that something keeps redrawing: as settled as it gets.
    Animating,
    Settled,
}

/// A screenshot may be taken now.
#[derive(Debug, Clone, Copy)]
pub struct ShotReady {
    /// False if the wait ran out or the app animates.
    pub settled: bool,
    pub animating: bool,
}

impl Session {
    /// Whether the apps finished drawing.
    pub fn settle_state(&self, now: Instant) -> Settle {
        let t = &self.settle;
        let mapped: Vec<_> = self.space.elements().filter_map(|w| w.toplevel()).collect();
        // A configured toplevel without a buffer is still drawing its first frame; so is
        // a mapped X window without one.
        let mut drawing = self.unmapped.iter().any(|w| w.toplevel().is_some_and(|t| t.is_initial_configure_sent()));
        let mut x_windows = 0;
        for x in self.space.elements().filter_map(|w| w.x11_surface()).filter(|x| !x.is_override_redirect()) {
            if x.wl_surface().is_some_and(|s| has_buffer(&s)) {
                x_windows += 1;
            } else {
                drawing = true;
            }
        }
        if mapped.is_empty() && x_windows == 0 {
            return Settle::NoWindow;
        }
        // A popup (menu) that hasn't drawn its first frame yet.
        drawing |= mapped.iter().any(|t| PopupManager::popups_for_surface(t.wl_surface()).any(|(p, _)| !has_buffer(p.wl_surface())));
        let acked = now.duration_since(t.configure) >= CONFIGURE_WAIT
            || mapped.iter().all(|t| !has_pending_configure(t.wl_surface()));
        // The app from the last `run` hasn't shown a window yet (another one is up).
        let starting = t.spawn.is_some_and(|(at, id)| at > t.map && now.duration_since(at) < NEW_APP_WAIT && self.is_running(id));
        if drawing || !acked || starting {
            return Settle::Busy;
        }
        // Input counts as activity: the app hasn't necessarily reacted (committed) yet.
        let since_input = now.duration_since(t.input);
        let quiet = now.duration_since(t.commit) >= SETTLE_QUIET && since_input >= SETTLE_QUIET;
        let drawn = t.render > t.commit;
        if quiet && drawn {
            return Settle::Settled;
        }
        let animating = !quiet && now.duration_since(t.busy_since) >= ANIM_DETECT;
        if animating && since_input >= ANIM_REACT && drawn { Settle::Animating } else { Settle::Busy }
    }

    /// Whether a screenshot requested `waited` ago may be taken now. `wait`: wait for the
    /// apps to settle (capped); else take it at once.
    pub fn shot_ready(&self, now: Instant, waited: Duration, wait: bool) -> Option<ShotReady> {
        let state = self.settle_state(now);
        let settled = if !wait {
            Some(state == Settle::Settled)
        } else {
            match state {
                Settle::Settled => Some(true),
                Settle::Animating => Some(false),
                // Nothing that could ever draw: don't wait.
                Settle::NoWindow if self.running_apps() == 0 => Some(true),
                Settle::NoWindow => (waited >= MAP_CAP).then_some(false),
                Settle::Busy => (waited >= SETTLE_CAP).then_some(false),
            }
        };
        settled.map(|settled| ShotReady { settled, animating: state == Settle::Animating })
    }
}
