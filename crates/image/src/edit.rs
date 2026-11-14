//! Edit ops on a mutable document (a draft or a scratch copy of an image). Hosts keep the
//! ops in their history and apply/revert them; nothing here owns undo.
//!
//! A [`Doc`] holds every asset its pictures use, so a draft paints and freezes from one
//! place, whatever the store evicted. Only a `Doc` builds [`Edit`]s, and each carries the
//! assets of the elements in it, so replaying edits on another doc keeps that true. Builders
//! refuse elements that would break a [`Body`] limit, so a doc always freezes.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::asset::Asset;
use crate::id::AssetId;
use crate::model::{Body, Element, IRect, Kind, MAX_ELEMENTS, MAX_PEN_POINTS, Rect};
use crate::store::{Assets, StoreError, asset_fits};

/// Stable element identity while editing (indices shift on insert/remove).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElemKey(u64);

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

/// What an edit does (read-only view; edits come from [`Doc`]'s builders).
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Add { key: ElemKey, at: usize, el: Element },
    Remove { key: ElemKey, at: usize, el: Element },
    Replace { key: ElemKey, old: Element, new: Element },
    Crop { old: Option<IRect>, new: Option<IRect> },
}

/// One undoable change, with the assets of the pictures in it.
#[derive(Clone, Debug)]
pub struct Edit {
    op: Op,
    assets: Vec<(AssetId, Arc<Asset>)>,
}

impl Edit {
    pub fn op(&self) -> &Op {
        &self.op
    }
}

impl PartialEq for Edit {
    fn eq(&self, o: &Edit) -> bool {
        self.op == o.op
    }
}

/// Why a builder refused. Display is a sentence for the human.
#[derive(Clone, Debug, PartialEq)]
pub enum EditError {
    /// The element isn't in the doc (any more).
    Gone(ElemKey),
    /// A picture's asset isn't in the doc.
    MissingAsset(AssetId),
    /// A raster element names an SVG or the other way round.
    WrongAsset(AssetId),
    /// The doc would break a [`Body`] limit, or the element is invalid.
    Limit(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EditError::Gone(_) => f.write_str("that element is gone"),
            EditError::MissingAsset(id) => write!(f, "{id} isn't in this image"),
            EditError::WrongAsset(id) => write!(f, "{id} is {}", id.kind().a()),
            EditError::Limit(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for EditError {}

#[derive(Debug)]
pub struct Doc {
    id: DocId,
    body: Body,
    keys: Vec<ElemKey>,
    next: u64,
    rev: u64,
    /// Every asset the doc has used (so undo needs nothing back); at least the body's.
    assets: HashMap<AssetId, Arc<Asset>>,
}

impl Doc {
    /// A doc on `body`, taking every asset its pictures use from `assets`.
    pub fn open(body: Body, assets: &dyn Assets) -> Result<Doc, StoreError> {
        let mut have = HashMap::new();
        for (i, e) in body.elements.iter().enumerate() {
            let Some(id) = e.kind.asset() else { continue };
            let a = assets.asset(id).ok_or(StoreError::MissingAsset { element: i + 1, asset: id })?;
            if !asset_fits(&e.kind, id, &a) {
                return Err(StoreError::WrongAsset { element: i + 1, asset: id });
            }
            have.insert(id, a);
        }
        let keys = (0..body.elements.len() as u64).map(ElemKey).collect();
        Ok(Doc { id: DocId::fresh(), next: body.elements.len() as u64, body, keys, rev: 0, assets: have })
    }

    pub fn id(&self) -> DocId {
        self.id
    }

    pub fn body(&self) -> &Body {
        &self.body
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

    /// The assets of `els` (all in the doc, or `extra`).
    fn carry(&self, els: &[&Element], extra: Option<(AssetId, &Arc<Asset>)>) -> Vec<(AssetId, Arc<Asset>)> {
        let mut v: Vec<(AssetId, Arc<Asset>)> = Vec::new();
        for id in els.iter().filter_map(|e| e.kind.asset()) {
            if v.iter().any(|(i, _)| *i == id) {
                continue;
            }
            let a = self.assets.get(&id).or(extra.filter(|x| x.0 == id).map(|x| x.1)).expect("checked asset");
            v.push((id, a.clone()));
        }
        v
    }

    /// Checks `el` against the limits, with `old` leaving the doc for it.
    fn check(&self, el: &Element, old: Option<&Element>, extra: Option<(AssetId, &Arc<Asset>)>) -> Result<(), EditError> {
        if let Some(why) = el.problem() {
            return Err(EditError::Limit(format!("the {} {why}", el.kind.name())));
        }
        if old.is_none() && self.body.elements.len() >= MAX_ELEMENTS {
            return Err(EditError::Limit(format!("an image holds at most {MAX_ELEMENTS} elements")));
        }
        let pts: usize = self.body.elements.iter().map(Element::pen_points).sum::<usize>() - old.map_or(0, Element::pen_points);
        if pts + el.pen_points() > MAX_PEN_POINTS {
            return Err(EditError::Limit(format!("an image holds at most {MAX_PEN_POINTS} pen points")));
        }
        if let Some(id) = el.kind.asset() {
            let a = self.assets.get(&id).or(extra.filter(|x| x.0 == id).map(|x| x.1));
            let Some(a) = a else { return Err(EditError::MissingAsset(id)) };
            if !asset_fits(&el.kind, id, a) {
                return Err(EditError::WrongAsset(id));
            }
        }
        Ok(())
    }

    fn new_key(&mut self) -> ElemKey {
        self.next += 1;
        ElemKey(self.next - 1)
    }

    /// The edit that adds `el` on top. A picture's asset must be in the doc already (see
    /// [`Doc::add_picture`]).
    pub fn add_edit(&mut self, el: Element) -> Result<Edit, EditError> {
        self.check(&el, None, None)?;
        let assets = self.carry(&[&el], None);
        Ok(Edit { op: Op::Add { key: self.new_key(), at: self.body.elements.len(), el }, assets })
    }

    /// The edit that adds picture `asset` (kind from the asset) at `dst`, on top.
    pub fn add_picture(&mut self, id: AssetId, asset: Arc<Asset>, dst: Rect) -> Result<Edit, EditError> {
        let kind = match *asset {
            Asset::Raster(_) => Kind::Raster { asset: id, dst, src: None },
            Asset::Svg(_) => Kind::Svg { asset: id, dst },
        };
        let el = Element::picture(kind);
        self.check(&el, None, Some((id, &asset)))?;
        let assets = vec![(id, asset)];
        Ok(Edit { op: Op::Add { key: self.new_key(), at: self.body.elements.len(), el }, assets })
    }

    /// The edit that removes element `key`.
    pub fn remove_edit(&self, key: ElemKey) -> Option<Edit> {
        let at = self.index_of(key)?;
        let el = self.body.elements[at].clone();
        let assets = self.carry(&[&el], None);
        Some(Edit { op: Op::Remove { key, at, el }, assets })
    }

    /// The edit that replaces element `key` with `new`.
    pub fn replace_edit(&self, key: ElemKey, new: Element) -> Result<Edit, EditError> {
        let old = self.element(key).ok_or(EditError::Gone(key))?;
        self.check(&new, Some(old), None)?;
        let assets = self.carry(&[old, &new], None);
        Ok(Edit { op: Op::Replace { key, old: old.clone(), new }, assets })
    }

    pub fn crop_edit(&self, new: Option<IRect>) -> Edit {
        Edit { op: Op::Crop { old: self.body.crop, new }, assets: Vec::new() }
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
        self.take_assets(e);
        match &e.op {
            Op::Add { key, at, el } => self.insert(*key, *at, el),
            Op::Remove { key, .. } => self.remove(*key),
            Op::Replace { key, new, .. } => self.set(*key, new),
            Op::Crop { new, .. } => self.body.crop = *new,
        }
        self.rev += 1;
    }

    pub fn revert(&mut self, e: &Edit) {
        self.take_assets(e);
        match &e.op {
            Op::Add { key, .. } => self.remove(*key),
            Op::Remove { key, at, el } => self.insert(*key, *at, el),
            Op::Replace { key, old, .. } => self.set(*key, old),
            Op::Crop { old, .. } => self.body.crop = *old,
        }
        self.rev += 1;
    }

    fn take_assets(&mut self, e: &Edit) {
        for (id, a) in &e.assets {
            self.assets.entry(*id).or_insert_with(|| a.clone());
        }
    }
}

/// The doc's assets: pass the doc as `carried` to `Store::add_image` when freezing it.
impl Assets for Doc {
    fn asset(&self, id: AssetId) -> Option<Arc<Asset>> {
        self.assets.get(&id).cloned()
    }
}

impl Clone for Doc {
    /// A copy with its own [`DocId`].
    fn clone(&self) -> Doc {
        Doc { id: DocId::fresh(), body: self.body.clone(), keys: self.keys.clone(), next: self.next, rev: self.rev, assets: self.assets.clone() }
    }
}
