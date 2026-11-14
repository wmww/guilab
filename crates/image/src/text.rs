//! Fonts and text layout. Core decides line breaks and glyph positions; backends draw the
//! given lines without wrapping, so text looks the same everywhere.

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
}

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
        Fonts { ascent: m.ascent, line: m.ascent - m.descent + m.leading, faces }
    }

    pub fn face(&self, i: u8) -> &FontRef<'static> {
        &self.faces[i as usize]
    }

    /// The face and glyph for a char; unknown chars use the primary font's .notdef.
    fn glyph(&self, c: char) -> (u8, GlyphId) {
        for (i, f) in self.faces.iter().enumerate() {
            if let Some(g) = f.charmap().map(c)
                && g != GlyphId::NOTDEF
            {
                return (i as u8, g);
            }
        }
        (0, GlyphId::NOTDEF)
    }

    fn advance(&self, face: u8, g: GlyphId, size: f32) -> f32 {
        self.faces[face as usize].glyph_metrics(FontSize::new(size), LocationRef::default()).advance_width(g).unwrap_or(0.0)
    }

    /// Shapes one line (no wrapping). Control chars draw nothing.
    fn shape(&self, text: &str, size: f32) -> (Vec<Glyph>, f32) {
        let mut x = 0.0;
        let mut out = Vec::new();
        for c in text.chars() {
            if c.is_control() {
                continue;
            }
            let (face, g) = self.glyph(c);
            out.push(Glyph { face, id: g.to_u32(), x });
            x += self.advance(face, g, size);
        }
        (out, x)
    }

    /// Lays out `text` at `size` px. `\n` always breaks; `width` wraps greedily at spaces
    /// (and inside words longer than the width).
    pub fn layout(&self, text: &str, size: f32, width: Option<f32>) -> TextLayout {
        let line_h = self.line * size;
        let mut lines: Vec<String> = Vec::new();
        for para in text.split('\n') {
            match width {
                None => lines.push(para.to_string()),
                Some(w) => self.wrap(para, size, w, &mut lines),
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

    fn width(&self, s: &str, size: f32) -> f32 {
        self.shape(s, size).1
    }

    fn wrap(&self, para: &str, size: f32, max: f32, out: &mut Vec<String>) {
        let mut cur = String::new();
        // Words keep their leading spaces; a break drops the spaces at the break.
        for word in split_keep_spaces(para) {
            let candidate = format!("{cur}{word}");
            if cur.is_empty() || self.width(candidate.trim_end(), size) <= max {
                cur = candidate;
            } else {
                out.push(cur.trim_end().to_string());
                cur = word.trim_start().to_string();
            }
            // A single word wider than the line breaks between chars.
            while self.width(cur.trim_end(), size) > max && cur.chars().count() > 1 {
                let mut head = String::new();
                for c in cur.chars() {
                    let mut h2 = head.clone();
                    h2.push(c);
                    if !head.is_empty() && self.width(&h2, size) > max {
                        break;
                    }
                    head = h2;
                }
                if head.len() == cur.len() {
                    break;
                }
                cur = cur[head.len()..].to_string();
                out.push(head);
            }
        }
        out.push(cur.trim_end().to_string());
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

fn split_keep_spaces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_word = false;
    for (i, c) in s.char_indices() {
        if c == ' ' && in_word {
            out.push(&s[start..i]);
            start = i;
            in_word = false;
        } else if c != ' ' {
            in_word = true;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
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
