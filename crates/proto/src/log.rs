//! The per-session event log store: append-only, with motion/scroll coalescing and a
//! delivery cursor per client key.
//!
//! Delivery keys: a client's lock token, or [`ANON`] for clients without one, so every
//! token-less client of a session shares one cursor. A delivery hands a client every
//! non-log event after its cursor (minus events its own commands caused) and moves the
//! cursor to the end. Several pollers on one key: each wake event reaches all of them
//! (see `deliver`'s `from`); passive events go to whichever delivery comes first.

use std::collections::{HashMap, VecDeque};

use crate::events::{Delivery, Event, EventData, LogInfo, Tier, Tiers, now_ms};

/// Delivery key of clients without a lock token.
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

    /// Starts a key's cursor at the end (a new lock token: it has seen everything so far).
    pub fn start_key(&mut self, key: &str) {
        self.uses += 1;
        self.cursors.insert(key.to_string(), Cursor { seq: self.next, last: Some(now_ms()), used: self.uses });
        self.evict();
    }

    fn evict(&mut self) {
        while self.cursors.len() > MAX_KEYS {
            let Some(k) = self.cursors.iter().filter(|(k, _)| k.as_str() != ANON).min_by_key(|(_, c)| c.used).map(|(k, _)| k.clone()) else {
                break;
            };
            self.cursors.remove(&k);
        }
    }

    fn deliverable<'a>(&'a self, key: &'a str, tiers: &'a Tiers, from: u64) -> impl Iterator<Item = (&'a Event, Tier)> + 'a {
        let first = self.next - self.events.len() as u64;
        self.events
            .iter()
            .skip(from.saturating_sub(first) as usize)
            .filter(move |e| e.by.as_deref() != Some(key))
            .map(move |e| (e, tiers.tier(&e.data)))
            .filter(|(_, t)| *t != Tier::Log)
    }

    /// First wake event for `key` at or after `from`.
    pub fn first_wake(&self, key: &str, tiers: &Tiers, from: u64) -> Option<u64> {
        self.deliverable(key, tiers, from).find(|(_, t)| *t == Tier::Wake).map(|(e, _)| e.seq)
    }

    /// Whether `key` has an undelivered wake event.
    pub fn has_wake(&self, key: &str, tiers: &Tiers) -> bool {
        self.first_wake(key, tiers, self.cursor(key)).is_some()
    }

    /// Everything `key` hasn't seen (from its cursor, or from `from` if earlier: a wake
    /// event another poller of the same key already got), and moves its cursor to the end.
    pub fn deliver(&mut self, key: &str, tiers: &Tiers, from: Option<u64>) -> Delivery {
        let c = self.cursor(key);
        let start = from.map_or(c, |f| f.min(c));
        let events = self
            .deliverable(key, tiers, start)
            .map(|(e, t)| Event { tier: Some(t), ..e.clone() })
            .collect();
        let since = self.cursors.get(key).or_else(|| self.cursors.get(ANON)).and_then(|c| c.last);
        self.uses += 1;
        self.cursors.insert(key.to_string(), Cursor { seq: self.next, last: Some(now_ms()), used: self.uses });
        self.evict();
        self.open = false;
        Delivery { since, events }
    }

    /// Events with seq >= `from`, oldest first (cheap: binary search).
    pub fn from_seq(&self, from: u64) -> impl DoubleEndedIterator<Item = &Event> {
        let i = self.events.partition_point(|e| e.seq < from);
        self.events.range(i..)
    }

    /// `guilab log`: events after seq `after` and/or since time `since`, of the given
    /// kinds (all if empty), at most the `last` newest (0 = all).
    pub fn query(&self, after: Option<u64>, since: Option<u64>, kinds: &[&str], last: usize) -> LogInfo {
        let mut v: Vec<Event> = self
            .events
            .iter()
            .filter(|e| after.is_none_or(|a| e.seq > a) && since.is_none_or(|s| e.t >= s))
            .filter(|e| kinds.is_empty() || kinds.contains(&e.data.kind()))
            .cloned()
            .collect();
        let omitted = if last > 0 && v.len() > last { v.len() - last } else { 0 };
        v.drain(..omitted);
        LogInfo { events: v, omitted, dropped: self.dropped }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Tiers;

    fn motion(x: i32) -> EventData {
        EventData::Motion { path: vec![[x, 0, 0]] }
    }
    fn msg() -> EventData {
        EventData::Message { text: "hi".into(), images: vec![], annotations: vec![] }
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
        let t = Tiers::default();
        l.push(EventData::Command { cmd: "tree".into() }, Some(ANON));
        l.push(EventData::AppStart { id: 1, argv: vec!["a".into()] }, Some(ANON));
        l.push(EventData::Title { id: 3, title: None }, None);
        assert!(!l.has_wake(ANON, &t));
        // Own commands and their effects aren't echoed; log-tier events never delivered.
        let d = l.deliver(ANON, &t, None);
        assert_eq!(d.events.len(), 1);
        assert_eq!(d.events[0].tier, Some(Tier::Passive));
        assert!(l.deliver(ANON, &t, None).events.is_empty());
        // Another key sees the app start, and starts at the anon cursor.
        l.push(EventData::AppStart { id: 2, argv: vec!["b".into()] }, Some(ANON));
        assert_eq!(l.deliver("tok", &t, None).events.len(), 1);
        let w = l.push(msg(), None);
        assert!(l.has_wake("tok", &t) && l.has_wake(ANON, &t));
        // Both pollers of one key get the wake event.
        assert_eq!(l.first_wake("tok", &t, w), Some(w));
        assert_eq!(l.deliver("tok", &t, Some(w)).events.len(), 1);
        assert!(!l.has_wake("tok", &t));
        assert_eq!(l.deliver("tok", &t, Some(w)).events.len(), 1);
        // A fresh lock token starts at the end.
        l.start_key("new");
        assert!(!l.has_wake("new", &t));
        // Overrides.
        let wake_input = Tiers { wake: vec!["window".into()], ..Default::default() };
        l.push(EventData::Title { id: 3, title: None }, None);
        assert!(l.has_wake("new", &wake_input) && !l.has_wake("new", &t));
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
    }
}
