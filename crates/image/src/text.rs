//! Fonts and text layout. Core decides line breaks and glyph positions; backends draw the
//! given lines without wrapping, so text looks the same everywhere.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use skrifa::instance::{LocationRef, Size as FontSize};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// egui's default proportional fonts (the same bytes the editor's egui uses): Ubuntu-Light,
/// then emoji fallbacks.
pub struct Fonts {
    faces: Vec<FontRef<'static>>,
    /// Ascent and line height per pixel of font size, from the primary font.
    ascent: f32,
    line: f32,
    /// Recent layouts: geometry, hit tests and painting ask for the same texts many times a
    /// frame. A mutex because the server shares `Fonts` between threads.
    cache: Mutex<LayoutCache>,
}

/// Key: text, size bits, width bits.
type LayoutKey = (String, u32, Option<u32>);

#[derive(Default)]
struct LayoutCache {
    map: HashMap<LayoutKey, (Arc<TextLayout>, u64)>,
    tick: u64,
}

/// Layouts kept; the least recently used goes first.
const CACHE_LAYOUTS: usize = 512;

/// One laid-out line. Glyph `x` is relative to the line start; `y` is the line's top.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub y: f32,
    pub w: f32,
    pub glyphs: Vec<Glyph>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// Index into the font list.
    pub face: u8,
    pub id: u32,
    pub x: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    pub lines: Vec<Line>,
    /// Font size in px.
    pub font_size: f32,
    /// Distance from a line's top to its baseline.
    pub ascent: f32,
    pub line_height: f32,
    /// Width (widest line) and height (all lines).
    pub size: [f32; 2],
}

/// The font bytes, in fallback order. Hosts that draw text themselves (egui) register these.
pub const FONT_DATA: [&[u8]; 3] =
    [epaint_default_fonts::UBUNTU_LIGHT, epaint_default_fonts::NOTO_EMOJI_REGULAR, epaint_default_fonts::EMOJI_ICON];

impl Fonts {
    pub fn new() -> Fonts {
        let faces: Vec<FontRef<'static>> = FONT_DATA.iter().map(|d| FontRef::new(d).expect("bundled font")).collect();
        let m = faces[0].metrics(FontSize::new(1.0), LocationRef::default());
        Fonts { ascent: m.ascent, line: m.ascent - m.descent + m.leading, faces, cache: Default::default() }
    }

    pub fn face(&self, i: u8) -> &FontRef<'static> {
        &self.faces[i as usize]
    }

    /// Shapes one line (no wrapping). Control chars draw nothing.
    fn shape(&self, text: &str, size: f32) -> (Vec<Glyph>, f32) {
        let mut sh = Shaper::new(self, size);
        let mut x = 0.0;
        let mut out = Vec::new();
        for c in text.chars() {
            let Some((face, g, adv)) = sh.glyph(c) else { continue };
            out.push(Glyph { face, id: g.to_u32(), x });
            x += adv;
        }
        (out, x)
    }

    /// Lays out `text` at `size` px. `\n` always breaks; `width` wraps greedily at spaces
    /// (and inside words longer than the width). Cached.
    pub fn layout(&self, text: &str, size: f32, width: Option<f32>) -> Arc<TextLayout> {
        let key = (text.to_string(), size.to_bits(), width.map(f32::to_bits));
        {
            let mut c = self.cache.lock().unwrap();
            c.tick += 1;
            let tick = c.tick;
            if let Some(e) = c.map.get_mut(&key) {
                e.1 = tick;
                return e.0.clone();
            }
        }
        let l = Arc::new(self.layout_uncached(text, size, width));
        let mut c = self.cache.lock().unwrap();
        if c.map.len() >= CACHE_LAYOUTS
            && let Some(old) = c.map.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone())
        {
            c.map.remove(&old);
        }
        let tick = c.tick;
        c.map.insert(key, (l.clone(), tick));
        l
    }

    fn layout_uncached(&self, text: &str, size: f32, width: Option<f32>) -> TextLayout {
        let line_h = self.line * size;
        let mut lines: Vec<String> = Vec::new();
        let mut sh = Shaper::new(self, size);
        for para in text.split('\n') {
            match width {
                None => lines.push(para.to_string()),
                Some(w) => wrap(para, &mut sh, w, &mut lines),
            }
        }
        let lines: Vec<Line> = lines
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let (glyphs, w) = self.shape(&t, size);
                Line { text: t, y: i as f32 * line_h, w, glyphs }
            })
            .collect();
        let w = lines.iter().map(|l| l.w).fold(0.0, f32::max);
        let h = lines.len() as f32 * line_h;
        TextLayout { lines, font_size: size, ascent: self.ascent * size, line_height: line_h, size: [w, h] }
    }

    /// Calls `pen` with the outline of every glyph, in canvas px with y down, the text's
    /// top-left at `at`.
    pub fn outline(&self, l: &TextLayout, at: (f32, f32), pen: &mut impl OutlinePen) {
        for line in &l.lines {
            let base = at.1 + line.y + l.ascent;
            for g in &line.glyphs {
                let Some(o) = self.faces[g.face as usize].outline_glyphs().get(GlyphId::new(g.id)) else { continue };
                let mut flip = Flip { pen, dx: at.0 + g.x, dy: base };
                let _ = o.draw(DrawSettings::unhinted(FontSize::new(l.font_size), LocationRef::default()), &mut flip);
            }
        }
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Fonts::new()
    }
}

/// Glyph lookup and advances at one size, with each face's charmap and metrics built once.
struct Shaper<'a> {
    charmaps: Vec<skrifa::charmap::Charmap<'a>>,
    metrics: Vec<skrifa::metrics::GlyphMetrics<'a>>,
}

impl<'a> Shaper<'a> {
    fn new(f: &'a Fonts, size: f32) -> Shaper<'a> {
        Shaper {
            charmaps: f.faces.iter().map(|f| f.charmap()).collect(),
            metrics: f.faces.iter().map(|f| f.glyph_metrics(FontSize::new(size), LocationRef::default())).collect(),
        }
    }

    /// The face, glyph and advance for a char (unknown chars: the primary font's .notdef);
    /// `None` for control chars, which draw nothing.
    fn glyph(&mut self, c: char) -> Option<(u8, GlyphId, f32)> {
        if c.is_control() {
            return None;
        }
        let (face, g) = self.charmaps.iter().enumerate().find_map(|(i, m)| m.map(c).filter(|g| *g != GlyphId::NOTDEF).map(|g| (i as u8, g))).unwrap_or((0, GlyphId::NOTDEF));
        Some((face, g, self.metrics[face as usize].advance_width(g).unwrap_or(0.0)))
    }

    fn advance(&mut self, c: char) -> f32 {
        self.glyph(c).map_or(0.0, |g| g.2)
    }
}

/// Greedy wrap of one paragraph at `max` px: words keep their leading spaces, a break drops
/// the spaces at the break, and a word wider than the line breaks between chars. Widths are
/// summed left to right from each line's start, exactly as [`Fonts::shape`] measures.
fn wrap(para: &str, sh: &mut Shaper, max: f32, out: &mut Vec<String>) {
    let chars: Vec<char> = para.chars().collect();
    let adv: Vec<f32> = chars.iter().map(|c| sh.advance(*c)).collect();
    let width = |a: usize, b: usize| adv[a..b].iter().fold(0.0, |x, w| x + w);
    // End of `[a, b)` with trailing whitespace trimmed.
    let trim_end = |a: usize, mut b: usize| {
        while b > a && chars[b - 1].is_whitespace() {
            b -= 1;
        }
        b
    };
    let text = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    // The current line is chars[a..b].
    let (mut a, mut b) = (0, 0);
    for (ws, we) in word_ranges(&chars) {
        if a == b || width(a, trim_end(a, we)) <= max {
            b = we;
        } else {
            out.push(text(a, trim_end(a, b)));
            a = ws;
            while a < we && chars[a].is_whitespace() {
                a += 1;
            }
            b = we;
        }
        while width(a, trim_end(a, b)) > max && b - a > 1 {
            let mut h = a;
            let mut w = 0.0;
            while h < b {
                let w2 = w + adv[h];
                if h > a && w2 > max {
                    break;
                }
                (w, h) = (w2, h + 1);
            }
            if h == b {
                break;
            }
            out.push(text(a, h));
            a = h;
        }
    }
    out.push(text(a, trim_end(a, b)));
}

/// Char ranges of a paragraph's words, each with its leading spaces (split before every
/// space that follows a non-space).
fn word_ranges(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut start, mut in_word) = (0, false);
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' && in_word {
            out.push((start, i));
            start = i;
            in_word = false;
        } else if c != ' ' {
            in_word = true;
        }
    }
    if start < chars.len() {
        out.push((start, chars.len()));
    }
    out
}

/// Font units are y-up; canvas is y-down.
struct Flip<'a, P: OutlinePen> {
    pen: &'a mut P,
    dx: f32,
    dy: f32,
}

impl<P: OutlinePen> OutlinePen for Flip<'_, P> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pen.move_to(self.dx + x, self.dy - y)
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pen.line_to(self.dx + x, self.dy - y)
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.pen.quad_to(self.dx + cx, self.dy - cy, self.dx + x, self.dy - y)
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        self.pen.curve_to(self.dx + ax, self.dy - ay, self.dx + bx, self.dy - by, self.dx + x, self.dy - y)
    }
    fn close(&mut self) {
        self.pen.close()
    }
}
