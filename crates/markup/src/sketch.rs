//! Drawing on a frozen screenshot, and the queue of finished drawings.
//!
//! Flow: with a drawing tool selected, pressing on the image starts a drawing. The sketch
//! then asks the host for a screenshot ([`Sketch::wants_screenshot`]) and shows it, frozen,
//! under the marks. "Queue" composites the drawing into an [`Annotated`] image and keeps it
//! as a thumbnail until the next message takes it ([`Sketch::take_all`], which also takes
//! a drawing that's still open).
//!
//! The host owns zoom and pan: each frame it says where the image is on screen ([`View`]).
//! Marks are stored in screenshot pixels, so they stay put under any zoom.

use std::sync::Arc;

use egui::{
    Align2, Color32, ColorImage, CursorIcon, Event, FontId, Id, Key, Modifiers, Order, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, TextureOptions, Ui,
    Vec2, pos2, vec2,
};

use crate::{
    mark::{COLORS, FONT, Kind, Mark, PAD, STROKE, Tool, Xf, text_backing},
    raster::{Renderer, downscale},
};

/// A finished drawing: the composited image and its text annotations.
#[derive(Clone)]
pub struct Annotated {
    pub image: Arc<ColorImage>,
    /// What the human saw: all marks, text included (= `image` when text is baked).
    pub preview: Arc<ColorImage>,
    /// Text annotations, top-left position in image pixels. Whether they're also drawn into
    /// `image` depends on [`Sketch::bake_text`].
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub text: String,
    pub pos: Pos2,
    pub color: Color32,
}

/// Where the screenshot is shown this frame.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// The whole image's rect on screen (may extend past `clip` when zoomed in).
    pub rect: Rect,
    /// The visible area the canvas owns (pointer input outside it isn't ours).
    pub clip: Rect,
    /// Image size in pixels.
    pub size: [usize; 2],
    /// Image pixels per logical pixel (marks are sized in logical pixels).
    pub scale: f32,
}

struct Shot {
    image: Arc<ColorImage>,
    tex: Option<TextureHandle>,
}

struct Editing {
    pos: Pos2,
    text: String,
    color: Color32,
    focus: bool,
}

struct Drawing {
    shot: Option<Shot>,
    scale: f32,
    marks: Vec<Mark>,
    /// The mark being dragged out.
    active: Option<Mark>,
    editing: Option<Editing>,
    /// Undone marks, most recent last; cleared by a new mark.
    redo: Vec<Mark>,
    /// Move tool: the mark being dragged, and the image point it was last dragged to.
    moving: Option<(usize, Pos2)>,
    /// On-screen backing of each text mark as last painted (hit testing), by mark index.
    text_rects: Vec<Option<Rect>>,
}

struct Queued {
    out: Annotated,
    thumb: ColorImage,
    tex: Option<TextureHandle>,
    /// Full-size preview, for hovering.
    full: Option<TextureHandle>,
}

pub struct Sketch {
    id: Id,
    pub tool: Tool,
    pub color: Color32,
    /// Also draw text annotations into the composited image (they're always in `notes`).
    /// Off by default: agents tested with both read the text more reliably than baked-in
    /// text, which also hides the UI under it.
    pub bake_text: bool,
    drawing: Option<Drawing>,
    queue: Vec<Queued>,
    want_shot: bool,
    renderer: Option<Renderer>,
    /// The image transform as of the last [`Self::canvas`].
    last_xf: Option<Xf>,
}

/// Marks smaller than this (screen points) are accidental clicks.
const MIN_MARK: f32 = 3.0;
pub const THUMB: [usize; 2] = [160, 100];

impl Sketch {
    /// `id` must be unique among sketches shown at once.
    pub fn new(id: impl Into<Id>) -> Sketch {
        Sketch {
            id: id.into(),
            tool: Tool::Interact,
            color: COLORS[0].0,
            bake_text: false,
            drawing: None,
            queue: Vec::new(),
            want_shot: false,
            renderer: None,
            last_xf: None,
        }
    }

    /// Whether the canvas owns pointer input over the image (a tool is selected or a
    /// drawing is open), so the host shouldn't use it.
    pub fn active(&self) -> bool {
        self.tool != Tool::Interact || self.drawing.is_some()
    }

    /// Whether a drawing is open: the host should show it instead of the live image and
    /// keep keyboard input in egui (undo, typing).
    pub fn drawing_open(&self) -> bool {
        self.drawing.is_some()
    }

    /// The topmost mark under screen point `p` (as painted last frame), with the Move tool.
    pub fn mark_at(&self, p: Pos2) -> Option<usize> {
        let (d, xf) = (self.drawing.as_ref()?, self.last_xf?);
        let tol = 6.0 + STROKE * d.scale * xf.zoom / 2.0;
        (0..d.marks.len()).rev().find(|&i| d.marks[i].hit(p, xf, tol, d.text_rects.get(i).copied().flatten()))
    }

    /// True once after a drawing started: the host should call [`Self::set_screenshot`].
    pub fn wants_screenshot(&mut self) -> bool {
        std::mem::take(&mut self.want_shot)
    }

    /// The frozen screenshot for the open drawing (premultiplied, opaque is fine).
    pub fn set_screenshot(&mut self, image: ColorImage) {
        if let Some(d) = &mut self.drawing {
            d.shot = Some(Shot { image: Arc::new(image), tex: None });
        }
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    pub fn undo(&mut self) {
        if let Some(d) = &mut self.drawing
            && let Some(m) = d.marks.pop()
        {
            d.redo.push(m);
        }
    }

    pub fn redo(&mut self) {
        if let Some(d) = &mut self.drawing
            && let Some(m) = d.redo.pop()
        {
            d.marks.push(m);
        }
    }

    /// Closes the open drawing without keeping it.
    pub fn discard(&mut self) {
        self.drawing = None;
        self.tool = Tool::Interact;
    }

    /// Composites the open drawing into the queue.
    pub fn queue_drawing(&mut self) {
        if let Some(out) = self.finish() {
            let thumb = downscale(&out.preview, THUMB);
            self.queue.push(Queued { out, thumb, tex: None, full: None });
        }
        self.tool = Tool::Interact;
    }

    /// Everything to send with a message: the queue, then the open drawing (both cleared).
    pub fn take_all(&mut self) -> Vec<Annotated> {
        let mut out: Vec<Annotated> = self.queue.drain(..).map(|q| q.out).collect();
        out.extend(self.finish());
        self.tool = Tool::Interact;
        out
    }

    /// Composites and closes the open drawing (None if there's none or no screenshot).
    fn finish(&mut self) -> Option<Annotated> {
        let mut d = self.drawing.take()?;
        if let Some(e) = d.editing.take() {
            push_text(&mut d, e);
        }
        let shot = d.shot?;
        let notes = d
            .marks
            .iter()
            .filter_map(|m| match &m.kind {
                Kind::Text(p, t) => Some(Note { text: t.clone(), pos: *p, color: m.color }),
                _ => None,
            })
            .collect();
        let r = self.renderer.get_or_insert_with(Renderer::default);
        let preview = Arc::new(r.composite(&shot.image, &d.marks, d.scale));
        let has_text = d.marks.iter().any(|m| matches!(m.kind, Kind::Text(..)));
        let image = if self.bake_text || !has_text {
            preview.clone()
        } else {
            let marks: Vec<Mark> = d.marks.into_iter().filter(|m| !matches!(m.kind, Kind::Text(..))).collect();
            Arc::new(r.composite(&shot.image, &marks, d.scale))
        };
        Some(Annotated { image, preview, notes })
    }

    /// The vertical tool strip.
    pub fn toolbar(&mut self, ui: &mut Ui) {
        let size = vec2(28.0, 28.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
        for t in Tool::ALL {
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            let sel = self.tool == t;
            let vis = ui.style().interact_selectable(&resp, sel);
            if sel || resp.hovered() {
                ui.painter().rect_filled(rect, 4.0, vis.weak_bg_fill);
            }
            tool_icon(ui.painter(), rect.shrink(6.0), t, vis.fg_stroke.color);
            if resp.on_hover_text(t.name()).clicked() {
                self.tool = t;
            }
        }
        ui.separator();
        for (c, name) in COLORS {
            let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 22.0), Sense::click());
            let sel = self.color == c;
            ui.painter().circle_filled(rect.center(), 8.0, c);
            ui.painter().circle_stroke(rect.center(), 8.0, Stroke::new(1.0, Color32::from_gray(90)));
            if sel {
                ui.painter().circle_stroke(rect.center(), 11.0, Stroke::new(2.0, ui.visuals().strong_text_color()));
            }
            if resp.on_hover_text(name).clicked() {
                self.color = c;
                if matches!(self.tool, Tool::Interact | Tool::Move) {
                    self.tool = Tool::Pen;
                }
            }
        }
        ui.separator();
        let can_undo = self.drawing.as_ref().is_some_and(|d| !d.marks.is_empty());
        let can_redo = self.drawing.as_ref().is_some_and(|d| !d.redo.is_empty());
        for (redo, can, tip) in [(false, can_undo, "Undo (Ctrl+Z)"), (true, can_redo, "Redo (Ctrl+Shift+Z)")] {
            let (rect, resp) = ui.allocate_exact_size(size, if can { Sense::click() } else { Sense::hover() });
            if can && resp.hovered() {
                ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let col = if can { ui.visuals().text_color() } else { ui.visuals().weak_text_color() };
            undo_icon(ui.painter(), rect.shrink(7.0), col, redo);
            if resp.on_hover_text(tip).clicked() {
                if redo {
                    self.redo();
                } else {
                    self.undo();
                }
            }
        }
    }

    /// Paints the frozen screenshot (if a drawing is open) and the marks over the host's
    /// image area, and handles primary-button drawing there. Call only while
    /// [`Self::active`]; the host keeps zoom/pan (scroll, middle-drag) for itself.
    pub fn canvas(&mut self, ui: &mut Ui, view: View) {
        let zoom = view.rect.width() / view.size[0].max(1) as f32;
        let xf = Xf { offset: view.rect.min.to_vec2(), zoom };
        let resp = ui.interact(view.clip, self.id.with("canvas"), Sense::click_and_drag());
        let (pressed, released, pos, double, moves, (undo, redo)) = ui.input_mut(|i| {
            let moves: Vec<Pos2> = i.events.iter().filter_map(|e| if let Event::PointerMoved(p) = e { Some(*p) } else { None }).collect();
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
                i.pointer.button_double_clicked(egui::PointerButton::Primary),
                moves,
                // consume_key ignores Shift, so take Ctrl+Shift+Z (redo) first.
                {
                    let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
                    (i.consume_key(Modifiers::COMMAND, Key::Z), redo)
                },
            )
        });
        let on_image = |p: Pos2| view.clip.contains(p) && view.rect.contains(p);
        let to_img = |p: Pos2| {
            let q = xf.invert(p);
            pos2(q.x.clamp(0.0, view.size[0] as f32), q.y.clamp(0.0, view.size[1] as f32))
        };
        if self.drawing.as_ref().is_some_and(|d| d.editing.is_none()) {
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
        }

        // Move tool: drag the mark under the press (elsewhere the host pans the view).
        if self.tool == Tool::Move {
            if pressed
                && resp.hovered()
                && let Some(p) = pos
            {
                if let Some(d) = &mut self.drawing
                    && let Some(e) = d.editing.take()
                {
                    push_text(d, e);
                }
                let hit = self.mark_at(p);
                if let (Some(d), Some(i)) = (&mut self.drawing, hit) {
                    d.moving = Some((i, xf.invert(p)));
                }
            }
            if let Some(d) = &mut self.drawing
                && let Some((i, last)) = &mut d.moving
            {
                for p in moves.iter().copied().chain(pos) {
                    let q = xf.invert(p);
                    if let Some(m) = d.marks.get_mut(*i) {
                        m.translate(q - *last);
                    }
                    *last = q;
                }
                if released {
                    d.moving = None;
                }
            }
        }

        // Press: start a mark (and a drawing, freezing the screenshot) or place text.
        if pressed
            && resp.hovered()
            && let Some(p) = pos.filter(|p| on_image(*p))
            && !matches!(self.tool, Tool::Interact | Tool::Move)
        {
            let q = to_img(p);
            let d = self.drawing.get_or_insert_with(|| {
                self.want_shot = true;
                Drawing { shot: None, scale: view.scale, marks: Vec::new(), active: None, editing: None, redo: Vec::new(), moving: None, text_rects: Vec::new() }
            });
            // A press elsewhere ends text editing (a press on the box itself isn't ours).
            let near_edit = d.editing.as_ref().is_some_and(|e| (e.pos - q).length() * zoom < 6.0);
            if !near_edit && let Some(e) = d.editing.take() {
                push_text(d, e);
            }
            let kind = match self.tool {
                Tool::Pen => Some(Kind::Pen(vec![q])),
                Tool::Box => Some(Kind::Box(q, q)),
                Tool::Ellipse => Some(Kind::Ellipse(q, q)),
                Tool::Line => Some(Kind::Line(q, q)),
                Tool::Arrow => Some(Kind::Arrow(q, q)),
                Tool::Text => {
                    if !near_edit {
                        d.editing = Some(Editing { pos: q, text: String::new(), color: self.color, focus: true });
                    }
                    None
                }
                Tool::Interact | Tool::Move => None,
            };
            d.active = kind.map(|kind| Mark { kind, color: self.color });
        }
        if let Some(d) = &mut self.drawing
            && let Some(m) = &mut d.active
        {
            for p in moves.into_iter().chain(pos) {
                let q = to_img(p);
                match &mut m.kind {
                    Kind::Pen(pts) => {
                        if pts.last().is_none_or(|l| (*l - q).length() * zoom >= 1.0) {
                            pts.push(q);
                        }
                    }
                    Kind::Box(_, b) | Kind::Ellipse(_, b) | Kind::Line(_, b) | Kind::Arrow(_, b) => *b = q,
                    Kind::Text(..) => {}
                }
            }
            if released {
                let m = d.active.take().unwrap();
                if !m.is_tiny(MIN_MARK / zoom) {
                    d.marks.push(m);
                    d.redo.clear();
                }
            }
        }
        // Double-click with any tool: text.
        if double
            && resp.hovered()
            && !matches!(self.tool, Tool::Text | Tool::Move)
            && let Some(d) = &mut self.drawing
            && let Some(p) = pos.filter(|p| on_image(*p))
        {
            d.active = None;
            d.editing = Some(Editing { pos: to_img(p), text: String::new(), color: self.color, focus: true });
        }

        let painter = ui.painter_at(view.clip);
        let mut shapes = Vec::new();
        if let Some(d) = &mut self.drawing {
            if let Some(shot) = &mut d.shot {
                let tex = shot.tex.get_or_insert_with(|| ui.ctx().load_texture("frozen-screenshot", (*shot.image).clone(), TextureOptions::LINEAR));
                shapes.push(Shape::image(tex.id(), view.rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE));
            }
            let mut layout = |t: String, size: f32, c: Color32| painter.layout_no_wrap(t, FontId::proportional(size), c);
            d.text_rects.clear();
            for m in d.marks.iter().chain(&d.active) {
                let first = shapes.len();
                m.shapes(d.scale, xf, &mut layout, &mut shapes);
                // A text mark's first shape is its backing.
                let r = matches!(m.kind, Kind::Text(..)).then(|| shapes.get(first).map(Shape::visual_bounding_rect)).flatten();
                d.text_rects.push(r);
            }
        }
        painter.extend(shapes);

        if let Some(d) = &mut self.drawing
            && d.editing.is_some()
        {
            self.text_editor(ui, xf);
        }
        if self.drawing.is_some() {
            self.bar(ui, view.clip);
        }
        self.last_xf = Some(xf);
        if self.tool == Tool::Move {
            // Over empty space the host shows its pan cursor.
            if self.drawing.as_ref().is_some_and(|d| d.moving.is_some()) {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else if resp.hovered() && pos.is_some_and(|p| self.mark_at(p).is_some()) {
                ui.ctx().set_cursor_icon(CursorIcon::Move);
            }
        } else if self.tool != Tool::Interact && resp.hovered() && pos.is_some_and(on_image) {
            ui.ctx().set_cursor_icon(if self.tool == Tool::Text { CursorIcon::Text } else { CursorIcon::Crosshair });
        }
    }

    fn text_editor(&mut self, ui: &mut Ui, xf: Xf) {
        let d = self.drawing.as_mut().unwrap();
        let e = d.editing.as_mut().unwrap();
        let k = d.scale * xf.zoom;
        let font = FontId::proportional(FONT * k);
        let pad = PAD * k;
        let (enter, esc) = ui.input_mut(|i| (crate::consume_plain_enter(i), i.consume_key(Modifiers::NONE, Key::Escape)));
        let min_width = FONT * k * 4.0;
        let color = e.color;
        let layout_font = font.clone();
        // No wrapping: the box grows with the text, like the mark it becomes.
        let mut layouter = move |ui: &Ui, text: &dyn egui::TextBuffer, _wrap: f32| ui.painter().layout_no_wrap(text.as_str().to_string(), layout_font.clone(), color);
        let r = egui::Area::new(self.id.with("text-edit"))
            .fixed_pos(xf.apply(e.pos) - Vec2::splat(pad))
            .order(Order::Foreground)
            .constrain(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::new().fill(text_backing(e.color)).inner_margin(pad).corner_radius(3.0 * k).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut e.text)
                            .font(font)
                            .text_color(e.color)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_rows(1)
                            .desired_width(min_width)
                            .layouter(&mut layouter),
                    )
                })
            });
        let te = r.inner.inner;
        if e.focus {
            te.request_focus();
            e.focus = false;
        }
        if esc {
            d.editing = None;
        } else if enter || te.lost_focus() {
            let e = d.editing.take().unwrap();
            push_text(d, e);
        }
    }

    /// The bar over an open drawing: what it is, and Queue / Discard.
    fn bar(&mut self, ui: &mut Ui, clip: Rect) {
        let mut queue = false;
        let mut discard = false;
        egui::Area::new(self.id.with("bar"))
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(pos2(clip.center().x, clip.min.y + 8.0))
            .order(Order::Middle)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Drawing on a frozen screenshot.");
                        queue = ui.button("Queue").on_hover_text("Keep it for the next message").clicked();
                        discard = ui.button("Discard").clicked();
                    });
                });
            });
        if queue {
            self.queue_drawing();
        } else if discard {
            self.discard();
        }
    }

    /// Queued thumbnails in a row, each with a delete button.
    pub fn queue_strip(&mut self, ui: &mut Ui) {
        let mut remove = None;
        for (i, q) in self.queue.iter_mut().enumerate() {
            let tex = q.tex.get_or_insert_with(|| ui.ctx().load_texture("queued", q.thumb.clone(), TextureOptions::LINEAR));
            let size = vec2(q.thumb.size[0] as f32, q.thumb.size[1] as f32) * (56.0 / q.thumb.size[1] as f32);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_gray(110)), egui::StrokeKind::Outside);
            let x = Rect::from_center_size(rect.right_top() + vec2(-9.0, 9.0), vec2(16.0, 16.0));
            let xr = ui.interact(x, self.id.with(("unqueue", i)), Sense::click());
            ui.painter().circle_filled(x.center(), 8.0, if xr.hovered() { Color32::from_rgb(200, 40, 40) } else { Color32::from_black_alpha(180) });
            let s = Stroke::new(1.5, Color32::WHITE);
            ui.painter().line_segment([x.center() + vec2(-3.5, -3.5), x.center() + vec2(3.5, 3.5)], s);
            ui.painter().line_segment([x.center() + vec2(3.5, -3.5), x.center() + vec2(-3.5, 3.5)], s);
            if xr.on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
            resp.on_hover_ui(|ui| {
                let p = &q.out.preview;
                let full = q.full.get_or_insert_with(|| ui.ctx().load_texture("queued-full", (**p).clone(), TextureOptions::LINEAR));
                let size = vec2(p.size[0] as f32, p.size[1] as f32);
                let f = (720.0 / size.x).min(480.0 / size.y).min(1.0);
                ui.image((full.id(), size * f));
            });
        }
        if let Some(i) = remove {
            self.queue.remove(i);
        }
    }
}

fn push_text(d: &mut Drawing, e: Editing) {
    let text = e.text.trim_end().to_string();
    if !text.trim().is_empty() {
        d.marks.push(Mark { kind: Kind::Text(e.pos, text), color: e.color });
        d.redo.clear();
    }
}

fn tool_icon(p: &egui::Painter, r: Rect, t: Tool, c: Color32) {
    let s = Stroke::new(1.6, c);
    match t {
        Tool::Interact => {
            // An arrow cursor.
            let o = r.left_top() + vec2(2.0, 0.0);
            let pts = [o, o + vec2(0.0, 14.0), o + vec2(4.0, 10.5), o + vec2(7.0, 16.0), o + vec2(9.0, 15.0), o + vec2(6.5, 9.5), o + vec2(11.0, 9.5)];
            p.add(Shape::closed_line(pts.to_vec(), s));
        }
        Tool::Move => {
            // Four-way arrows.
            let c0 = r.center();
            let h = r.width() / 2.0;
            for d in [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)] {
                let tip = c0 + d * h;
                let n = vec2(-d.y, d.x);
                p.line_segment([c0, tip - d * 2.0], s);
                p.add(Shape::convex_polygon(vec![tip, tip - d * 4.5 + n * 3.5, tip - d * 4.5 - n * 3.5], c, Stroke::NONE));
            }
        }
        Tool::Pen => {
            // A pencil, tip at the bottom left.
            let u = vec2(1.0, -1.0).normalized();
            let n = vec2(u.y, -u.x) * 2.6;
            let tip = r.left_bottom() + vec2(0.5, -0.5);
            let len = r.width() * 1.3;
            let (a, b) = (tip + u * 5.0, tip + u * len);
            p.add(Shape::closed_line(vec![tip, a + n, b + n, b - n, a - n], s));
            p.line_segment([a + n, a - n], s);
            p.line_segment([b - u * 3.5 + n, b - u * 3.5 - n], s);
        }
        Tool::Box => {
            p.rect_stroke(r.shrink(1.0), 0.0, s, egui::StrokeKind::Middle);
        }
        Tool::Ellipse => {
            p.add(Shape::ellipse_stroke(r.center(), vec2(r.width() / 2.0, r.height() / 2.0 * 0.75), s));
        }
        Tool::Line => {
            p.line_segment([r.left_bottom(), r.right_top()], s);
        }
        Tool::Arrow => {
            let (a, b) = (r.left_bottom(), r.right_top());
            p.line_segment([a, b - vec2(3.0, -3.0)], s);
            p.add(Shape::convex_polygon(vec![b, b + vec2(-8.0, 2.0), b + vec2(-2.0, 8.0)], c, Stroke::NONE));
        }
        Tool::Text => {
            p.text(r.center(), Align2::CENTER_CENTER, "T", FontId::proportional(r.height() + 2.0), c);
        }
    }
}

/// A hooked arrow: the head on the left pointing left, the tail curving back under it.
/// Mirrored for redo.
fn undo_icon(p: &egui::Painter, r: Rect, c: Color32, mirror: bool) {
    let s = Stroke::new(1.7, c);
    let m = |q: Pos2| if mirror { pos2(r.left() + r.right() - q.x, q.y) } else { q };
    let y0 = r.top() + r.height() * 0.3;
    let rad = r.height() * 0.32;
    let cx = r.right() - rad;
    let mut pts = vec![pos2(r.left() + 3.0, y0)];
    pts.extend((0..=12).map(|i| {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
        pos2(cx + a.cos() * rad, y0 + rad + a.sin() * rad)
    }));
    pts.push(pos2(r.left() + r.width() * 0.35, y0 + 2.0 * rad));
    p.add(Shape::line(pts.into_iter().map(m).collect(), s));
    let tip = pos2(r.left() - 0.5, y0);
    p.add(Shape::convex_polygon(vec![m(tip), m(tip + vec2(5.0, -4.0)), m(tip + vec2(5.0, 4.0))], c, Stroke::NONE));
}
