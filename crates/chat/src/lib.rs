//! A chat widget: a feed (messages, compact activity lines, image thumbnails, the agent's
//! status line) above a composer (pending attachments inside the text box, above the text).
//!
//! Data in, events out: the host pushes [`Item`]s, passes the pending attachment keys and a
//! [`Thumbs`] source each frame, and gets [`ChatEvent`]s back (open, remove, send, undo).
//! Attachments are opaque keys `K`; the widget knows nothing about images. Slot rects come
//! back every frame ([`ChatOut::slots`]) so a host can animate images to and from them, and
//! [`Thumbs::state`] lets it show a slot empty (its image is elsewhere) or outlined.
//! [`demo::DemoHost`] is a complete host.

mod composer;
pub mod demo;
mod feed;
mod slot;

use std::collections::VecDeque;
use std::hash::Hash;

use egui::{Context, Id, Rect, TextureId, Ui};
use guilab_kit::Theme;

pub use egui;

/// Who wrote an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Author {
    /// The person using this UI.
    Me,
    /// The other side (an agent), by name.
    Them { name: String },
    /// Activity (e.g. a command the agent ran): one compact, dim line.
    Activity,
}

/// One feed entry.
#[derive(Clone, Debug)]
pub struct Item<K> {
    pub author: Author,
    /// Shown as is (e.g. "14:05").
    pub time: String,
    pub text: String,
    /// Images, in a row above the text.
    pub attachments: Vec<K>,
    /// Smaller and dimmed images (e.g. an agent's working screenshots); full on hover.
    pub faint: bool,
}

impl<K> Item<K> {
    pub fn new(author: Author, time: impl Into<String>, text: impl Into<String>) -> Item<K> {
        Item { author, time: time.into(), text: text.into(), attachments: Vec::new(), faint: false }
    }
    pub fn me(time: impl Into<String>, text: impl Into<String>) -> Item<K> {
        Item::new(Author::Me, time, text)
    }
    pub fn them(name: impl Into<String>, time: impl Into<String>, text: impl Into<String>) -> Item<K> {
        Item::new(Author::Them { name: name.into() }, time, text)
    }
    pub fn activity(time: impl Into<String>, text: impl Into<String>) -> Item<K> {
        Item::new(Author::Activity, time, text)
    }
    pub fn with(mut self, attachments: impl IntoIterator<Item = K>) -> Item<K> {
        self.attachments.extend(attachments);
        self
    }
    pub fn faint(mut self) -> Item<K> {
        self.faint = true;
        self
    }
}

/// Where a thumbnail sits. With its key it identifies a slot; composer slots follow their
/// key when others are added or removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SlotId {
    /// Attachment `index` of the feed item `item` (the id [`Chat::push`] returned).
    Feed { item: u64, index: u16 },
    Composer,
}

/// How the host wants a slot shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SlotState {
    #[default]
    Shown,
    /// An empty slot: its image is elsewhere (open in an editor, or flying). `outlined`
    /// marks the slot whose image is open.
    Away { outlined: bool },
}

/// What the host supplies for attachments.
pub trait Thumbs<K> {
    /// Width / height of the key's image. Called for every slot laid out, so keep it cheap.
    fn aspect(&mut self, key: &K) -> f32;
    /// A texture for the key, `height_px` physical pixels tall at most (the hovered size).
    /// `None` shows a placeholder (e.g. still loading, or expired). Only called for slots on
    /// screen. Keep a removed key's texture alive for `Theme.motion.slot_grow`: it shrinks out.
    fn texture(&mut self, ctx: &Context, key: &K, height_px: f32) -> Option<TextureId>;
    fn state(&self, _slot: SlotId, _key: &K) -> SlotState {
        SlotState::Shown
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent<K> {
    /// A thumbnail was clicked (also an empty one). `rect` is where it's drawn now.
    Open { slot: SlotId, key: K, rect: Rect },
    /// The × on a pending attachment. The host removes it from `pending` (and keeps it for undo).
    Remove { key: K },
    /// Enter or the send button, with text or pending attachments. The text box is already
    /// cleared; the host takes `pending` itself.
    Send { text: String },
    /// Undo/redo pressed in the text box while it has nothing of its own to undo (no edits
    /// since it got focus), so it's the host's (e.g. bring back a removed attachment).
    Undo,
    Redo,
}

pub struct ChatOut<K> {
    pub events: Vec<ChatEvent<K>>,
    /// Every slot laid out this frame (feed slots scrolled out of view too) and where its
    /// image is drawn (grown while hovered, scaled while growing in), or would be for an
    /// empty slot. Hosts aim animations at these.
    pub slots: Vec<(SlotId, K, Rect)>,
    /// The feed's visible area: feed slots outside it are scrolled out of view.
    pub feed_clip: Rect,
    /// The composer box.
    pub composer: Rect,
}

impl<K> Default for ChatOut<K> {
    fn default() -> Self {
        ChatOut { events: Vec::new(), slots: Vec::new(), feed_clip: Rect::NOTHING, composer: Rect::NOTHING }
    }
}

impl<K: PartialEq> ChatOut<K> {
    pub fn slot(&self, slot: SlotId, key: &K) -> Option<Rect> {
        self.slots.iter().find(|(s, k, _)| *s == slot && k == key).map(|(_, _, r)| *r)
    }
}

/// What the other side is busy with: animated dots, the text, how long it's been at it,
/// and an optional dim note (e.g. "no activity for 3m"). None while it waits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    /// Seconds since it started.
    pub elapsed: u64,
    pub note: Option<String>,
}

struct Entry<K> {
    id: u64,
    item: Item<K>,
}

pub struct Chat<K: Clone + Eq + Hash> {
    id: Id,
    items: VecDeque<Entry<K>>,
    next_id: u64,
    /// Oldest items are dropped past this.
    pub max_items: usize,
    /// Names and hints.
    pub me_name: String,
    pub hint: String,
    pub empty_hint: String,
    status: Option<Status>,
    draft: String,
    feed: feed::FeedState,
    composer: composer::ComposerState<K>,
    slots: slot::Slots<K>,
}

impl<K: Clone + Eq + Hash> Chat<K> {
    pub fn new(id: impl Into<Id>) -> Chat<K> {
        Chat {
            id: id.into(),
            items: VecDeque::new(),
            next_id: 1,
            max_items: 2000,
            me_name: "You".into(),
            hint: "Message".into(),
            empty_hint: "No messages yet".into(),
            status: None,
            draft: String::new(),
            feed: feed::FeedState::default(),
            composer: composer::ComposerState::default(),
            slots: slot::Slots::default(),
        }
    }

    /// Adds an item at the end and returns its id (for [`SlotId::Feed`]). The feed follows
    /// it if it was scrolled to the end.
    pub fn push(&mut self, item: Item<K>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push_back(Entry { id, item });
        self.feed.pushed();
        while self.items.len() > self.max_items {
            self.items.pop_front();
        }
        id
    }

    pub fn items(&self) -> impl Iterator<Item = (u64, &Item<K>)> {
        self.items.iter().map(|e| (e.id, &e.item))
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Sets the status line (call every frame; cheap when unchanged).
    pub fn set_status(&mut self, status: Option<Status>) {
        if status.is_some() && self.status.is_none() {
            self.feed.pushed();
        }
        self.status = status;
    }

    pub fn draft(&self) -> &str {
        &self.draft
    }

    pub fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }

    /// The text box's id (e.g. to give it focus).
    pub fn text_id(&self) -> Id {
        self.id.with("text")
    }

    /// Cross-fades the next change of this slot's [`SlotState`] instead of switching at once
    /// (e.g. a slot quietly showing its image again, with nothing flying to it).
    pub fn cross_fade(&mut self, slot: SlotId, key: K) {
        self.slots.fades.insert((slot, key));
    }

    /// Scrolls the feed to the end next frame.
    pub fn scroll_to_end(&mut self) {
        self.feed.jump();
    }

    /// Shows the feed and the composer in `ui`'s remaining space. `pending` = the
    /// attachments of the next message, in order.
    pub fn show(&mut self, ui: &mut Ui, pending: &[K], thumbs: &mut dyn Thumbs<K>) -> ChatOut<K> {
        let t = Theme::get(ui.ctx());
        let mut out = ChatOut::default();
        self.slots.begin_frame(ui.ctx());
        let sent = composer::show(self, ui, &t, pending, thumbs, &mut out);
        if sent {
            self.feed.jump();
        }
        feed::show(self, ui, &t, thumbs, &mut out);
        self.slots.end_frame(ui.ctx());
        out
    }
}

#[cfg(test)]
mod tests;
