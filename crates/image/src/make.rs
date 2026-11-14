//! `image make`: a new image from an optional `--from` image plus element lines.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::asset::{Asset, Origin, SvgDoc, load_file};
use crate::format::{self, INLINE, Mark, Source, Spec};
use crate::id::{AssetId, ImageId};
use crate::model::{Body, Element, IRect, Kind, MAX_SIDE, Made, Rect};
use crate::store::{Group, NoAssets, Store};
use crate::text::Fonts;

/// The request, as the CLI sends it (strings unparsed, so every error comes from here).
#[derive(Clone, Debug, Default)]
pub struct MakeReq {
    pub from: Option<String>,
    pub canvas: Option<[u32; 2]>,
    /// A color or `none`.
    pub color: Option<String>,
    /// `X,Y WxH` or `none`.
    pub crop: Option<String>,
    /// Element numbers of `--from` to remove.
    pub drop: Vec<u32>,
    /// Element lines; one string may hold several lines (stdin).
    pub elements: Vec<String>,
    /// Relative file paths resolve against this.
    pub cwd: PathBuf,
}

/// Errors are full sentences for the agent.
pub fn make(store: &mut Store, fonts: &Fonts, req: &MakeReq, made: Made, group: Group) -> Result<ImageId, String> {
    let from = match &req.from {
        Some(s) => Some(store.lookup(s).map_err(|e| e.to_string())?),
        None => None,
    };
    let mut lines = Vec::new();
    for e in &req.elements {
        lines.extend(format::parse_lines(e).map_err(|e| e.to_string())?);
    }
    let base_len = from.as_ref().map_or(0, |f| f.body.elements.len());
    let from_name = from.as_ref().map(|f| f.id.to_string()).unwrap_or_default();
    let range_err = |n: usize| format!("image {from_name} has {base_len} element{}; there's no element {n}", if base_len == 1 { "" } else { "s" });

    let mut dropped = HashSet::new();
    for &n in &req.drop {
        let n = n as usize;
        if from.is_none() {
            return Err("--drop needs --from (it removes elements of that image)".into());
        }
        if n == 0 || n > base_len {
            return Err(format!("--drop {n}: {}", range_err(n)));
        }
        dropped.insert(n);
    }
    let mut replaced = HashSet::new();
    for l in &lines {
        let Some(n) = l.num else { continue };
        let at = format!("in '{}'", l.src);
        if from.is_none() {
            return Err(format!("{at}: a number at the start replaces that element of --from, and there's no --from"));
        }
        if n > base_len {
            return Err(format!("{at}: {}", range_err(n)));
        }
        if !replaced.insert(n) {
            return Err(format!("{at}: element {n} is replaced twice"));
        }
        if dropped.contains(&n) {
            return Err(format!("{at}: element {n} is both dropped and replaced; do one"));
        }
    }

    let mut body = match &from {
        Some(f) => (*f.body).clone(),
        None => Body::new([1, 1], 1.0),
    };
    let scale = body.scale;
    let mut files = Files { store, group, cwd: &req.cwd, by_path: HashMap::new() };
    let mut slots: Vec<Option<Element>> = body.elements.drain(..).map(Some).collect();
    let mut added = Vec::new();
    for l in &lines {
        let at = format!("in '{}'", l.src);
        let old = l.num.map(|n| slots[n - 1].clone().unwrap());
        let el = resolve(&l.spec, old.as_ref(), scale, fonts, &mut files).map_err(|e| format!("{at}: {e}"))?;
        match l.num {
            Some(n) => slots[n - 1] = Some(el),
            None => added.push(el),
        }
    }
    for n in &dropped {
        slots[n - 1] = None;
    }
    body.elements = slots.into_iter().flatten().chain(added).collect();

    if let Some(c) = req.canvas {
        body.canvas = c;
    } else if from.is_none() {
        let mut r = Rect::default();
        for e in &body.elements {
            r = r.union(&e.bounds(body.scale, fonts));
        }
        if body.elements.is_empty() {
            return Err("nothing to draw: give elements, or --canvas WxH for an empty image".into());
        }
        let m = r.max();
        body.canvas = [(m.x.ceil().max(1.0) as u32).min(MAX_SIDE), (m.y.ceil().max(1.0) as u32).min(MAX_SIDE)];
    }
    if let Some(c) = &req.color {
        body.background = if c.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            Some(format::parse_color(c.trim()).ok_or_else(|| format!("--color \"{c}\" isn't a color; expected red, yellow, green, blue, white, black, #rrggbb or none"))?)
        };
    }
    if let Some(c) = &req.crop {
        body.crop = format::parse_crop(c)?;
    }
    if let Some(c) = body.crop
        && !IRect::of_size(body.canvas).covers(&c)
    {
        let [w, h] = body.canvas;
        return Err(format!("crop {c} reaches outside the {w}x{h} canvas"));
    }
    let parent = from.as_ref().map(|f| f.id);
    store.add_image(parent, body, made, group, &NoAssets).map_err(|e| e.to_string())
}

struct Files<'a> {
    store: &'a mut Store,
    group: Group,
    cwd: &'a Path,
    /// Each file is loaded once per request.
    by_path: HashMap<PathBuf, AssetId>,
}

impl Files<'_> {
    fn path(&mut self, p: &str) -> Result<(AssetId, Arc<Asset>), String> {
        let path = self.cwd.join(p);
        if let Some(id) = self.by_path.get(&path) {
            return Ok((*id, self.store.get_asset(*id).map_err(|e| e.to_string())?));
        }
        let a = load_file(&path).map_err(|e| e.to_string())?;
        let id = self.store.add_asset(a, self.group);
        self.by_path.insert(path, id);
        Ok((id, self.store.get_asset(id).unwrap()))
    }

    fn get(&mut self, src: &Source) -> Result<(AssetId, Arc<Asset>), String> {
        match src {
            Source::Id(id) => Ok((*id, self.store.get_asset(*id).map_err(|e| e.to_string())?)),
            Source::Path(p) => self.path(p),
            Source::Inline(text) => {
                let doc = SvgDoc::parse(text, Origin::File { name: INLINE.into() }, Some(self.cwd))?;
                let id = self.store.add_asset(Asset::Svg(Box::new(doc)), self.group);
                Ok((id, self.store.get_asset(id).unwrap()))
            }
        }
    }
}

fn resolve(spec: &Spec, old: Option<&Element>, scale: f32, fonts: &Fonts, files: &mut Files) -> Result<Element, String> {
    Ok(match spec {
        Spec::Mark { color, mark } => {
            let kind = match mark {
                Mark::Box(r) => Kind::Box(*r),
                Mark::Ellipse(r) => Kind::Ellipse(*r),
                Mark::Line(a, b) => Kind::Line(*a, *b),
                Mark::Arrow(a, b) => Kind::Arrow(*a, *b),
                Mark::Pen(p) => Kind::Pen(p.clone()),
                Mark::PenRect(r) => match old {
                    Some(e @ Element { kind: Kind::Pen(_), .. }) => {
                        return Ok(Element { color: *color, ..e.fit_to(*r, scale, fonts) });
                    }
                    Some(e) => return Err(format!("pen X,Y WxH scales an existing pen, but that element is a {}; give points instead", e.kind.name())),
                    None => {
                        return Err("pen X,Y WxH only works when replacing a pen (like '6 green pen 10,10 50x40'); give points X,Y X,Y ... instead".into());
                    }
                },
                Mark::Text { at, size, width, text } => {
                    Kind::Text { at: *at, size: size.unwrap_or(scale * crate::model::FONT), width: *width, text: text.clone() }
                }
            };
            Element::new(*color, kind)
        }
        Spec::Raster { src, at, size, from } => {
            let (id, a) = files.get(src)?;
            let Asset::Raster(r) = &*a else {
                return Err(format!("{} is an SVG; use svg instead of raster", show_src(src, id)));
            };
            if let Some(f) = from
                && !IRect::of_size(r.size()).covers(f)
            {
                return Err(format!("from {f} reaches outside the {}x{} source", r.size()[0], r.size()[1]));
            }
            let natural = match from {
                Some(f) => crate::model::Size { w: f.w as f32, h: f.h as f32 },
                None => a.natural_size(),
            };
            let s = size.unwrap_or(natural);
            Element::picture(Kind::Raster { asset: id, dst: Rect { min: *at, size: s }, src: *from })
        }
        Spec::Svg { src, at, size } => {
            let (id, a) = files.get(src)?;
            if !matches!(&*a, Asset::Svg(_)) {
                return Err(format!("{} is a raster; use raster instead of svg", show_src(src, id)));
            }
            let s = size.unwrap_or(a.natural_size());
            Element::picture(Kind::Svg { asset: id, dst: Rect { min: *at, size: s } })
        }
    })
}

fn show_src(src: &Source, id: AssetId) -> String {
    match src {
        Source::Path(p) => p.clone(),
        _ => id.to_string(),
    }
}
