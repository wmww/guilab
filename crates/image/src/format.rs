//! The text format: listings out, element lines in, in the same syntax (see
//! `notes/image-format-doc.md`). Output is the canonical subset of the input grammar, so
//! print → parse → print is identical.

use std::fmt::Write as _;

use crate::asset::Origin;
use crate::id::{AssetId, Id, IdKind};
use crate::model::{Body, Color, Element, IRect, Image, Kind, PALETTE, Pt, Rect, Size, pt};
use crate::store::{Assets, Store};
use crate::text::Fonts;

#[derive(Clone, Copy, Debug, Default)]
pub struct ListOpts {
    /// Print pen points instead of the pen's bounding rect.
    pub points: bool,
    /// Local time offset for screenshot times.
    pub utc_offset_s: i32,
}

/// A number as the format prints it: integer when whole, else one decimal.
pub fn num(x: f32) -> String {
    let r = (x * 10.0).round() / 10.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r:.1}") }
}

fn fpt(p: Pt) -> String {
    format!("{},{}", num(p.x), num(p.y))
}

fn fsize(s: Size) -> String {
    format!("{}x{}", num(s.w), num(s.h))
}

fn frect(r: &Rect) -> String {
    format!("{} {}", fpt(r.min), fsize(r.size))
}

/// `"..."` with `\"`, `\\` and `\n` escaped.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn clock(ms: u64, offset_s: i32) -> String {
    let s = (ms / 1000) as i64 + offset_s as i64;
    let d = s.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", d / 3600, d / 60 % 60, d % 60)
}

/// `image ID WxH[ scale S][, parent ID[ (note)]][, background C][, crop X,Y WxH]`.
pub fn header(img: &Image, parent_note: Option<&str>) -> String {
    let b = &img.body;
    let mut s = format!("image {} {}x{}", img.id, b.canvas[0], b.canvas[1]);
    if b.scale != 1.0 {
        write!(s, " scale {}", num(b.scale)).unwrap();
    }
    if let Some(p) = img.parent {
        write!(s, ", parent {p}").unwrap();
        if let Some(n) = parent_note {
            write!(s, " ({n})").unwrap();
        }
    }
    if let Some(c) = b.background {
        write!(s, ", background {c}").unwrap();
    }
    if let Some(c) = b.crop {
        write!(s, ", crop {c}").unwrap();
    }
    s
}

/// One element line, without its number, with the trailing `(...)` note when there is one.
pub fn element_line(el: &Element, body: &Body, assets: &dyn Assets, fonts: &Fonts, opts: &ListOpts) -> String {
    let mut s = String::new();
    if el.is_mark() {
        write!(s, "{} ", el.color).unwrap();
    }
    s.push_str(el.kind.name());
    let mut note = None;
    match &el.kind {
        Kind::Box(r) | Kind::Ellipse(r) => write!(s, " {}", frect(r)).unwrap(),
        Kind::Line(a, b) | Kind::Arrow(a, b) => write!(s, " {} to {}", fpt(*a), fpt(*b)).unwrap(),
        Kind::Pen(p) => {
            if opts.points {
                for q in p {
                    write!(s, " {}", fpt(*q)).unwrap();
                }
            } else {
                write!(s, " {}", frect(&Rect::bounding(p))).unwrap();
                note = Some(format!("{} point{}", p.len(), if p.len() == 1 { "" } else { "s" }));
            }
        }
        Kind::Text { at, size, width, text } => {
            write!(s, " {}", fpt(*at)).unwrap();
            if num(*size) != num(body.font_size()) {
                write!(s, " size {}", num(*size)).unwrap();
            }
            if let Some(w) = width {
                write!(s, " width {}", num(*w)).unwrap();
            }
            write!(s, " {}", quote(text)).unwrap();
            note = Some(format!("box {}", el.bounds(body.scale, fonts).round_out()));
        }
        Kind::Raster { asset, dst, src } => {
            write!(s, " {asset} {}", frect(dst)).unwrap();
            if let Some(f) = src {
                write!(s, " from {f}").unwrap();
            }
            note = asset_note(*asset, assets, opts);
        }
        Kind::Svg { asset, dst } => {
            write!(s, " {asset} {}", frect(dst)).unwrap();
            note = asset_note(*asset, assets, opts);
        }
    }
    if let Some(n) = note {
        write!(s, " ({n})").unwrap();
    }
    s
}

fn asset_note(id: AssetId, assets: &dyn Assets, opts: &ListOpts) -> Option<String> {
    let Some(a) = assets.asset(id) else { return Some("expired".into()) };
    match a.origin() {
        Origin::Capture { at_ms } => Some(format!("screenshot {}", clock(*at_ms, opts.utc_offset_s))),
        Origin::File { name } if name == INLINE => None,
        Origin::File { name } => Some(format!("from {name}")),
    }
}

/// Origin name of inline SVG.
pub const INLINE: &str = "inline";

/// Header plus numbered element lines.
pub fn listing(img: &Image, parent_note: Option<&str>, assets: &dyn Assets, fonts: &Fonts, opts: &ListOpts) -> String {
    let mut s = header(img, parent_note);
    for (i, e) in img.body.elements.iter().enumerate() {
        write!(s, "\n  {} {}", i + 1, element_line(e, &img.body, assets, fonts, opts)).unwrap();
    }
    s
}

/// `listing` with the parent note and assets from the store.
pub fn store_listing(store: &Store, img: &Image, fonts: &Fonts, opts: &ListOpts) -> String {
    listing(img, store.parent_note(img), store, fonts, opts)
}

// ---------------------------------------------------------------- parsing

/// A parsed element line, before files are loaded and defaults resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The element of `--from` this line replaces.
    pub num: Option<usize>,
    pub spec: Spec,
    /// The line's source text, shortened, for errors.
    pub src: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Spec {
    Mark { color: Color, mark: Mark },
    Raster { src: Source, at: Pt, size: Option<Size>, from: Option<IRect> },
    Svg { src: Source, at: Pt, size: Option<Size> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Box(Rect),
    Ellipse(Rect),
    Line(Pt, Pt),
    Arrow(Pt, Pt),
    Pen(Vec<Pt>),
    /// `pen X,Y WxH`: fit the replaced pen into this rect.
    PenRect(Rect),
    Text { at: Pt, size: Option<f32>, width: Option<f32>, text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Id(AssetId),
    Path(String),
    Inline(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Svg(String),
}

impl Tok {
    fn show(&self) -> String {
        match self {
            Tok::Word(w) => format!("\"{w}\""),
            Tok::Str(s) => quote(s),
            Tok::Svg(_) => "inline SVG".into(),
        }
    }
}

/// Splits text into lines of tokens. Newlines end a line except inside strings and inline
/// SVG. A trailing `(...)` is dropped.
fn tokenize(text: &str) -> Result<Vec<(String, Vec<Tok>)>, ParseError> {
    let cs: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut cur: Vec<Tok> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let flush = |cur: &mut Vec<Tok>, lines: &mut Vec<(String, Vec<Tok>)>, start: usize, end: usize| {
        if !cur.is_empty() {
            let src: String = cs[start..end].iter().collect();
            lines.push((src.trim().to_string(), std::mem::take(cur)));
        }
    };
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            flush(&mut cur, &mut lines, start, i);
            i += 1;
            start = i;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                match cs.get(i) {
                    None => {
                        let src: String = cs[start..].iter().collect();
                        return Err(perr(src.trim(), "the closing \" of the text is missing"));
                    }
                    Some('"') => break,
                    Some('\\') => {
                        match cs.get(i + 1) {
                            Some('n') => s.push('\n'),
                            Some('"') => s.push('"'),
                            Some('\\') => s.push('\\'),
                            Some(o) => {
                                s.push('\\');
                                s.push(*o)
                            }
                            None => s.push('\\'),
                        }
                        i += 1;
                    }
                    Some(o) => s.push(*o),
                }
                i += 1;
            }
            i += 1;
            cur.push(Tok::Str(s));
        } else if starts_with_ci(&cs[i..], "<svg") {
            // Runs to the matching </svg>.
            let mut depth = 0;
            let mut j = i;
            let end = loop {
                if j >= cs.len() {
                    let src: String = cs[start..].iter().collect();
                    return Err(perr(src.trim(), "inline SVG has no matching </svg>"));
                }
                if starts_with_ci(&cs[j..], "<svg") {
                    depth += 1;
                    j += 4;
                } else if starts_with_ci(&cs[j..], "</svg>") {
                    depth -= 1;
                    j += 6;
                    if depth == 0 {
                        break j;
                    }
                } else {
                    j += 1;
                }
            };
            cur.push(Tok::Svg(cs[i..end].iter().collect()));
            i = end;
        } else if c == '(' {
            // Trailing note: must reach the end of the line.
            let mut depth = 0;
            let mut j = i;
            while j < cs.len() && cs[j] != '\n' {
                if cs[j] == '(' {
                    depth += 1;
                } else if cs[j] == ')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            let mut k = j + 1;
            while k < cs.len() && cs[k] != '\n' && cs[k].is_whitespace() {
                k += 1;
            }
            if j >= cs.len() || cs[j] != ')' || (k < cs.len() && cs[k] != '\n') {
                let end = cs[i..].iter().position(|c| *c == '\n').map(|p| i + p).unwrap_or(cs.len());
                let src: String = cs[start..end].iter().collect();
                return Err(perr(src.trim(), "\"(\" starts a note, which must be last on the line and end with \")\""));
            }
            flush(&mut cur, &mut lines, start, i);
            i = k;
            start = i;
        } else {
            let mut w = String::new();
            while i < cs.len() && !cs[i].is_whitespace() {
                w.push(cs[i]);
                i += 1;
            }
            cur.push(Tok::Word(w));
        }
    }
    flush(&mut cur, &mut lines, start, cs.len());
    Ok(lines)
}

fn starts_with_ci(cs: &[char], pat: &str) -> bool {
    pat.chars().enumerate().all(|(i, p)| cs.get(i).is_some_and(|c| c.to_ascii_lowercase() == p))
}

fn short(src: &str) -> String {
    let one: String = src.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 60 { format!("{}…", one.chars().take(59).collect::<String>()) } else { one }
}

fn perr(src: &str, msg: &str) -> ParseError {
    ParseError(format!("in '{}': {msg}", short(src)))
}

/// Parses element lines. `text` may hold several lines (stdin); strings and inline SVG may
/// span lines. Blank lines are skipped.
pub fn parse_lines(text: &str) -> Result<Vec<Line>, ParseError> {
    tokenize(text)?.into_iter().map(|(src, toks)| parse_line(&src, toks)).collect()
}

/// Parses exactly one element line.
pub fn parse_line_str(text: &str) -> Result<Line, ParseError> {
    let mut v = parse_lines(text)?;
    match v.len() {
        1 => Ok(v.pop().unwrap()),
        0 => Err(ParseError("empty element line".into())),
        _ => Err(ParseError(format!("expected one element, got {}", v.len()))),
    }
}

const ELEMENTS: &str = "box, ellipse, line, arrow, pen, text, raster or svg";

fn usage(kw: &str) -> &'static str {
    match kw {
        "box" => "box X,Y WxH",
        "ellipse" => "ellipse X,Y WxH",
        "line" => "line X,Y to X,Y",
        "arrow" => "arrow X,Y to X,Y",
        "pen" => "pen X,Y X,Y ...",
        "text" => "text X,Y [size N] [width N] \"TEXT\"",
        "raster" => "raster ID|FILE X,Y [WxH] [from X,Y WxH]",
        _ => "svg ID|FILE|<svg ...</svg> X,Y [WxH]",
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let l = s.to_ascii_lowercase();
    if let Some((_, c)) = PALETTE.iter().find(|(n, _)| *n == l) {
        return Some(*c);
    }
    let h = l.strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn parse_num(s: &str) -> Option<f32> {
    let ok = !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-');
    let v: f32 = s.parse().ok()?;
    (ok && v.is_finite()).then_some(v)
}

fn parse_pt(s: &str) -> Option<Pt> {
    let (a, b) = s.split_once(',')?;
    Some(pt(parse_num(a)?, parse_num(b)?))
}

fn parse_size(s: &str) -> Option<Size> {
    let (a, b) = s.split_once(['x', 'X', '×'])?;
    let (w, h) = (parse_num(a)?, parse_num(b)?);
    (w >= 0.0 && h >= 0.0).then_some(Size { w, h })
}

/// `X,Y WxH` (two words) or `X,Y,W,H` (one).
fn parse_rect_words(a: &str, b: Option<&str>) -> Result<(Rect, bool), String> {
    let parts: Vec<&str> = a.split(',').collect();
    if parts.len() == 4 {
        let v: Option<Vec<f32>> = parts.iter().map(|p| parse_num(p)).collect();
        if let Some(v) = v
            && v[2] >= 0.0
            && v[3] >= 0.0
        {
            return Ok((Rect::new(v[0], v[1], v[2], v[3]), false));
        }
    }
    let p = parse_pt(a).ok_or_else(|| format!("\"{a}\" isn't a point; expected X,Y like 600,380"))?;
    let Some(b) = b else { return Err(format!("a size must follow \"{a}\"; expected WxH like 100x60")) };
    let s = parse_size(b).ok_or_else(|| format!("\"{b}\" isn't a size; expected WxH like 100x60"))?;
    Ok((Rect { min: p, size: s }, true))
}

/// `X,Y WxH`, `X,Y,W,H` or `none`, in whole pixels.
pub fn parse_crop(s: &str) -> Result<Option<IRect>, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let bad = || format!("crop \"{s}\" isn't a rect; expected 'X,Y WxH' or none");
    let w: Vec<&str> = s.split_whitespace().collect();
    let (r, _) = match w.as_slice() {
        [a] => parse_rect_words(a, None),
        [a, b] => parse_rect_words(a, Some(b)),
        _ => Err(String::new()),
    }
    .map_err(|_| bad())?;
    let r = irect(&r).ok_or_else(|| format!("crop \"{s}\" must be whole pixels"))?;
    if r.w == 0 || r.h == 0 {
        return Err(format!("crop \"{s}\" is empty"));
    }
    Ok(Some(r))
}

fn irect(r: &Rect) -> Option<IRect> {
    let whole = |v: f32| v.fract() == 0.0;
    (whole(r.min.x) && whole(r.min.y) && whole(r.size.w) && whole(r.size.h))
        .then(|| IRect::new(r.min.x as i32, r.min.y as i32, r.size.w as u32, r.size.h as u32))
}

/// Token cursor for one line.
struct Cur<'a> {
    toks: &'a [Tok],
    i: usize,
    src: &'a str,
    kw: &'static str,
}

impl<'a> Cur<'a> {
    fn err(&self, msg: impl AsRef<str>) -> ParseError {
        if self.kw.is_empty() {
            perr(self.src, msg.as_ref())
        } else {
            perr(self.src, &format!("{} ({})", msg.as_ref(), usage(self.kw)))
        }
    }
    fn peek(&self) -> Option<&'a Tok> {
        self.toks.get(self.i)
    }
    fn peek_word(&self) -> Option<&'a str> {
        match self.peek() {
            Some(Tok::Word(w)) => Some(w),
            _ => None,
        }
    }
    fn prev(&self) -> String {
        match self.i.checked_sub(1).and_then(|i| self.toks.get(i)) {
            Some(t) => format!(" after {}", t.show()),
            None => String::new(),
        }
    }
    /// The next token as a word, or an error naming what was expected.
    fn word(&mut self, what: &str) -> Result<&'a str, ParseError> {
        match self.peek() {
            Some(Tok::Word(w)) => {
                self.i += 1;
                Ok(w)
            }
            Some(t) => Err(self.err(format!("{} isn't {what}", t.show()))),
            None => Err(self.err(format!("{what} is missing{}", self.prev()))),
        }
    }
    fn pt(&mut self) -> Result<Pt, ParseError> {
        let w = self.word("a point X,Y")?;
        parse_pt(w).ok_or_else(|| self.err(format!("\"{w}\" isn't a point; expected X,Y like 600,380")))
    }
    fn size(&mut self) -> Result<Size, ParseError> {
        let w = self.word("a size WxH")?;
        parse_size(w).ok_or_else(|| self.err(format!("\"{w}\" isn't a size; expected WxH like 100x60")))
    }
    fn rect(&mut self) -> Result<Rect, ParseError> {
        let a = self.word("a point X,Y")?;
        let b = self.peek_word();
        match parse_rect_words(a, b) {
            Ok((r, two)) => {
                if two {
                    self.i += 1;
                }
                Ok(r)
            }
            Err(e) => Err(self.err(e)),
        }
    }
    fn to(&mut self) -> Result<(), ParseError> {
        match self.peek_word() {
            Some(w) if w.eq_ignore_ascii_case("to") || w == "->" || w == "→" => {
                self.i += 1;
                Ok(())
            }
            Some(w) => Err(self.err(format!("expected \"to\" between the points, not \"{w}\""))),
            None => Err(self.err(format!("\"to X,Y\" is missing{}", self.prev()))),
        }
    }
    fn num_opt(&mut self, kw: &str) -> Result<Option<f32>, ParseError> {
        match self.peek_word() {
            Some(w) if w.eq_ignore_ascii_case(kw) => {
                self.i += 1;
                let v = self.word(&format!("a number after {kw}"))?;
                match parse_num(v) {
                    Some(n) if n > 0.0 => Ok(Some(n)),
                    _ => Err(self.err(format!("\"{v}\" isn't a positive number for {kw}"))),
                }
            }
            _ => Ok(None),
        }
    }
    fn end(&self) -> Result<(), ParseError> {
        match self.peek() {
            None => Ok(()),
            Some(t) => Err(self.err(format!("unexpected {} after the end of the {}", t.show(), self.kw))),
        }
    }
}

fn parse_line(src: &str, toks: Vec<Tok>) -> Result<Line, ParseError> {
    let mut c = Cur { toks: &toks, i: 0, src, kw: "" };
    let mut num = None;
    if let Some(w) = c.peek_word()
        && !w.is_empty()
        && w.chars().all(|ch| ch.is_ascii_digit())
    {
        let n: usize = w.parse().map_err(|_| c.err(format!("\"{w}\" is too big an element number")))?;
        if n == 0 {
            return Err(c.err("element numbers start at 1"));
        }
        num = Some(n);
        c.i += 1;
    }
    let first = c.word(&format!("an element ({ELEMENTS})"))?;
    let mut color = None;
    let kw_word = if let Some(col) = parse_color(first) {
        color = Some(col);
        c.word("a mark after the color (box, ellipse, line, arrow, pen or text)")?
    } else {
        first
    };
    let kw: &'static str = match kw_word.to_ascii_lowercase().as_str() {
        "box" => "box",
        "ellipse" => "ellipse",
        "line" => "line",
        "arrow" => "arrow",
        "pen" => "pen",
        "text" => "text",
        "raster" => "raster",
        "svg" => "svg",
        _ if color.is_some() => {
            return Err(c.err(format!("\"{kw_word}\" isn't a mark; expected box, ellipse, line, arrow, pen or text")));
        }
        _ => {
            return Err(c.err(format!(
                "\"{kw_word}\" isn't a color or an element; expected red, yellow, green, blue, white, black, #rrggbb, or {ELEMENTS}"
            )));
        }
    };
    c.kw = kw;
    if color.is_some() && (kw == "raster" || kw == "svg") {
        return Err(c.err(format!("{kw} takes no color")));
    }
    let color = color.unwrap_or(Color::RED);
    let mark = |m: Mark| Spec::Mark { color, mark: m };
    let spec = match kw {
        "box" => mark(Mark::Box(c.rect()?)),
        "ellipse" => mark(Mark::Ellipse(c.rect()?)),
        "line" | "arrow" => {
            let a = c.pt()?;
            c.to()?;
            let b = c.pt()?;
            mark(if kw == "line" { Mark::Line(a, b) } else { Mark::Arrow(a, b) })
        }
        "pen" => {
            if c.peek_word().is_some_and(|w| w.split(',').count() == 4) && c.toks.len() == c.i + 1 {
                return Ok(Line { num, spec: mark(Mark::PenRect(c.rect()?)), src: short(src) });
            }
            let a = c.pt()?;
            let next = c.peek_word();
            if c.toks.len() == c.i + 1 && next.is_some_and(|w| parse_size(w).is_some()) {
                let s = c.size()?;
                mark(Mark::PenRect(Rect { min: a, size: s }))
            } else {
                let mut pts = vec![a];
                while c.peek().is_some() {
                    pts.push(c.pt()?);
                }
                mark(Mark::Pen(pts))
            }
        }
        "text" => {
            let at = c.pt()?;
            let (mut size, mut width) = (None, None);
            loop {
                if let Some(v) = c.num_opt("size")? {
                    if size.replace(v).is_some() {
                        return Err(c.err("size is given twice"));
                    }
                } else if let Some(v) = c.num_opt("width")? {
                    if width.replace(v).is_some() {
                        return Err(c.err("width is given twice"));
                    }
                } else {
                    break;
                }
            }
            let text = match c.peek() {
                Some(Tok::Str(s)) => {
                    c.i += 1;
                    s.clone()
                }
                Some(t) => {
                    return Err(c.err(format!("expected the text in double quotes, like \"hello\", not {}", t.show())));
                }
                None => return Err(c.err("the text is missing; put it in double quotes, like \"hello\"")),
            };
            mark(Mark::Text { at, size, width, text })
        }
        "raster" => {
            let src = source(&mut c, IdKind::Raster)?;
            let at = c.pt()?;
            let size = match c.peek_word() {
                Some(w) if parse_size(w).is_some() => Some(c.size()?),
                _ => None,
            };
            let from = match c.peek_word() {
                Some(w) if w.eq_ignore_ascii_case("from") => {
                    c.i += 1;
                    let r = c.rect()?;
                    let ir = irect(&r).ok_or_else(|| c.err("\"from\" must be whole source pixels"))?;
                    if ir.w == 0 || ir.h == 0 {
                        return Err(c.err("\"from\" is empty"));
                    }
                    Some(ir)
                }
                _ => None,
            };
            Spec::Raster { src, at, size, from }
        }
        _ => {
            let src = source(&mut c, IdKind::Svg)?;
            let at = c.pt()?;
            let size = match c.peek_word() {
                Some(w) if parse_size(w).is_some() => Some(c.size()?),
                _ => None,
            };
            Spec::Svg { src, at, size }
        }
    };
    c.end()?;
    Ok(Line { num, spec, src: short(src) })
}

/// Whether a word is a file path: contains `/` or ends in `.ext`.
fn is_path(w: &str) -> bool {
    w.contains('/') || w.rsplit_once('.').is_some_and(|(a, e)| !a.is_empty() && !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn source(c: &mut Cur, want: IdKind) -> Result<Source, ParseError> {
    let noun = if want == IdKind::Raster { "raster" } else { "SVG" };
    match c.peek() {
        Some(Tok::Svg(s)) if want == IdKind::Svg => {
            c.i += 1;
            Ok(Source::Inline(s.clone()))
        }
        Some(Tok::Svg(_)) => Err(c.err("inline SVG needs svg, not raster")),
        Some(Tok::Word(w)) => {
            c.i += 1;
            if is_path(w) {
                return Ok(Source::Path(w.clone()));
            }
            match Id::parse(w) {
                Some(id) if id.kind() == want => Ok(Source::Id(AssetId(id))),
                Some(id) if id.kind() == IdKind::Image => Err(c.err(format!(
                    "{w} is an image, not a {noun}; use the {noun} ids from its listing (raster ids start with r, SVG ids with s)"
                ))),
                Some(id) => {
                    let other = if id.kind() == IdKind::Raster { "raster" } else { "svg" };
                    Err(c.err(format!("{w} is {}, not {}; write {other} {w} ...", id.kind().a(), if want == IdKind::Raster { "a raster" } else { "an SVG" })))
                }
                None => Err(c.err(format!("\"{w}\" isn't a {noun} id or a file path (paths need a / or an extension, like ./mock.png)"))),
            }
        }
        Some(t) => Err(c.err(format!("{} isn't a {noun} id or a file path", t.show()))),
        None => Err(c.err(format!("the {noun} id or file is missing"))),
    }
}
