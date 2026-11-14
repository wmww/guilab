//! Edit ops on a mutable document (a draft or a scratch copy of an image). Hosts keep the
//! ops in their history and apply/revert them; nothing here owns undo.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::{Body, Element, IRect};

/// Stable element identity while editing (indices shift on insert/remove).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElemKey(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Add { key: ElemKey, at: usize, el: Element },
    Remove { key: ElemKey, at: usize, el: Element },
    Replace { key: ElemKey, old: Element, new: Element },
    Crop { old: Option<IRect>, new: Option<IRect> },
}

/// Process-unique identity of a [`Doc`]: kept when the doc moves, fresh for a clone. Keys
/// are small integers in every doc, so state about keys (an editor's gesture) must also
/// remember which doc they belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(u64);

impl DocId {
    fn fresh() -> DocId {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        DocId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug)]
pub struct Doc {
    id: DocId,
    body: Body,
    keys: Vec<ElemKey>,
    next: u64,
    rev: u64,
}

impl Doc {
    pub fn from_body(body: Body) -> Doc {
        let keys = (0..body.elements.len() as u64).map(ElemKey).collect();
        Doc { id: DocId::fresh(), next: body.elements.len() as u64, body, keys, rev: 0 }
    }

    pub fn id(&self) -> DocId {
        self.id
    }

    pub fn body(&self) -> &Body {
        &self.body
    }

    pub fn into_body(self) -> Body {
        self.body
    }

    pub fn keys(&self) -> &[ElemKey] {
        &self.keys
    }

    /// Bumped by every apply/revert, for caches.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn index_of(&self, key: ElemKey) -> Option<usize> {
        self.keys.iter().position(|k| *k == key)
    }

    pub fn element(&self, key: ElemKey) -> Option<&Element> {
        self.index_of(key).map(|i| &self.body.elements[i])
    }

    /// A key for a new element.
    pub fn new_key(&mut self) -> ElemKey {
        self.next += 1;
        ElemKey(self.next - 1)
    }

    /// The edit that adds `el` on top.
    pub fn add_edit(&mut self, el: Element) -> Edit {
        Edit::Add { key: self.new_key(), at: self.body.elements.len(), el }
    }

    /// The edit that removes element `key`.
    pub fn remove_edit(&self, key: ElemKey) -> Option<Edit> {
        let at = self.index_of(key)?;
        Some(Edit::Remove { key, at, el: self.body.elements[at].clone() })
    }

    /// The edit that replaces element `key` with `new`.
    pub fn replace_edit(&self, key: ElemKey, new: Element) -> Option<Edit> {
        let old = self.element(key)?.clone();
        Some(Edit::Replace { key, old, new })
    }

    pub fn crop_edit(&self, new: Option<IRect>) -> Edit {
        Edit::Crop { old: self.body.crop, new }
    }

    fn insert(&mut self, key: ElemKey, at: usize, el: &Element) {
        debug_assert!(self.index_of(key).is_none(), "duplicate {key:?}");
        // Edits may come from another doc (a replay on a fresh copy): keys made later must
        // not collide with theirs.
        self.next = self.next.max(key.0 + 1);
        let at = at.min(self.keys.len());
        self.keys.insert(at, key);
        self.body.elements.insert(at, el.clone());
    }

    fn remove(&mut self, key: ElemKey) {
        if let Some(i) = self.index_of(key) {
            self.keys.remove(i);
            self.body.elements.remove(i);
        }
    }

    fn set(&mut self, key: ElemKey, el: &Element) {
        if let Some(i) = self.index_of(key) {
            self.body.elements[i] = el.clone();
        }
    }

    pub fn apply(&mut self, e: &Edit) {
        match e {
            Edit::Add { key, at, el } => self.insert(*key, *at, el),
            Edit::Remove { key, .. } => self.remove(*key),
            Edit::Replace { key, new, .. } => self.set(*key, new),
            Edit::Crop { new, .. } => self.body.crop = *new,
        }
        self.rev += 1;
    }

    pub fn revert(&mut self, e: &Edit) {
        match e {
            Edit::Add { key, .. } => self.remove(*key),
            Edit::Remove { key, at, el } => self.insert(*key, *at, el),
            Edit::Replace { key, old, .. } => self.set(*key, old),
            Edit::Crop { old, .. } => self.body.crop = *old,
        }
        self.rev += 1;
    }
}

impl Clone for Doc {
    /// A copy with its own [`DocId`].
    fn clone(&self) -> Doc {
        Doc { id: DocId::fresh(), body: self.body.clone(), keys: self.keys.clone(), next: self.next, rev: self.rev }
    }
}
