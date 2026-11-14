//! One session's drawing state: the editor, the chat, the pending drafts, which image is
//! open, and the undo history over all of it (mark edits and composer changes alike).
//!
//! A draft is a mutable [`Doc`] only the viewer holds, frozen into a store image when the
//! message is sent. Store images are never edited: the first edit of an open past image
//! moves its scratch doc into a new draft (copy-on-edit). See notes/image-system-design.md.

use std::sync::Arc;

use guilab_chat::{Chat, ChatEvent, SlotId};
use guilab_editor::{Editor, Tool};
use guilab_image::{Asset, AssetId, Doc, Edit, Fonts, Image, ImageId, Store};
use guilab_kit::History;

use super::{Zoom, thumbs::ThumbCache};
use crate::flight::{Flights, Place};

pub type DraftKey = u64;

/// A chat attachment: a draft in the composer, or a store image (sent, or the agent's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Att {
    Draft(DraftKey),
    Image(ImageId),
}

pub struct Draft {
    pub key: DraftKey,
    /// Holds the assets its pictures use, so it paints and freezes even if the store
    /// evicted them.
    pub doc: Doc,
    /// The image it was made from (a screenshot, or a past image it copies).
    pub parent: Option<ImageId>,
}

/// What the session view shows.
pub enum Open {
    /// The live app.
    Live,
    /// A drawing started on the live app; the screenshot it goes on is on its way (`doc`
    /// arrives with it). Looks like the live app meanwhile. `inserts`: pictures to add
    /// once the doc is there (File > Insert image on the live app).
    Capturing { ticket: u64, doc: Option<(Doc, ImageId)>, inserts: Vec<(AssetId, Arc<Asset>)> },
    Draft(DraftKey),
    /// A store image, on a scratch doc: its first edit makes a draft.
    Past { image: ImageId, slot: SlotId, doc: Doc },
}

#[derive(Clone, Debug)]
enum Op {
    Edit(DraftKey, Edit),
    /// A draft made from a capture went into the composer at `at`.
    Create { key: DraftKey, at: usize },
    /// × on a composer slot.
    Remove { key: DraftKey, at: usize },
    /// The first edit of past image `from` (opened from `slot`) made draft `key`.
    Copy { from: ImageId, slot: SlotId, key: DraftKey },
}

impl Op {
    fn key(&self) -> DraftKey {
        match self {
            Op::Edit(k, _) => *k,
            Op::Create { key, .. } | Op::Remove { key, .. } | Op::Copy { key, .. } => *key,
        }
    }
}

/// What a workspace asks the host (server) for.
pub enum Request {
    /// A screenshot of the live app for a new drawing; answer with [`Workspace::captured`].
    Capture { ticket: u64 },
}

pub struct Workspace {
    pub id: egui::Id,
    pub editor: Editor,
    pub chat: Chat<Att>,
    /// The composer's drafts, in order.
    pub pending: Vec<Draft>,
    /// Drafts out of the composer that undo or redo may bring back.
    pub shelf: Vec<Draft>,
    pub open: Open,
    history: History<Op>,
    /// The open image's own zoom and pan (never locked to the window).
    pub doc_zoom: Zoom,
    pub thumbs: ThumbCache,
    /// The canvas wanted the keyboard last frame.
    pub wants_keys: bool,
    /// The chat's slot rects (this frame's once the chat ran), the feed's visible area and
    /// the composer box: where flights and the glow aim.
    pub slots: Vec<(SlotId, Att, egui::Rect)>,
    pub feed_clip: egui::Rect,
    pub composer: egui::Rect,
    /// Where the open image is drawn in the view (last frame's until the view ran).
    pub view_place: Option<Place>,
    /// Images flying between the view and the chat, and the glow.
    pub flights: Flights,
    next_key: DraftKey,
    next_ticket: u64,
    pub requests: Vec<Request>,
    /// Messages for the human (toasts).
    pub notes: Vec<String>,
}

/// The doc `open` shows (split out so callers can borrow other fields alongside).
pub fn open_doc<'a>(open: &'a mut Open, pending: &'a mut [Draft]) -> Option<&'a mut Doc> {
    match open {
        Open::Live | Open::Capturing { doc: None, .. } => None,
        Open::Capturing { doc: Some((d, _)), .. } | Open::Past { doc: d, .. } => Some(d),
        Open::Draft(k) => pending.iter_mut().find(|d| d.key == *k).map(|d| &mut d.doc),
    }
}

impl Workspace {
    pub fn new(id: egui::Id, fonts: Arc<Fonts>) -> Workspace {
        let mut chat = Chat::new(id.with("chat"));
        chat.hint = "Message the agent".into();
        chat.empty_hint = "Messages, and what the agent does, show up here. Draw on the app to attach a screenshot.".into();
        chat.send_empty_hint = "Type a message or draw something to send".into();
        let mut editor = Editor::new(id.with("editor"), fonts);
        editor.interact_name = "Use the app".into();
        Workspace {
            id,
            editor,
            chat,
            pending: Vec::new(),
            shelf: Vec::new(),
            open: Open::Live,
            history: History::new(),
            doc_zoom: Zoom::default(),
            thumbs: ThumbCache::default(),
            wants_keys: false,
            slots: Vec::new(),
            feed_clip: egui::Rect::NOTHING,
            composer: egui::Rect::NOTHING,
            view_place: None,
            flights: Flights::default(),
            next_key: 1,
            next_ticket: 1,
            requests: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The doc the canvas shows and edits, if any.
    pub fn doc_mut(&mut self) -> Option<&mut Doc> {
        open_doc(&mut self.open, &mut self.pending)
    }

    pub fn doc(&self) -> Option<&Doc> {
        match &self.open {
            Open::Live | Open::Capturing { doc: None, .. } => None,
            Open::Capturing { doc: Some((d, _)), .. } | Open::Past { doc: d, .. } => Some(d),
            Open::Draft(k) => self.pending.iter().find(|d| d.key == *k).map(|d| &d.doc),
        }
    }

    /// The open image's slot and key (drafts sit in the composer).
    pub fn open_key(&self) -> Option<(SlotId, Att)> {
        match &self.open {
            Open::Draft(k) => Some((SlotId::Composer, Att::Draft(*k))),
            Open::Past { image, slot, .. } => Some((*slot, Att::Image(*image))),
            _ => None,
        }
    }

    /// Where a slot is drawn, if the chat laid it out.
    pub fn slot_rect(&self, slot: SlotId, att: Att) -> Option<egui::Rect> {
        self.slots.iter().find(|(s, a, _)| *s == slot && *a == att).map(|(_, _, r)| *r)
    }

    /// The open image flies home to its slot (or, with no slot to fly to, just goes).
    fn fly_home(&mut self) {
        match self.open_key() {
            Some((slot, att)) if self.slot_rect(slot, att).is_some() => self.flights.home(att, slot, self.view_place),
            Some(_) => self.flights.quiet(true),
            None => {}
        }
    }

    /// The (just) open image flies out of its slot into the view.
    fn fly_out(&mut self) {
        match self.open_key() {
            Some((slot, att)) if self.slot_rect(slot, att).is_some() => self.flights.out(att, slot),
            Some(_) => self.flights.quiet(false),
            None => {}
        }
    }

    /// An image (draft or past) is open in the editor, not the live app.
    pub fn image_open(&self) -> bool {
        matches!(self.open, Open::Draft(_) | Open::Past { .. })
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// A drawing press on the live app (`CanvasOut::begin`).
    pub fn begin(&mut self) {
        if !matches!(self.open, Open::Live) {
            return;
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.open = Open::Capturing { ticket, doc: None, inserts: Vec::new() };
        self.requests.push(Request::Capture { ticket });
    }

    /// The drawing that asked for a capture came to nothing (`CanvasOut::abandon`).
    pub fn abandon(&mut self) {
        if matches!(self.open, Open::Capturing { .. }) {
            self.open = Open::Live;
        }
    }

    /// The screenshot for capture `ticket` (or why there's none).
    pub fn captured(&mut self, ticket: u64, img: Result<Arc<Image>, String>, store: &Store, session_zoom: Zoom) {
        let Open::Capturing { ticket: want, doc, inserts } = &mut self.open else { return };
        if *want != ticket || doc.is_some() {
            return;
        }
        let opened = img.and_then(|img| Doc::open((*img.body).clone(), store).map(|d| (d, img.id)).map_err(|e| e.to_string()));
        match opened {
            Ok(d) => *doc = Some(d),
            Err(e) => {
                tracing::warn!("viewer: screenshot for a drawing: {e}");
                self.notes.push("Couldn't take a screenshot to draw on".into());
                self.open = Open::Live;
                self.editor.reset();
                return;
            }
        }
        // Pictures inserted on the live app go in now.
        let inserts = std::mem::take(inserts);
        if !inserts.is_empty() {
            let mut edits = Vec::new();
            for (id, asset) in inserts {
                if let Some(e) = self.insert_into_open(id, asset) {
                    edits.push(e);
                }
            }
            self.record(edits, session_zoom);
        }
    }

    fn insert_into_open(&mut self, id: AssetId, asset: Arc<Asset>) -> Option<Edit> {
        let editor = &mut self.editor;
        let r = open_doc(&mut self.open, &mut self.pending).map(|d| {
            let region = editor.view_region(d.body().canvas, d.body().crop);
            editor.insert(d, id, asset, region.to_rect().center(), region)
        });
        match r {
            Some(Ok(e)) => Some(e),
            Some(Err(e)) => {
                self.notes.push(format!("Couldn't insert the picture: {e}"));
                None
            }
            None => None,
        }
    }

    /// File > Insert image: a picture for the open image, or (on the live app) for a new
    /// drawing on a screenshot.
    pub fn insert(&mut self, id: AssetId, asset: Arc<Asset>, session_zoom: Zoom) {
        match &mut self.open {
            Open::Live => {
                self.begin();
                if let Open::Capturing { inserts, .. } = &mut self.open {
                    inserts.push((id, asset));
                }
                if self.editor.tool == Tool::Interact {
                    self.editor.tool = Tool::Move;
                }
            }
            Open::Capturing { doc: None, inserts, .. } => inserts.push((id, asset)),
            _ => {
                if let Some(e) = self.insert_into_open(id, asset) {
                    self.record(vec![e], session_zoom);
                }
            }
        }
    }

    /// Records edits the canvas already applied to the open doc: one undo step. The first
    /// edit of a capture or a past image makes a draft. `session_zoom`: the live view's,
    /// which a new draft from a capture starts with (so nothing moves).
    pub fn record(&mut self, edits: Vec<Edit>, session_zoom: Zoom) {
        if edits.is_empty() {
            return;
        }
        self.history.begin_group();
        let key = match std::mem::replace(&mut self.open, Open::Live) {
            Open::Draft(k) => {
                self.open = Open::Draft(k);
                k
            }
            Open::Capturing { doc: Some((doc, parent)), .. } => {
                let key = self.new_key();
                let at = self.pending.len();
                self.pending.push(Draft { key, doc, parent: Some(parent) });
                self.history.push(Op::Create { key, at });
                self.doc_zoom = session_zoom;
                self.open = Open::Draft(key);
                // The doc already covers the live app, in the same place: nothing moves.
                self.flights.quiet(false);
                key
            }
            Open::Past { image, slot, doc } => {
                let key = self.new_key();
                self.pending.push(Draft { key, doc, parent: Some(image) });
                self.history.push(Op::Copy { from: image, slot, key });
                // The feed slot quietly shows its image again; the copy grows into the composer.
                self.chat.cross_fade(slot, Att::Image(image));
                self.open = Open::Draft(key);
                key
            }
            other => {
                self.open = other;
                self.history.end_group();
                return;
            }
        };
        for e in edits {
            self.history.push(Op::Edit(key, e));
        }
        self.history.end_group();
    }

    fn new_key(&mut self) -> DraftKey {
        let k = self.next_key;
        self.next_key += 1;
        k
    }

    /// Commits text being typed (and marks waiting for a doc) into the open doc.
    pub fn commit_editor(&mut self, session_zoom: Zoom) {
        let editor = &mut self.editor;
        let out = open_doc(&mut self.open, &mut self.pending).map(|d| editor.commit(d));
        if let Some(out) = out {
            if let Some(r) = out.refused {
                self.notes.push(r);
            }
            self.record(out.edits, session_zoom);
        }
    }

    /// Back to the live app (an open draft stays in the composer).
    pub fn close(&mut self, session_zoom: Zoom) {
        self.commit_editor(session_zoom);
        if self.image_open() {
            self.fly_home();
            self.open = Open::Live;
        }
    }

    /// Opens a chat slot's image: a draft, or a store image on a scratch doc. A click on
    /// the open image's own slot closes it.
    pub fn open_slot(&mut self, slot: SlotId, key: Att, store: &Store, session_zoom: Zoom) {
        let same = match (&self.open, key) {
            (Open::Draft(k), Att::Draft(a)) => *k == a,
            (Open::Past { image, slot: s, .. }, Att::Image(id)) => *image == id && *s == slot,
            _ => false,
        };
        self.close(session_zoom);
        if same {
            if self.editor.tool != Tool::Interact && !self.editor.tool.draws() {
                self.editor.tool = Tool::Interact;
            }
            return;
        }
        if matches!(self.open, Open::Capturing { .. }) {
            self.editor.reset();
        }
        match key {
            Att::Draft(k) if self.pending.iter().any(|d| d.key == k) => self.open = Open::Draft(k),
            Att::Draft(_) => return,
            Att::Image(id) => {
                let doc = store.image(id).map_err(|e| e.to_string()).and_then(|img| Doc::open((*img.body).clone(), store).map_err(|e| e.to_string()));
                match doc {
                    Ok(doc) => self.open = Open::Past { image: id, slot, doc },
                    Err(e) => {
                        self.notes.push(format!("Can't open that image: {e}"));
                        return;
                    }
                }
            }
        }
        self.doc_zoom = Zoom::default();
        self.fly_out();
        // An open image needs a tool that works on it.
        if self.editor.tool == Tool::Interact {
            self.editor.tool = Tool::Move;
        }
    }

    /// The open draft `key` leaves the view without flying anywhere (×, send): it fades out
    /// where it was.
    fn fade_out(&mut self, key: DraftKey) {
        let snap = self.pending.iter().find(|d| d.key == key).and_then(|d| Doc::open(d.doc.body().clone(), &d.doc).ok());
        match snap {
            Some(doc) => self.flights.fade_out(self.view_place, doc),
            None => self.flights.quiet(true),
        }
    }

    /// × on a composer slot.
    pub fn remove(&mut self, key: DraftKey) {
        let Some(at) = self.pending.iter().position(|d| d.key == key) else { return };
        if matches!(self.open, Open::Draft(k) if k == key) {
            self.editor.reset();
            self.open = Open::Live;
            self.fade_out(key);
        }
        let d = self.pending.remove(at);
        self.shelf.push(d);
        self.history.push(Op::Remove { key, at });
    }

    fn take(&mut self, key: DraftKey) -> Option<Draft> {
        if let Some(i) = self.pending.iter().position(|d| d.key == key) {
            return Some(self.pending.remove(i));
        }
        let i = self.shelf.iter().position(|d| d.key == key)?;
        Some(self.shelf.remove(i))
    }

    fn draft_mut(&mut self, key: DraftKey) -> Option<&mut Draft> {
        self.pending.iter_mut().chain(self.shelf.iter_mut()).find(|d| d.key == key)
    }

    fn shelve(&mut self, key: DraftKey) {
        if matches!(self.open, Open::Draft(k) if k == key) {
            self.open = Open::Live;
            self.flights.quiet(true);
        }
        if let Some(d) = self.take(key) {
            self.shelf.push(d);
        }
    }

    fn unshelve(&mut self, key: DraftKey, at: usize) {
        if let Some(d) = self.take(key) {
            let at = at.min(self.pending.len());
            self.pending.insert(at, d);
        }
    }

    /// Opens the draft an undo step changes, so a change never happens out of sight.
    /// (Not for a step that makes or unmakes the draft: that shows in the composer.)
    fn reveal(&mut self, ops: &[Op], session_zoom: Zoom) {
        let Some(key) = ops.iter().find_map(|o| if let Op::Edit(k, _) = o { Some(*k) } else { None }) else { return };
        let makes = ops.iter().any(|o| matches!(o, Op::Create { key: k, .. } | Op::Copy { key: k, .. } if *k == key));
        if makes || matches!(self.open, Open::Draft(k) if k == key) || !self.pending.iter().any(|d| d.key == key) {
            return;
        }
        self.close(session_zoom);
        self.open = Open::Draft(key);
        self.doc_zoom = Zoom::default();
        self.fly_out();
        if self.editor.tool == Tool::Interact {
            self.editor.tool = Tool::Move;
        }
    }

    pub fn undo(&mut self, store: &Store, session_zoom: Zoom) {
        let Some(ops) = self.history.undo() else { return };
        self.reveal(&ops, session_zoom);
        for op in ops {
            match op {
                Op::Edit(k, e) => {
                    if let Some(d) = self.draft_mut(k) {
                        d.doc.revert(&e);
                    }
                }
                Op::Create { key, .. } => self.shelve(key),
                Op::Remove { key, at } => self.unshelve(key, at),
                Op::Copy { from, slot, key } => {
                    let was_open = matches!(self.open, Open::Draft(k) if k == key);
                    self.shelve(key);
                    if was_open && let Ok(doc) = store.image(from).map_err(|e| e.to_string()).and_then(|i| Doc::open((*i.body).clone(), store).map_err(|e| e.to_string())) {
                        self.open = Open::Past { image: from, slot, doc };
                        self.flights.quiet(false);
                    }
                }
            }
        }
    }

    pub fn redo(&mut self, session_zoom: Zoom) {
        let Some(ops) = self.history.redo() else { return };
        self.reveal(&ops, session_zoom);
        for op in ops {
            match op {
                Op::Edit(k, e) => {
                    if let Some(d) = self.draft_mut(k) {
                        d.doc.apply(&e);
                    }
                }
                Op::Create { key, at } => {
                    self.unshelve(key, at);
                    self.open = Open::Draft(key);
                    self.flights.quiet(false);
                }
                Op::Remove { key, .. } => self.shelve(key),
                Op::Copy { from, key, .. } => {
                    // From its past image: the copy just takes over. Else it pops in (its
                    // slot is only now coming back, so there's nothing to fly from).
                    let quiet = !matches!(self.open, Open::Past { image, .. } if image == from);
                    self.unshelve(key, usize::MAX);
                    self.open = Open::Draft(key);
                    if quiet {
                        self.flights.quiet(false);
                    }
                }
            }
        }
        if self.editor.tool == Tool::Interact && self.image_open() {
            self.editor.tool = Tool::Move;
        }
    }

    /// Send: the message takes every pending draft. Returns them (the server freezes them).
    pub fn send(&mut self, session_zoom: Zoom) -> Vec<Draft> {
        self.commit_editor(session_zoom);
        if let Open::Draft(k) = self.open {
            self.fade_out(k);
            self.open = Open::Live;
            self.editor.tool = Tool::Interact;
        }
        let sent = std::mem::take(&mut self.pending);
        let keys: Vec<DraftKey> = sent.iter().map(|d| d.key).collect();
        self.history.retain(|op| !keys.contains(&op.key()));
        let used: Vec<DraftKey> = self.history.ops().map(Op::key).collect();
        self.shelf.retain(|d| used.contains(&d.key));
        sent
    }

    /// The image a message refers to without attaching it: a past image open in the editor.
    pub fn viewing(&self) -> Option<ImageId> {
        match &self.open {
            Open::Past { image, .. } => Some(*image),
            _ => None,
        }
    }

    /// The composer's keys, in order.
    pub fn pending_keys(&self) -> Vec<Att> {
        self.pending.iter().map(|d| Att::Draft(d.key)).collect()
    }

    /// Applies a chat event; returns the text of a send (the caller makes the message).
    pub fn chat_event(&mut self, ev: ChatEvent<Att>, store: &Store, session_zoom: Zoom) -> Option<String> {
        match ev {
            ChatEvent::Open { slot, key, .. } => self.open_slot(slot, key, store, session_zoom),
            ChatEvent::Remove { key: Att::Draft(k) } => self.remove(k),
            ChatEvent::Remove { .. } => {}
            ChatEvent::Send { text } => return Some(text),
            ChatEvent::Started => {}
            ChatEvent::Undo => self.undo(store, session_zoom),
            ChatEvent::Redo => self.redo(session_zoom),
        }
        None
    }
}
