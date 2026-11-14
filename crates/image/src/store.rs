//! The image store: images and assets by id, with expiry and a memory budget.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::asset::Asset;
use crate::id::{AssetId, Id, IdError, IdGen, IdKind, ImageId};
use crate::model::{Body, Image, Kind, Made};

/// Read access to assets (the store, or a host's own set).
pub trait Assets {
    fn asset(&self, id: AssetId) -> Option<Arc<Asset>>;
}

impl Assets for HashMap<AssetId, Arc<Asset>> {
    fn asset(&self, id: AssetId) -> Option<Arc<Asset>> {
        self.get(&id).cloned()
    }
}

/// Owner tag (the server uses one per session). Dropping a group drops its images.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Group(pub u64);

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// An unheld image expires after this long without use.
    pub idle: Duration,
    /// Decoded asset bytes above which images are evicted.
    pub budget: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { idle: Duration::from_secs(30 * 60), budget: 1 << 30 }
    }
}

/// Why a lookup failed. Display is a full sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Unknown(String),
    Expired(String),
    Id(IdError),
}

impl fmt::Display for Lookup {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Lookup::Unknown(id) => write!(f, "there's no {id}"),
            Lookup::Expired(id) => write!(f, "{id} has expired (unused images are dropped after a while)"),
            Lookup::Id(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Lookup {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// An element uses an asset the store doesn't have (and that wasn't carried).
    MissingAsset { element: usize, asset: AssetId },
    /// A raster element names an SVG or the other way round.
    WrongAsset { element: usize, asset: AssetId },
    /// A limit or the crop (the message says which).
    Invalid(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            StoreError::MissingAsset { element, asset } => write!(f, "element {element}: {asset} has expired or doesn't exist"),
            StoreError::WrongAsset { element, asset } => {
                write!(f, "element {element}: {asset} is {}", asset.kind().a())
            }
            StoreError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for StoreError {}

struct ImageEntry {
    img: Arc<Image>,
    group: Group,
    holds: u32,
    added: u64,
    /// ms since the store's epoch.
    used: AtomicU64,
}

struct AssetEntry {
    asset: Arc<Asset>,
    group: Group,
    refs: u32,
}

pub struct Store {
    limits: Limits,
    ids: IdGen,
    epoch: Instant,
    images: HashMap<ImageId, ImageEntry>,
    assets: HashMap<AssetId, AssetEntry>,
    /// Every id ever issued, so a stale id is an error and never a different image.
    issued: HashSet<Id>,
    issued_per_kind: HashMap<IdKind, u64>,
    bytes: usize,
    seq: u64,
}

impl Store {
    pub fn new(limits: Limits) -> Store {
        Store::with_ids(limits, IdGen::new())
    }

    pub fn with_ids(limits: Limits, ids: IdGen) -> Store {
        Store {
            limits,
            ids,
            epoch: Instant::now(),
            images: HashMap::new(),
            assets: HashMap::new(),
            issued: HashSet::new(),
            issued_per_kind: HashMap::new(),
            bytes: 0,
            seq: 0,
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Decoded asset bytes held now.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    fn ms(&self, t: Instant) -> u64 {
        t.saturating_duration_since(self.epoch).as_millis() as u64
    }

    /// A fresh id: 4 characters until half of that kind's 4-character space is used, then
    /// longer.
    fn new_id(&mut self, kind: IdKind) -> Id {
        let n = *self.issued_per_kind.get(&kind).unwrap_or(&0);
        let len = (4..7).find(|l| n < IdGen::space(kind, *l) / 2).unwrap_or(7);
        loop {
            let id = self.ids.gen_id(kind, len);
            if self.issued.insert(id) {
                *self.issued_per_kind.entry(kind).or_default() += 1;
                return id;
            }
        }
    }

    /// Adds an asset with no users yet. Unused assets go at the next `sweep`, so add the image
    /// that uses it before then.
    pub fn add_asset(&mut self, a: Asset, group: Group) -> AssetId {
        let kind = if matches!(a, Asset::Raster(_)) { IdKind::Raster } else { IdKind::Svg };
        let id = AssetId(self.new_id(kind));
        self.insert_asset(id, Arc::new(a), group);
        id
    }

    fn insert_asset(&mut self, id: AssetId, a: Arc<Asset>, group: Group) {
        self.bytes += a.bytes();
        self.issued.insert(id.0);
        self.assets.insert(id, AssetEntry { asset: a, group, refs: 0 });
    }

    /// Adds an image. Checks limits, the crop and every asset reference; assets in `carried`
    /// that were evicted (a draft's) are put back under their ids. `parent` may have expired.
    pub fn add_image(
        &mut self,
        parent: Option<ImageId>,
        body: Body,
        made: Made,
        group: Group,
        carried: &[(AssetId, Arc<Asset>)],
    ) -> Result<ImageId, StoreError> {
        body.check().map_err(StoreError::Invalid)?;
        for (i, e) in body.elements.iter().enumerate() {
            let Some(id) = e.kind.asset() else { continue };
            let have = self.assets.get(&id).map(|a| a.asset.clone()).or_else(|| carried.iter().find(|c| c.0 == id).map(|c| c.1.clone()));
            let Some(a) = have else { return Err(StoreError::MissingAsset { element: i + 1, asset: id }) };
            let ok = match e.kind {
                Kind::Raster { .. } => matches!(*a, Asset::Raster(_)) && id.kind() == IdKind::Raster,
                _ => matches!(*a, Asset::Svg(_)) && id.kind() == IdKind::Svg,
            };
            if !ok {
                return Err(StoreError::WrongAsset { element: i + 1, asset: id });
            }
        }
        let used = body.assets();
        for id in &used {
            if !self.assets.contains_key(id) {
                let a = carried.iter().find(|c| c.0 == *id).unwrap().1.clone();
                self.insert_asset(*id, a, group);
            }
            self.assets.get_mut(id).unwrap().refs += 1;
        }
        let id = ImageId(self.new_id(IdKind::Image));
        self.seq += 1;
        let now = self.ms(Instant::now());
        let img = Arc::new(Image { id, parent, body: Arc::new(body), made });
        self.images.insert(id, ImageEntry { img, group, holds: 0, added: self.seq, used: AtomicU64::new(now) });
        Ok(id)
    }

    /// An image by id, marking it used.
    pub fn image(&self, id: ImageId) -> Result<Arc<Image>, Lookup> {
        match self.images.get(&id) {
            Some(e) => {
                e.used.store(self.ms(Instant::now()), Ordering::Relaxed);
                Ok(e.img.clone())
            }
            None if self.issued.contains(&id.0) => Err(Lookup::Expired(id.to_string())),
            None => Err(Lookup::Unknown(id.to_string())),
        }
    }

    /// `image` from a string id, with errors for malformed ids and other kinds.
    pub fn lookup(&self, s: &str) -> Result<Arc<Image>, Lookup> {
        let id = ImageId::parse(s.trim()).map_err(Lookup::Id)?;
        self.image(id)
    }

    /// An image's state without marking it used: `Ok` (present), `Expired` or `Unknown`.
    pub fn status(&self, id: ImageId) -> Result<(), Lookup> {
        if self.images.contains_key(&id) {
            Ok(())
        } else if self.issued.contains(&id.0) {
            Err(Lookup::Expired(id.to_string()))
        } else {
            Err(Lookup::Unknown(id.to_string()))
        }
    }

    /// An asset by id (no use marking: assets live as long as images use them).
    pub fn get_asset(&self, id: AssetId) -> Result<Arc<Asset>, Lookup> {
        match self.assets.get(&id) {
            Some(e) => Ok(e.asset.clone()),
            None if self.issued.contains(&id.0) => Err(Lookup::Expired(id.to_string())),
            None => Err(Lookup::Unknown(id.to_string())),
        }
    }

    /// Keeps an image from expiring (counted). Unknown ids are ignored.
    pub fn hold(&mut self, id: ImageId) {
        if let Some(e) = self.images.get_mut(&id) {
            e.holds += 1;
        }
    }

    pub fn release(&mut self, id: ImageId) {
        if let Some(e) = self.images.get_mut(&id) {
            e.holds = e.holds.saturating_sub(1);
        }
    }

    fn remove_image(&mut self, id: ImageId) -> usize {
        let Some(e) = self.images.remove(&id) else { return 0 };
        let mut freed = 0;
        for a in e.img.body.assets() {
            if let Some(ae) = self.assets.get_mut(&a) {
                ae.refs -= 1;
                if ae.refs == 0 {
                    freed += self.remove_asset(a);
                }
            }
        }
        freed
    }

    fn remove_asset(&mut self, id: AssetId) -> usize {
        let Some(e) = self.assets.remove(&id) else { return 0 };
        let b = e.asset.bytes();
        self.bytes -= b;
        b
    }

    /// Expiry and budget. Unheld images idle for `limits.idle` go; then, while over budget,
    /// unheld images least recently used first, then held ones oldest first. Unused assets go
    /// too. Returns the bytes freed.
    pub fn sweep(&mut self, now: Instant) -> usize {
        let now = self.ms(now);
        let idle = self.limits.idle.as_millis() as u64;
        let mut freed = 0;
        let stale: Vec<ImageId> = self
            .images
            .iter()
            .filter(|(_, e)| e.holds == 0 && now.saturating_sub(e.used.load(Ordering::Relaxed)) >= idle)
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            freed += self.remove_image(id);
        }
        let unused: Vec<AssetId> = self.assets.iter().filter(|(_, e)| e.refs == 0).map(|(id, _)| *id).collect();
        for id in unused {
            freed += self.remove_asset(id);
        }
        if self.bytes > self.limits.budget {
            let mut order: Vec<(bool, u64, ImageId)> = self
                .images
                .iter()
                .map(|(id, e)| {
                    let held = e.holds > 0;
                    (held, if held { e.added } else { e.used.load(Ordering::Relaxed) }, *id)
                })
                .collect();
            order.sort();
            for (_, _, id) in order {
                if self.bytes <= self.limits.budget {
                    break;
                }
                freed += self.remove_image(id);
            }
        }
        freed
    }

    /// Drops every image and asset of a group (session end). Assets that images of other
    /// groups still use stay.
    pub fn drop_group(&mut self, g: Group) {
        let ids: Vec<ImageId> = self.images.iter().filter(|(_, e)| e.group == g).map(|(id, _)| *id).collect();
        for id in ids {
            self.remove_image(id);
        }
        let unused: Vec<AssetId> = self.assets.iter().filter(|(_, e)| e.group == g && e.refs == 0).map(|(id, _)| *id).collect();
        for id in unused {
            self.remove_asset(id);
        }
    }

    /// The note after `parent ID` in a listing: "expired", "your screenshot" or "screenshot".
    pub fn parent_note(&self, img: &Image) -> Option<&'static str> {
        let p = img.parent?;
        match self.images.get(&p) {
            None => Some("expired"),
            Some(e) => match e.img.made {
                Made::Screenshot { by_agent: true, .. } => Some("your screenshot"),
                Made::Screenshot { by_agent: false, .. } => Some("screenshot"),
                _ => None,
            },
        }
    }
}

impl Assets for Store {
    fn asset(&self, id: AssetId) -> Option<Arc<Asset>> {
        self.assets.get(&id).map(|e| e.asset.clone())
    }
}
