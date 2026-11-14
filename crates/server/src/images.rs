//! The server's image store: captures, renders to files, `image make`, message images.
//! See notes/image-system-design.md.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use guilab_compositor::Session;
use guilab_image::{
    Asset, AssetId, Body, Element, Fonts, Group, IRect, Image, ImageId, Kind, LoadError, Made, MakeReq, Pt, Raster, Rect, RenderOpts, Store,
    format::{self, ListOpts},
    load_file,
};
use guilab_proto::{ImageRef, RenderInfo};

/// Margin around the marks in a fetch hint, canvas px.
const HINT_MARGIN: f32 = 20.0;

pub(crate) struct Images {
    pub store: Store,
    pub fonts: Arc<Fonts>,
    /// Per session tag: the last capture's frame seq, size and raster, reused while the
    /// frame is unchanged (agents screenshot unchanged screens often).
    last_capture: HashMap<String, (u64, [u32; 2], AssetId)>,
    next_group: u64,
}

impl Images {
    pub fn new() -> Images {
        Images { store: Store::new(Default::default()), fonts: Arc::new(Fonts::new()), last_capture: HashMap::new(), next_group: 0 }
    }

    pub fn new_group(&mut self) -> Group {
        self.next_group += 1;
        Group(self.next_group)
    }

    pub fn drop_session(&mut self, s: &Session, g: Group) {
        self.store.drop_group(g);
        self.last_capture.remove(&s.tag);
    }

    pub fn sweep(&mut self, now: Instant) {
        self.store.sweep(now);
    }

    fn opts(&self, points: bool) -> ListOpts {
        ListOpts { points, utc_offset_s: crate::tz_offset() as i32 }
    }

    /// A screenshot of the session's output as a new image (with `crop`, if any).
    pub fn capture(&mut self, s: &mut Session, g: Group, cursor: bool, by_agent: bool, crop: Option<IRect>) -> Result<ImageId, String> {
        let o = s.output_info();
        let size = [o.width, o.height];
        let reuse = match self.last_capture.get(&s.tag) {
            Some((seq, sz, a)) if !cursor && *seq == s.frame_seq && *sz == size && self.store.get_asset(*a).is_ok() => Some(*a),
            _ => None,
        };
        let at_ms = guilab_proto::events::now_ms();
        let asset = match reuse {
            Some(a) => a,
            None => {
                let (w, h, rgba) = s.capture(cursor).map_err(|e| format!("screenshot: {e:#}"))?;
                // The only place a live frame becomes a raster (see guilab_image::asset).
                let a = self.store.add_asset(Asset::Raster(Raster::from_capture(w, h, rgba, at_ms)), g);
                if !cursor {
                    self.last_capture.insert(s.tag.clone(), (s.frame_seq, [w, h], a));
                }
                a
            }
        };
        let scale = o.scale as f32;
        let canvas = match self.store.get_asset(asset).map_err(|e| e.to_string())?.as_raster() {
            Some(r) => r.size(),
            None => size,
        };
        let mut body = Body::screenshot(asset, canvas, scale);
        body.crop = crop;
        self.store.add_image(None, body, Made::Screenshot { at_ms, by_agent }, g, &[]).map_err(|e| e.to_string())
    }

    pub fn get(&self, id: &str) -> Result<Arc<Image>, String> {
        self.store.lookup(id).map_err(|e| e.to_string())
    }

    pub fn listing(&self, img: &Image, points: bool) -> String {
        format::store_listing(&self.store, img, &self.fonts, &self.opts(points))
    }

    pub fn header(&self, img: &Image) -> String {
        format::header(img, self.store.parent_note(img))
    }

    /// The text snapshot that goes into events.
    pub fn image_ref(&self, img: &Image) -> ImageRef {
        let b = &img.body;
        let mut marks: Option<Rect> = None;
        for (i, e) in b.elements.iter().enumerate() {
            if b.is_base(i) {
                continue;
            }
            let r = e.bounds(b.scale, &self.fonts);
            marks = Some(marks.map_or(r, |m| m.union(&r)));
        }
        let marks = marks.and_then(|m| m.expand(HINT_MARGIN).round_out().intersect(&IRect::of_size(b.canvas))).map(|r| [r.x, r.y, r.w as i32, r.h as i32]);
        ImageRef { id: img.id.to_string(), size: b.canvas, listing: self.listing(img, false), marks }
    }

    /// Renders to `out`, or to `dir/img/<id>[-c..][-m..][-nomarks].png` (reused if there:
    /// images never change).
    pub fn render_file(&self, img: &Image, opts: &RenderOpts, dir: &Path, out: Option<&Path>) -> Result<RenderInfo, String> {
        let r = guilab_image::render(&img.body, &self.store, &self.fonts, opts).map_err(|e| format!("rendering {}: {e}", img.id))?;
        let path = match out {
            Some(p) => p.to_path_buf(),
            None => {
                let mut name = img.id.to_string();
                if r.shows != img.body.shown() {
                    let c = r.shows;
                    name += &format!("-c{}_{}_{}x{}", c.x, c.y, c.w, c.h);
                }
                if let Some([w, h]) = opts.max {
                    name += &format!("-m{w}x{h}");
                }
                if !opts.marks {
                    name += "-nomarks";
                }
                let d = dir.join("img");
                std::fs::create_dir_all(&d).map_err(|e| format!("creating {}: {e}", d.display()))?;
                d.join(format!("{name}.png"))
            }
        };
        let [width, height] = r.size();
        if out.is_some() || !path.exists() {
            write_atomic(&path, &r.png(Some(img.id)))?;
        }
        let c = r.shows;
        Ok(RenderInfo { path, width, height, shows: [c.x, c.y, c.w as i32, c.h as i32], zoom: r.zoom })
    }

    pub fn make(&mut self, req: &MakeReq, g: Group) -> Result<ImageId, String> {
        guilab_image::make(&mut self.store, &self.fonts, req, Made::Agent, g)
    }

    /// An image for a message: an id, or an absolute path of a file that becomes an image
    /// (one raster or SVG element at its natural size).
    pub fn attach(&mut self, spec: &str, made: Made, g: Group) -> Result<Arc<Image>, String> {
        if !spec.starts_with('/') {
            return self.get(spec);
        }
        let path = Path::new(spec);
        let a = load_file(path).map_err(|e| match e {
            LoadError::Rendering(id) if id != "draft" => format!("{spec} is a rendering of image {id}; attach the image itself: --image {id}"),
            e => e.to_string(),
        })?;
        let n = a.natural_size();
        let is_raster = matches!(a, Asset::Raster(_));
        let asset = self.store.add_asset(a, g);
        let dst = Rect { min: Pt { x: 0.0, y: 0.0 }, size: n };
        let kind = if is_raster { Kind::Raster { asset, dst, src: None } } else { Kind::Svg { asset, dst } };
        let mut body = Body::new([n.w.ceil().max(1.0) as u32, n.h.ceil().max(1.0) as u32], 1.0);
        body.elements.push(Element::picture(kind));
        let id = self.store.add_image(None, body, made, g, &[]).map_err(|e| e.to_string())?;
        self.store.image(id).map_err(|e| e.to_string())
    }

    /// A human drawing from the current viewer: the frozen screenshot `base` plus marks.
    pub fn drawing(&mut self, base: &str, marks: Vec<Element>, g: Group) -> Result<Arc<Image>, String> {
        let base = self.get(base)?;
        let mut body = (*base.body).clone();
        body.elements.extend(marks);
        let id = self.store.add_image(Some(base.id), body, Made::Human, g, &[]).map_err(|e| e.to_string())?;
        self.store.image(id).map_err(|e| e.to_string())
    }

    /// Images referenced by messages live as long as their session.
    pub fn hold(&mut self, img: &Image) {
        self.store.hold(img.id);
    }

    /// Text that wraps at `width` keeps it; text that doesn't wrap there doesn't need one.
    pub fn text_width(&self, text: &str, size: f32, width: f32) -> Option<f32> {
        let wrapped = self.fonts.layout(text, size, Some(width)).lines.len();
        (wrapped > text.split('\n').count()).then_some(width)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp: PathBuf = path.with_extension("png.tmp");
    std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| format!("writing {}: {e}", path.display()))
}
