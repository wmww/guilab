//! Typing text in place: a new text, or editing an existing one (which hides while edited).
//! Core decides line breaks; the egui text field shows the same breaks.

use egui::{Key, Modifiers, Order, Ui, Vec2};
use guilab_image::model::{FONT, TEXT_PAD, TEXT_RADIUS};
use guilab_image::{Color, Doc, ElemKey, Element, Fonts, IRect, Kind, Pt, pt};

use crate::paint::{self, c32};
use crate::{CanvasOut, Editor, MIN_TEXT_EMS, Tool, WRAP_EMS, Xf};

pub(crate) struct Typing {
    /// The element being edited; `None` for new text.
    pub key: Option<ElemKey>,
    orig: Option<Element>,
    /// Top-left of the text (inside the backing), canvas px.
    at: Pt,
    size: f32,
    /// Wrap width while typing, canvas px.
    wrap: f32,
    /// The text had a stored width, which it keeps.
    fixed_width: bool,
    pub text: String,
    color: Color,
    /// The tool it started with; another tool ends it.
    pub tool: Tool,
    /// Take focus once the pointer is up (the press's release would take it away).
    focus: bool,
    had_focus: bool,
    /// Where the edit was started from (screen), and whether to select the word there
    /// (double-click) rather than put the caret there.
    click: Option<(egui::Pos2, bool)>,
}

/// The default wrap width of text at `x`: [`WRAP_EMS`] or up to the region's right edge.
fn default_wrap(x: f32, size: f32, scale: f32, region: IRect) -> f32 {
    let right = region.right() as f32 - TEXT_PAD * scale;
    (WRAP_EMS * size).min(right - x).max(size)
}

impl Typing {
    /// New text with its first line centered on `q`, moved left if fewer than
    /// [`MIN_TEXT_EMS`] fit before the region's right edge.
    pub fn new(q: Pt, color: Color, scale: f32, region: IRect, tool: Tool, fonts: &Fonts) -> Typing {
        let size = FONT * scale;
        let pad = TEXT_PAD * scale;
        let r = region.to_rect();
        let right = r.max().x - pad;
        let min = (MIN_TEXT_EMS * size).min(right - r.min.x - pad);
        let line_h = fonts.layout("", size, None).line_height;
        let x = q.x.min(right - min).max(r.min.x + pad);
        let y = (q.y - line_h / 2.0).clamp(r.min.y + pad, (r.max().y - pad - line_h).max(r.min.y + pad));
        Typing {
            key: None,
            orig: None,
            at: pt(x, y),
            size,
            wrap: default_wrap(x, size, scale, region),
            fixed_width: false,
            text: String::new(),
            color,
            tool,
            focus: true,
            had_focus: false,
            click: None,
        }
    }

    pub fn edit(d: &Doc, key: ElemKey, tool: Tool, region: IRect, fonts: &Fonts, click: (egui::Pos2, bool)) -> Typing {
        let el = d.element(key).unwrap().clone();
        let Kind::Text { at, size, width, text } = &el.kind else { unreachable!("text_at finds text") };
        let scale = d.body().scale;
        let wrap = width.unwrap_or_else(|| fonts.layout(text, *size, None).size[0].max(default_wrap(at.x, *size, scale, region)));
        Typing {
            key: Some(key),
            at: *at,
            size: *size,
            wrap,
            fixed_width: width.is_some(),
            text: text.clone(),
            color: el.color,
            orig: Some(el),
            tool,
            focus: true,
            had_focus: false,
            click: Some(click),
        }
    }
}

/// The word (run of alphanumerics, or of other non-space chars) around char `i`.
fn word_at(text: &str, i: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let i = i.min(chars.len() - 1);
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let k = class(chars[i]);
    let (mut a, mut b) = (i, i + 1);
    while a > 0 && class(chars[a - 1]) == k {
        a -= 1;
    }
    while b < chars.len() && class(chars[b]) == k {
        b += 1;
    }
    (a, b)
}

/// `text` with the spaces where core wraps turned into newlines (same char count, so egui's
/// cursor positions stay right). Breaks inside words are left to egui.
fn with_breaks(text: &str, fonts: &Fonts, size: f32, wrap: f32) -> String {
    let l = fonts.layout(text, size, Some(wrap));
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    let mut pos = 0;
    for (n, line) in l.lines.iter().enumerate() {
        if n > 0 {
            if chars.get(pos) == Some(&'\n') {
                pos += 1;
            } else if chars.get(pos) == Some(&' ') {
                out[pos] = '\n';
                while chars.get(pos) == Some(&' ') {
                    pos += 1;
                }
            }
        }
        pos += line.text.chars().count();
        // Trailing spaces before a paragraph end were trimmed from the line.
        let mut p2 = pos;
        while chars.get(p2) == Some(&' ') {
            p2 += 1;
        }
        if chars.get(p2).is_none_or(|c| *c == '\n') {
            pos = p2;
        }
    }
    out.into_iter().collect()
}

impl Editor {
    pub(crate) fn finish_typing(&mut self, ty: Typing, doc: Option<&mut Doc>, out: &mut CanvasOut) {
        let text = ty.text.trim_end().to_string();
        let width = if ty.fixed_width || self.fonts.layout(&text, ty.size, None).size[0] > ty.wrap + 0.01 { Some(ty.wrap) } else { None };
        let el = Element::new(ty.color, Kind::Text { at: ty.at, size: ty.size, width, text: text.clone() });
        match (ty.key, doc) {
            (None, _) if text.trim().is_empty() => {
                if self.began && self.pending.is_empty() {
                    self.began = false;
                    out.abandon = true;
                }
            }
            (None, Some(d)) => {
                let e = d.add_edit(el);
                crate::canvas::apply(d, e, out);
                self.began = false;
            }
            (None, None) => self.pending.push(el),
            (Some(k), Some(d)) => {
                let e = if text.trim().is_empty() { d.remove_edit(k).map(Ok) } else if Some(&el) != ty.orig.as_ref() { Some(d.replace_edit(k, el)) } else { None };
                if let Some(e) = e {
                    crate::canvas::apply(d, e, out);
                }
            }
            (Some(_), None) => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn text_ui(&mut self, ui: &mut Ui, doc: Option<&mut Doc>, xf: Xf, region: IRect, scale: f32, fonts_ready: bool, out: &mut CanvasOut) {
        let fonts = self.fonts.clone();
        let id = self.id.with("text");
        let ty = self.typing.as_mut().unwrap();
        let z = xf.zoom;
        let pad = TEXT_PAD * scale;
        // egui drops focus on Escape before widgets run, so look at the key itself.
        let has_focus = ui.memory(|m| m.has_focus(id));
        let enter = has_focus && ui.input_mut(guilab_kit::consume_plain_enter);
        let esc = (has_focus || ty.had_focus) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));

        let color = c32(ty.color);
        let font = paint::font(ty.size * z, fonts_ready);
        let (size, wrap) = (ty.size, ty.wrap);
        let lf = fonts.clone();
        let mut layouter = move |ui: &Ui, buf: &dyn egui::TextBuffer, _w: f32| {
            let s = with_breaks(buf.as_str(), &lf, size, wrap);
            let line_h = lf.layout("", size, None).line_height * z;
            let job = paint::line_job(&s, font.clone(), color, line_h, wrap * z * 1.02 + 2.0);
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let l = fonts.layout(&ty.text, ty.size, Some(ty.wrap));
        let em = ty.size * z;
        let text_w = (l.size[0] * z).max(em * 0.6);
        let backing = c32(guilab_image::draw::text_backing(ty.color));
        let r = egui::Area::new(self.id.with("text-area"))
            .fixed_pos(xf.apply(ty.at) - Vec2::splat(pad * z))
            .order(Order::Foreground)
            .constrain(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::new().fill(backing).inner_margin(pad * z).corner_radius(TEXT_RADIUS * scale * z).show(ui, |ui| {
                    egui::TextEdit::multiline(&mut ty.text)
                            .id(id)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_rows(1)
                            .desired_width(text_w)
                            .min_size(egui::vec2(text_w, 0.0))
                            .layouter(&mut layouter)
                            .show(ui)
                })
            });
        let area = r.response.rect;
        let mut te_out = r.inner.inner;
        let te = te_out.response.response.clone();
        if ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| area.contains(p)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            out.cursor = Some(egui::CursorIcon::Text);
        }
        if let Some((p, word)) = ty.click.take() {
            let c = te_out.galley.cursor_from_pos(p - te_out.galley_pos);
            let range = if word {
                let (a, b) = word_at(&ty.text, c.index.0);
                egui::text::CCursorRange::two(egui::text::CCursor::new(a), egui::text::CCursor::new(b))
            } else {
                egui::text::CCursorRange::one(c)
            };
            te_out.state.cursor.set_char_range(Some(range));
            te_out.state.store(ui.ctx(), id);
        }
        // Growing past the region's bottom pushes the text up.
        let l = fonts.layout(&ty.text, ty.size, Some(ty.wrap));
        let bottom = region.bottom() as f32 - pad;
        if ty.at.y + l.size[1] > bottom {
            ty.at.y = (bottom - l.size[1]).max(region.y as f32 + pad);
        }
        if ty.focus && !ui.input(|i| i.pointer.any_down()) {
            te.request_focus();
            ty.focus = false;
        }
        ty.had_focus |= te.has_focus();
        if esc {
            let ty = self.typing.take().unwrap();
            if ty.key.is_none() && self.began && self.pending.is_empty() {
                self.began = false;
                out.abandon = true;
            }
        } else if enter || (ty.had_focus && te.lost_focus()) {
            let ty = self.typing.take().unwrap();
            self.finish_typing(ty, doc, out);
        }
    }
}
