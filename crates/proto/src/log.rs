//! The per-session event log store: append-only, with motion/scroll coalescing, a
//! delivery cursor per client key, and the unread mark for human messages.
//!
//! Delivery keys: the client's git worktree root (one agent per worktree), or [`ANON`]
//! outside git. A delivery hands a client every non-log event after its cursor (minus
//! events its own commands caused) and moves the cursor to the end. Several pollers on one
//! key: each wake event reaches all of them (see `deliver`'s `from`); passive events go to
//! whichever delivery comes first.
//!
//! Human messages are never consumed by a command reply (agents trim and drop stderr):
//! replies only count them as unread, and they stay unread for the whole session until a
//! poll prints them. So a message can't be lost, and is shown in full exactly once.

use std::collections::{HashMap, VecDeque};

use crate::events::{Delivery, Event, EventData, LogInfo, ResolvedTiers, Tier, Unread, now_ms};

fn is_message(e: &Event) -> bool {
    matches!(e.data, EventData::Message { .. })
}

/// Delivery key of clients outside a git worktree.
pub const ANON: &str = "";

/// Events kept in memory; older ones are dropped.
const MAX_EVENTS: usize = 100_000;
/// Delivery keys kept (least recently used dropped).
const MAX_KEYS: usize = 32;
/// Motion/scroll closer than this to the previous one extends it.
const MERGE_GAP_MS: u64 = 500;
const MAX_PATH: usize = 2000;

struct Cursor {
    seq: u64,
    /// Time of the last delivery.
    last: Option<u64>,
    used: u64,
}

#[derive(Default)]
pub struct EventLog {
    events: VecDeque<Event>,
    /// Seq of the next event.
    next: u64,
    dropped: u64,
    /// The last event may still absorb more motion/scroll.
    open: bool,
    cursors: HashMap<String, Cursor>,
    uses: u64,
    /// Human messages from this seq on haven't been shown by a poll.
    read: u64,
}

impl EventLog {
    pub fn push(&mut self, data: EventData, by: Option<&str>) -> u64 {
        self.push_at(now_ms(), data, by)
    }

    pub fn push_at(&mut self, t: u64, data: EventData, by: Option<&str>) -> u64 {
        if self.open
            && let Some(last) = self.events.back_mut()
            && last.by.as_deref() == by
        {
            match (&mut last.data, &data) {
                (EventData::Motion { path }, EventData::Motion { path: p }) => {
                    let end = last.t + path.last().map_or(0, |q| q[2].max(0) as u64);
                    if t >= end && t - end <= MERGE_GAP_MS && path.len() + p.len() <= MAX_PATH {
                        let off = (t - last.t) as i32;
                        path.extend(p.iter().map(|q| [q[0], q[1], q[2] + off]));
                        return last.seq;
                    }
                }
                (
                    EventData::Scroll { dx, dy, x, y, smooth },
                    EventData::Scroll { dx: ndx, dy: ndy, x: nx, y: ny, smooth: ns },
                ) if smooth == ns && t >= last.t && t - last.t <= MERGE_GAP_MS => {
                    *dx += ndx;
                    *dy += ndy;
                    (*x, *y) = (*nx, *ny);
                    return last.seq;
                }
                _ => {}
            }
        }
        self.open = matches!(data, EventData::Motion { .. } | EventData::Scroll { .. });
        let seq = self.next;
        self.next += 1;
        self.events.push_back(Event { seq, t, data, tier: None, by: by.map(str::to_string) });
        if self.events.len() > MAX_EVENTS {
            self.events.pop_front();
            self.dropped += 1;
        }
        seq
    }

    /// Stops the last event from absorbing more motion (so new motion gets a new seq).
    pub fn seal(&mut self) {
        self.open = false;
    }

    /// Seq the next event will get.
    pub fn end(&self) -> u64 {
        self.next
    }

    fn cursor(&self, key: &str) -> u64 {
        self.cursors.get(key).or_else(|| self.cursors.get(ANON)).map_or(0, |c| c.seq)
    }

    fn evict(&mut self) {
        while self.cursors.len() > MAX_KEYS {
            let Some(k) = self.cursors.iter().filter(|(k, _)| k.as_str() != ANON).min_by_key(|(_, c)| c.used).map(|(k, _)| k.clone()) else {
                break;
            };
            self.cursors.remove(&k);
        }
    }

    /// Non-log events from seq `from` on that `key`'s own commands didn't cause.
    fn visible<'a>(&'a self, key: &'a str, tiers: &'a ResolvedTiers, from: u64) -> impl Iterator<Item = (&'a Event, Tier)> + 'a {
        self.from_seq(from)
            .filter(move |e| e.by.as_deref() != Some(key))
            .map(move |e| (e, tiers.tier(&e.data)))
            .filter(|(_, t)| *t != Tier::Log)
    }

    /// Human messages no poll has shown yet.
    pub fn unread(&self) -> impl Iterator<Item = &Event> {
        self.from_seq(self.read).filter(|e| is_message(e))
    }

    /// First wake event for `key` at or after `from` (read or not: a sibling poller may
    /// have just shown it).
    pub fn first_wake(&self, key: &str, tiers: &ResolvedTiers, from: u64) -> Option<u64> {
        self.visible(key, tiers, from).find(|(_, t)| *t == Tier::Wake).map(|(e, _)| e.seq)
    }

    /// Whether a poll by `key` would return at once: an unread message, or another wake
    /// event after its cursor.
    pub fn has_wake(&self, key: &str, tiers: &ResolvedTiers) -> bool {
        self.unread().any(|e| tiers.tier(&e.data) == Tier::Wake)
            || self.visible(key, tiers, self.cursor(key)).any(|(e, t)| t == Tier::Wake && !is_message(e))
    }

    /// Everything `key` hasn't seen (from its cursor, or from `from` if earlier: a wake
    /// event another poller of the same key already got), and moves its cursor to the end.
    /// A `poll` delivery also includes every unread message and marks them read; a command
    /// reply leaves messages out and only reports them as [`Delivery::unread`].
    pub fn deliver(&mut self, key: &str, tiers: &ResolvedTiers, from: Option<u64>, poll: bool) -> Delivery {
        let c = self.cursor(key);
        let start = from.map_or(c, |f| f.min(c));
        let msgs_from = from.map_or(self.read, |f| f.min(self.read));
        let mut events: Vec<Event> = self
            .visible(key, tiers, start.min(if poll { msgs_from } else { u64::MAX }))
            .filter(|(e, _)| if is_message(e) { poll && e.seq >= msgs_from } else { e.seq >= start })
            .map(|(e, t)| Event { tier: Some(t), ..e.clone() })
            .collect();
        let unread = if poll {
            self.read = self.next;
            None
        } else {
            events.retain(|e| !is_message(e));
            let mut u = self.unread();
            u.next().map(|first| Unread { count: 1 + u.count(), first: first.t })
        };
        let since = self.cursors.get(key).or_else(|| self.cursors.get(ANON)).and_then(|c| c.last);
        self.uses += 1;
        self.cursors.insert(key.to_string(), Cursor { seq: self.next, last: Some(now_ms()), used: self.uses });
        self.evict();
        self.open = false;
        Delivery { since, events, unread }
    }

    /// Events with seq >= `from`, oldest first (cheap: binary search).
    pub fn from_seq(&self, from: u64) -> impl DoubleEndedIterator<Item = &Event> {
        let i = self.events.partition_point(|e| e.seq < from);
        self.events.range(i..)
    }

    /// `guilab log`: events after seq `after` and/or since time `since`, of the given
    /// kinds (all if empty), at most the `last` newest (0 = all). Pointer motion doesn't
    /// count toward `last` (a human moving the mouse would push everything else out).
    pub fn query(&self, after: Option<u64>, since: Option<u64>, kinds: &[&str], last: usize) -> LogInfo {
        let mut v: Vec<Event> = self
            .events
            .iter()
            .filter(|e| after.is_none_or(|a| e.seq > a) && since.is_none_or(|s| e.t >= s))
            .filter(|e| kinds.is_empty() || kinds.contains(&e.data.kind()))
            .cloned()
            .collect();
        let mut omitted = 0;
        if last > 0 {
            let mut n = 0;
            for (i, e) in v.iter().enumerate().rev() {
                if !matches!(e.data, EventData::Motion { .. }) {
                    n += 1;
                    if n > last {
                        omitted = i + 1;
                        break;
                    }
                }
            }
        }
        v.drain(..omitted);
        LogInfo { events: v, omitted, dropped: self.dropped }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{ResolvedTiers, Tiers};

    fn motion(x: i32) -> EventData {
        EventData::Motion { path: vec![[x, 0, 0]] }
    }
    fn msg() -> EventData {
        EventData::Message { text: "hi".into(), images: vec![], screen: None }
    }

    #[test]
    fn coalescing() {
        let mut l = EventLog::default();
        let a = l.push_at(1000, motion(1), None);
        assert_eq!(l.push_at(1100, motion(2), None), a);
        assert_eq!(l.push_at(1500, motion(3), None), a);
        // A gap starts a new run, as does any other event.
        let b = l.push_at(2100, motion(4), None);
        assert_ne!(a, b);
        l.push_at(2200, EventData::Unlock, None);
        let c = l.push_at(2300, motion(5), None);
        assert_eq!(c, b + 2);
        let ev = l.query(None, None, &[], 0).events;
        assert_eq!(ev[0].data, EventData::Motion { path: vec![[1, 0, 0], [2, 0, 100], [3, 0, 500]] });
        // Scrolls add up.
        let s = |dy| EventData::Scroll { dx: 0.0, dy, x: 1, y: 2, smooth: false };
        let d = l.push_at(3000, s(1.0), None);
        assert_eq!(l.push_at(3100, s(2.0), None), d);
        assert!(matches!(l.query(Some(c), None, &[], 0).events[0].data, EventData::Scroll { dy: 3.0, .. }));
    }

    #[test]
    fn delivery() {
        let mut l = EventLog::default();
        let t = ResolvedTiers::default();
        l.push(EventData::Command { cmd: "tree".into() }, Some(ANON));
        l.push(EventData::AppStart { id: 1, argv: vec!["a".into()] }, Some(ANON));
        l.push(EventData::Title { id: 3, title: None }, None);
        assert!(!l.has_wake(ANON, &t));
        // Own commands and their effects aren't echoed; log-tier events never delivered.
        let d = l.deliver(ANON, &t, None, false);
        assert_eq!(d.events.len(), 1);
        assert_eq!(d.events[0].tier, Some(Tier::Passive));
        assert!(l.deliver(ANON, &t, None, false).is_empty());
        // Another key sees the app start, and starts at the anon cursor.
        l.push(EventData::AppStart { id: 2, argv: vec!["b".into()] }, Some(ANON));
        assert_eq!(l.deliver("/wt", &t, None, false).events.len(), 1);
        // Overrides.
        let wake_window = Tiers { wake: vec!["window".into()], ..Default::default() }.resolve().unwrap();
        l.push(EventData::Title { id: 3, title: None }, None);
        assert!(l.has_wake("/wt", &wake_window) && !l.has_wake("/wt", &t));
    }

    #[test]
    fn messages_stay_unread_until_polled() {
        let mut l = EventLog::default();
        let t = ResolvedTiers::default();
        let w = l.push(msg(), None);
        l.push(EventData::Title { id: 3, title: None }, None);
        // A command reply only counts it; the passive event is consumed.
        let d = l.deliver(ANON, &t, None, false);
        assert_eq!(d.events.len(), 1);
        assert_eq!(d.unread, Some(Unread { count: 1, first: d.unread.unwrap().first }));
        assert!(l.deliver(ANON, &t, None, false).unread.is_some());
        // Any key's poll returns it at once, with nothing already delivered, and reads it.
        assert!(l.has_wake("/wt", &t) && l.has_wake(ANON, &t));
        let d = l.deliver(ANON, &t, None, true);
        assert_eq!(d.events.len(), 1);
        assert_eq!((d.events[0].seq, d.events[0].tier), (w, Some(Tier::Wake)));
        assert!(!l.has_wake(ANON, &t) && !l.has_wake("/wt", &t));
        assert!(l.deliver("/wt", &t, None, false).unread.is_none());
        // Two pollers waiting on one key both get the next one.
        let from = l.end();
        let w = l.push(msg(), None);
        assert_eq!(l.first_wake(ANON, &t, from), Some(w));
        assert_eq!(l.deliver(ANON, &t, Some(w), true).events.len(), 1);
        assert_eq!(l.first_wake(ANON, &t, from), Some(w));
        assert_eq!(l.deliver(ANON, &t, Some(w), true).events.len(), 1);
        // --passive message: no wake, but a poll still shows (and reads) it.
        l.push(msg(), None);
        let p = Tiers { passive: vec!["message".into()], ..Default::default() }.resolve().unwrap();
        assert!(!l.has_wake(ANON, &p));
        assert_eq!(l.deliver(ANON, &p, None, true).events[0].tier, Some(Tier::Passive));
        assert!(l.unread().next().is_none());
    }

    #[test]
    fn query() {
        let mut l = EventLog::default();
        for i in 0..5 {
            l.push_at(1000 * i, EventData::Unlock, None);
            l.push_at(1000 * i, EventData::Revoke, None);
        }
        assert_eq!(l.query(None, None, &["revoke"], 0).events.len(), 5);
        let r = l.query(None, Some(3000), &[], 3);
        assert_eq!((r.events.len(), r.omitted), (3, 1));
        assert_eq!(l.query(Some(7), None, &[], 0).events.len(), 2);
        // Motion doesn't count toward the limit.
        l.push_at(9000, motion(1), None);
        let r = l.query(None, None, &[], 2);
        assert_eq!((r.events.len(), r.omitted), (3, 8));

    }
}
