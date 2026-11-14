//! Assets (rasters and SVGs) and loading them from files.
//!
//! Invariant: renderings never become rasters. Only `Raster::from_capture` (the server's
//! capture of a live frame, and nothing else) and `load_file` make a `Raster`, and
//! `load_file` refuses PNGs that guilab rendered (see `render::RENDER_MARK`).

use std::fmt;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use resvg::usvg;

use crate::model::{MAX_PIXELS, MAX_SIDE, Size};
use crate::render::RENDER_MARK;

#[derive(Debug)]
pub enum Asset {
    Raster(Raster),
    Svg(Box<SvgDoc>),
}

impl Asset {
    /// Decoded size in memory, for the store's budget.
    pub fn bytes(&self) -> usize {
        match self {
            Asset::Raster(r) => r.px.len(),
            // Rough: the parsed tree is several times the text.
            Asset::Svg(s) => s.text.len() * 8 + 4096,
        }
    }
    pub fn origin(&self) -> &Origin {
        match self {
            Asset::Raster(r) => &r.origin,
            Asset::Svg(s) => &s.origin,
        }
    }
    /// Natural size: raster pixels, or the SVG's width/height (or viewBox).
    pub fn natural_size(&self) -> Size {
        match self {
            Asset::Raster(r) => Size { w: r.size[0] as f32, h: r.size[1] as f32 },
            Asset::Svg(s) => s.size,
        }
    }
    pub fn as_raster(&self) -> Option<&Raster> {
        match self {
            Asset::Raster(r) => Some(r),
            _ => None,
        }
    }
    pub fn as_svg(&self) -> Option<&SvgDoc> {
        match self {
            Asset::Svg(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Capture { at_ms: u64 },
    /// File name (no directory), or "inline" for inline SVG.
    File { name: String },
}

/// Immutable pixels, premultiplied RGBA8.
#[derive(Clone)]
pub struct Raster {
    size: [u32; 2],
    px: Arc<[u8]>,
    origin: Origin,
}

impl fmt::Debug for Raster {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Raster({}x{}, {:?})", self.size[0], self.size[1], self.origin)
    }
}

impl Raster {
    /// From a live session frame (straight RGBA). Call only from the server's capture; any
    /// other pixels must come in through `load_file`, which refuses renderings.
    pub fn from_capture(w: u32, h: u32, rgba: Vec<u8>, at_ms: u64) -> Raster {
        assert_eq!(rgba.len(), w as usize * h as usize * 4, "capture size");
        Raster::from_straight([w, h], rgba, Origin::Capture { at_ms })
    }

    fn from_straight(size: [u32; 2], mut px: Vec<u8>, origin: Origin) -> Raster {
        for p in px.as_chunks_mut::<4>().0 {
            let a = p[3] as u32;
            if a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            }
        }
        Raster { size, px: px.into(), origin }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    /// Premultiplied RGBA8, row-major, no padding.
    pub fn premultiplied(&self) -> &[u8] {
        &self.px
    }
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
}

/// An opaque SVG picture.
pub struct SvgDoc {
    text: Arc<str>,
    size: Size,
    tree: usvg::Tree,
    origin: Origin,
}

impl fmt::Debug for SvgDoc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SvgDoc({}x{}, {:?})", self.size.w, self.size.h, self.origin)
    }
}

/// System fonts for SVG `<text>` (mockups need real fonts), loaded once on first use.
fn system_fonts() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
    .clone()
}

impl SvgDoc {
    /// Parses SVG text. `xmlns` is added when missing. `dir` resolves relative `<image>` hrefs.
    pub fn parse(text: &str, origin: Origin, dir: Option<&Path>) -> Result<SvgDoc, String> {
        let text = add_xmlns(text);
        let mut opt = usvg::Options { resources_dir: dir.map(Path::to_path_buf), font_family: "sans-serif".into(), ..Default::default() };
        if text.contains("<text") {
            opt.fontdb = system_fonts();
        }
        let tree = usvg::Tree::from_str(&text, &opt).map_err(|e| format!("bad SVG: {e}"))?;
        let s = tree.size();
        let size = Size { w: s.width(), h: s.height() };
        if size.w > MAX_SIDE as f32 || size.h > MAX_SIDE as f32 {
            return Err(format!("SVG size {}x{} is too big (at most {MAX_SIDE} px per side)", size.w, size.h));
        }
        Ok(SvgDoc { text: text.into(), size, tree, origin })
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Its width/height, or the viewBox size.
    pub fn size(&self) -> Size {
        self.size
    }
    pub fn tree(&self) -> &usvg::Tree {
        &self.tree
    }
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
}

/// Adds `xmlns="http://www.w3.org/2000/svg"` to the root `<svg` tag if it has none.
fn add_xmlns(text: &str) -> String {
    let Some(start) = text.find("<svg") else { return text.to_string() };
    let end = text[start..].find('>').map(|e| start + e).unwrap_or(text.len());
    if text[start..end].contains("xmlns=") {
        return text.to_string();
    }
    format!("{} xmlns=\"http://www.w3.org/2000/svg\"{}", &text[..start + 4], &text[start + 4..])
}

#[derive(Debug)]
pub enum LoadError {
    Io(String),
    /// The file is a PNG guilab rendered; the value is its mark (the image id, or "draft").
    Rendering(String),
    Format(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LoadError::Io(e) | LoadError::Format(e) => f.write_str(e),
            LoadError::Rendering(id) if id == "draft" => {
                f.write_str("that file is a guilab rendering of an unsent drawing; it can't be loaded back")
            }
            LoadError::Rendering(id) => write!(f, "that file is a rendering of image {id}; use --from {id}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Loads a PNG, JPEG or SVG file. Refuses PNGs guilab rendered.
pub fn load_file(path: &Path) -> Result<Asset, LoadError> {
    let data = std::fs::read(path).map_err(|e| LoadError::Io(format!("can't read {}: {e}", path.display())))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let origin = Origin::File { name };
    let what = |e: String| LoadError::Format(format!("{}: {e}", path.display()));
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        if let Some(id) = render_mark(&data) {
            return Err(LoadError::Rendering(id));
        }
        decode_png(&data, origin).map(Asset::Raster).map_err(what)
    } else if data.starts_with(&[0xff, 0xd8]) {
        decode_jpeg(&data, origin).map(Asset::Raster).map_err(what)
    } else if let Ok(text) = std::str::from_utf8(&data)
        && text.contains("<svg")
    {
        SvgDoc::parse(text, origin, path.parent()).map(|d| Asset::Svg(Box::new(d))).map_err(what)
    } else {
        Err(what("not a PNG, JPEG or SVG file".into()))
    }
}

/// The value of a `tEXt` chunk with keyword `RENDER_MARK`, anywhere in a PNG.
pub fn render_mark(png: &[u8]) -> Option<String> {
    let mut i = 8;
    while i + 8 <= png.len() {
        let len = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
        let ty = &png[i + 4..i + 8];
        let body = png.get(i + 8..(i + 8).checked_add(len)?)?;
        if ty == b"tEXt"
            && let Some(nul) = body.iter().position(|b| *b == 0)
            && &body[..nul] == RENDER_MARK.as_bytes()
        {
            return Some(String::from_utf8_lossy(&body[nul + 1..]).into_owned());
        }
        if ty == b"IEND" {
            break;
        }
        i += 12 + len;
    }
    None
}

fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("{w}x{h} is too big (at most {MAX_SIDE} px per side and {} Mpx)", MAX_PIXELS >> 20));
    }
    Ok(())
}

fn decode_png(data: &[u8], origin: Origin) -> Result<Raster, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let (w, h) = (r.info().width, r.info().height);
    check_size(w, h)?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("PNG too big")?];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let buf = &buf[..info.buffer_size()];
    let n = w as usize * h as usize;
    let mut px = Vec::with_capacity(n * 4);
    use png::ColorType::*;
    match info.color_type {
        Rgba => px.extend_from_slice(buf),
        Rgb => buf.as_chunks::<3>().0.iter().for_each(|c| px.extend_from_slice(&[c[0], c[1], c[2], 255])),
        Grayscale => buf.iter().for_each(|g| px.extend_from_slice(&[*g, *g, *g, 255])),
        GrayscaleAlpha => buf.as_chunks::<2>().0.iter().for_each(|c| px.extend_from_slice(&[c[0], c[0], c[0], c[1]])),
        Indexed => return Err("indexed PNG wasn't expanded".into()),
    }
    if px.len() != n * 4 {
        return Err("PNG data is truncated".into());
    }
    Ok(Raster::from_straight([w, h], px, origin))
}

fn decode_jpeg(data: &[u8], origin: Origin) -> Result<Raster, String> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), opts);
    dec.decode_headers().map_err(|e| format!("bad JPEG: {e:?}"))?;
    let (w, h) = dec.dimensions().ok_or("bad JPEG")?;
    check_size(w as u32, h as u32)?;
    let px = dec.decode().map_err(|e| format!("bad JPEG: {e:?}"))?;
    if px.len() != w * h * 4 {
        return Err("JPEG decoded to an unexpected size".into());
    }
    Ok(Raster::from_straight([w as u32, h as u32], px, origin))
}
