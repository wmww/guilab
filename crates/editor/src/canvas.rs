//! One canvas frame: input (pointer events in order, keys), gestures, hover, cursor, paint.

use egui::{CursorIcon, Event, Key, Modifiers, PointerButton, Pos2, Sense, Shape, Ui};
use guilab_image::geom::{self, Mods};
use guilab_image::{Body, Doc, Edit, EditError, ElemKey, Element, Fonts, IRect, Kind, Pt, Rect, element_prims, pt};
use guilab_kit::Theme;

use crate::paint::{self, Ctx};
use crate::text::Typing;
use crate::{Canvas, CanvasOut, CropMode, Editor, Gesture, Hit, MIN_MARK, PEN_STEP, Tool, Xf};

/// Element under `p` by the move tool's priority: key points (topmost element first), then
/// elements within `hit_slop` (topmost first). The locked base never hits.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hit_test(body: &Body, keys: &[ElemKey], p: Pt, zoom: f32, t: &Theme, fonts: &Fonts, region: IRect, key_points: bool, skip: Option<ElemKey>) -> Hit {
    let r = region.to_rect();
    let kr = t.size.key_point_hit_r / zoom;
    let live = |i: usize| !body.is_base(i) && Some(keys[i]) != skip;
    if key_points && r.expand(kr).contains(p) {
        for i in (0..body.elements.len()).rev().filter(|&i| live(i)) {
            let e = &body.elements[i];
            let best = e
                .key_points(body.scale, fonts)
                .into_iter()
                .filter(|k| r.expand(kr).contains(k.at))
                .map(|k| ((k.at - p).len(), k.key))
                .filter(|(d, _)| *d <= kr)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, k)) = best {
                return Hit::Key(keys[i], k);
            }
        }
    }
    if r.contains(p) {
        let tol = t.size.hit_slop / zoom;
        for i in (0..body.elements.len()).rev().filter(|&i| live(i)) {
            if body.elements[i].hit(p, tol, body.scale, fonts) {
                return Hit::Item(keys[i]);
            }
        }
    }
    Hit::None
}

/// The topmost text element whose box contains `p`.
fn text_at(body: &Body, keys: &[ElemKey], p: Pt, fonts: &Fonts) -> Option<ElemKey> {
    (0..body.elements.len())
        .rev()
        .find(|&i| matches!(body.elements[i].kind, Kind::Text { .. }) && body.elements[i].bounds(body.scale, fonts).contains(p))
        .map(|i| keys[i])
}

fn crop_corners(r: Rect) -> [Pt; 8] {
    let (a, b, c) = (r.min, r.max(), r.center());
    [a, pt(b.x, a.y), b, pt(a.x, b.y), pt(c.x, a.y), pt(b.x, c.y), pt(c.x, b.y), pt(a.x, c.y)]
}

/// Crop tool: what a press at `p` does to crop `c`.
/// `whole`: there's no crop (`c` is the canvas), so a drag inside starts a new one.
fn crop_hit(c: IRect, whole: bool, p: Pt, zoom: f32, t: &Theme) -> CropMode {
    let r = c.to_rect();
    let near = (t.size.key_point_r + t.size.hit_slop) / zoom;
    let h = crop_corners(r);
    if let Some(i) = (0..4).find(|&i| (h[i] - p).len() <= near) {
        return CropMode::Handle(i as u8);
    }
    let tol = t.size.hit_slop / zoom;
    let (a, b) = (r.min, r.max());
    let in_x = p.x >= a.x - tol && p.x <= b.x + tol;
    let in_y = p.y >= a.y - tol && p.y <= b.y + tol;
    if in_x && (p.y - a.y).abs() <= tol {
        return CropMode::Handle(4);
    }
    if in_y && (p.x - b.x).abs() <= tol {
        return CropMode::Handle(5);
    }
    if in_x && (p.y - b.y).abs() <= tol {
        return CropMode::Handle(6);
    }
    if in_y && (p.x - a.x).abs() <= tol {
        return CropMode::Handle(7);
    }
    if r.contains(p) && !whole { CropMode::Move } else { CropMode::New }
}

fn crop_cursor(m: CropMode) -> CursorIcon {
    match m {
        CropMode::Handle(0 | 2) => CursorIcon::ResizeNwSe,
        CropMode::Handle(1 | 3) => CursorIcon::ResizeNeSw,
        CropMode::Handle(4 | 6) => CursorIcon::ResizeVertical,
        CropMode::Handle(_) => CursorIcon::ResizeHorizontal,
        CropMode::Move => CursorIcon::AllScroll,
        CropMode::New => CursorIcon::Crosshair,
    }
}

/// The crop after dragging from `start` to `q`, in whole pixels inside the canvas.
fn crop_drag(mode: CropMode, orig: IRect, start: Pt, q: Pt, canvas: [u32; 2]) -> IRect {
    let (cw, ch) = (canvas[0] as f32, canvas[1] as f32);
    let q = pt(q.x.clamp(0.0, cw).round(), q.y.clamp(0.0, ch).round());
    let o = orig.to_rect();
    let (mut a, mut b) = (o.min, o.max());
    match mode {
        CropMode::New => {
            let s = pt(start.x.clamp(0.0, cw).round(), start.y.clamp(0.0, ch).round());
            return Rect::from_corners(s, q).round_out();
        }
        CropMode::Move => {
            let d = pt((q.x - start.x.round()).clamp(-a.x, cw - b.x), (q.y - start.y.round()).clamp(-a.y, ch - b.y));
            return o.translate(d).round_out();
        }
        CropMode::Handle(h) => {
            let min = 1.0;
            let (l, t, r, bo) = match h {
                0 => (true, true, false, false),
                1 => (false, true, true, false),
                2 => (false, false, true, true),
                3 => (true, false, false, true),
                4 => (false, true, false, false),
                5 => (false, false, true, false),
                6 => (false, false, false, true),
                _ => (true, false, false, false),
            };
            if l {
                a.x = q.x.min(b.x - min);
            }
            if r {
                b.x = q.x.max(a.x + min);
            }
            if t {
                a.y = q.y.min(b.y - min);
            }
            if bo {
                b.y = q.y.max(a.y + min);
            }
        }
    }
    Rect::from_corners(a, b).round_out()
}

/// Shift while drawing: squares and circles, lines at 45° steps.
fn constrain(kind: &Kind, start: Pt, q: Pt) -> Pt {
    let d = q - start;
    match kind {
        Kind::Box(_) | Kind::Ellipse(_) => {
            let s = d.x.abs().max(d.y.abs());
            pt(start.x + s * d.x.signum(), start.y + s * d.y.signum())
        }
        Kind::Line(..) | Kind::Arrow(..) => {
            let step = std::f32::consts::FRAC_PI_4;
            let a = (d.y.atan2(d.x) / step).round() * step;
            start + pt(a.cos(), a.sin()) * d.len()
        }
        _ => q,
    }
}

fn frame_of(el: &Element) -> Option<Rect> {
    Some(match &el.kind {
        Kind::Pen(p) => Rect::bounding(p),
        Kind::Box(r) | Kind::Ellipse(r) => *r,
        Kind::Line(a, b) | Kind::Arrow(a, b) => Rect::from_corners(*a, *b),
        _ => return None,
    })
}

fn key_cursor(k: geom::Key, el: &Element, scale: f32, fonts: &Fonts) -> CursorIcon {
    match k {
        geom::Key::End(_) => CursorIcon::Crosshair,
        geom::Key::Corner(i) => {
            // Which way the corner points, from the element's frame (corners of a flipped
            // frame keep their index).
            let f = el.frame(scale, fonts);
            let kp = el.key_points(scale, fonts);
            let at = kp.iter().find(|p| p.key == k).map(|p| p.at).unwrap_or(f.min);
            let c = f.center();
            if (at.x < c.x) == (at.y < c.y) || i > 3 { CursorIcon::ResizeNwSe } else { CursorIcon::ResizeNeSw }
        }
    }
}

/// Applies and reports an edit, or reports why the doc refused it.
pub(crate) fn apply(doc: &mut Doc, e: Result<Edit, EditError>, out: &mut CanvasOut) {
    let e = match e {
        Ok(e) => e,
        Err(err) => {
            out.refused = Some(err.to_string());
            return;
        }
    };
    doc.apply(&e);
    out.edits.push(e);
}

impl Editor {
    /// Paints the doc (or, without one, what's being drawn) and handles primary-button
    /// input over `c.clip`. The host keeps middle-drag, wheel zoom and Undo for itself.
    pub fn canvas(&mut self, ui: &mut Ui, c: Canvas) -> CanvasOut {
        let mut out = CanvasOut::default();
        let Canvas { mut doc, xf, clip, new_doc, visible } = c;
        self.sync_doc(doc.as_ref().map(|d| d.id()));
        let t = Theme::get(ui.ctx());
        let ctx = ui.ctx().clone();
        let fonts_ready = paint::install_fonts(&ctx);
        self.textures.begin_frame();
        out.wants_keys = doc.is_some() || self.typing.is_some();
        let (canvas, scale, crop) = match &doc {
            Some(d) => (d.body().canvas, d.body().scale, d.body().crop),
            None => (new_doc.0, new_doc.1, None),
        };
        let region = self.view_region(canvas, crop);
        out.shown = Some(xf.rect(region.to_rect()));
        if !visible {
            return out;
        }

        // Keep state consistent with a doc that changed under us (undo mid-drag).
        if let Some(d) = &doc {
            let gone = |k: ElemKey| d.index_of(k).is_none();
            match &self.gesture {
                Gesture::Move { key, .. } | Gesture::Reshape { key, .. } if gone(*key) => self.gesture = Gesture::Idle,
                _ => {}
            }
            if self.typing.as_ref().is_some_and(|ty| ty.key.is_some_and(gone)) {
                self.typing = None;
            }
            if let Hit::Key(k, _) | Hit::Item(k) = self.hover
                && gone(k)
            {
                self.hover = Hit::None;
            }
        }
        // A tool change ends typing; Interact ends everything.
        if self.typing.as_ref().is_some_and(|ty| ty.tool != self.tool) {
            let ty = self.typing.take().unwrap();
            self.finish_typing(ty, doc.as_deref_mut(), &mut out);
        }
        if self.tool == Tool::Interact {
            self.cancel_gesture(&mut out);
        }
        if let Some(d) = doc.as_deref_mut() {
            for el in self.pending.drain(..) {
                let e = d.add_edit(el);
                apply(d, e, &mut out);
                self.began = false;
            }
        }

        // Claims the area so egui widgets behind it don't react; input is read raw below.
        ui.interact(clip, self.id.with("canvas"), Sense::click_and_drag());
        let layer = ui.layer_id();
        // Our own text field's area counts as ours once typing ended (it lingers a frame).
        let text_layer = egui::LayerId::new(egui::Order::Foreground, self.id.with("text-area"));
        let typing = self.typing.is_some();
        let owns = |p: Pos2| clip.contains(p) && ctx.layer_id_at(p).is_none_or(|l| l == layer || l == text_layer && !typing);
        let (events, now, mods) = ui.input(|i| (i.events.clone(), i.time, i.modifiers));
        let keys_free = !ctx.egui_wants_keyboard_input();

        // Keys (not while a text field has focus).
        if self.tool != Tool::Interact && keys_free && self.typing.is_none() && (doc.is_some() || self.busy()) {
            // Esc and Delete are taken only when they do something, so the host can use them
            // otherwise (e.g. Esc to close).
            if self.busy() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.cancel_gesture(&mut out);
            }
            let deletable = self.tool == Tool::Move && !self.busy() && !matches!(self.hover, Hit::None);
            if deletable
                && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace))
                && let Some(d) = doc.as_deref_mut()
                && let Some(e) = self.delete_hovered(d)
            {
                out.edits.push(e);
            }
            if doc.is_some() && matches!(self.gesture, Gesture::Idle) {
                for tool in Tool::ALL {
                    if let Some(k) = tool.key()
                        && ui.input_mut(|i| i.consume_key(Modifiers::NONE, k))
                    {
                        self.tool = tool;
                    }
                }
            }
        }

        if self.tool != Tool::Interact {
            for ev in &events {
                match ev {
                    Event::PointerMoved(p) => {
                        self.pointer = Some(*p);
                        self.drag_to(*p, mods, canvas, scale, xf, region, &mut out);
                    }
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => {
                        self.pointer = Some(*pos);
                                        if owns(*pos) {
                            let double = self.last_press.is_some_and(|(t0, p0)| now - t0 < 0.35 && (p0 - *pos).length() < 6.0);
                            self.last_press = if double { None } else { Some((now, *pos)) };
                            self.press(*pos, double, doc.as_deref_mut(), canvas, scale, xf, region, &t, &mut out);
                        }
                    }
                    Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers } => {
                        self.drag_to(*pos, *modifiers, canvas, scale, xf, region, &mut out);
                        self.release(doc.as_deref_mut(), xf, &mut out);
                    }
                    Event::PointerGone => self.pointer = None,
                    _ => {}
                }
            }
        }

        // Hover and cursor.
        // During a drag the cursor stays what the drag set, wherever the pointer is.
        let dragging = !matches!(self.gesture, Gesture::Idle);
        let over = self.pointer.filter(|p| dragging || owns(*p));
        self.hover = Hit::None;
        let mut cursor = None;
        if self.tool != Tool::Interact
            && let Some(p) = over
        {
            let q = xf.invert(p);
            let in_region = region.to_rect().contains(q);
            cursor = Some(match (&self.gesture, self.tool) {
                (Gesture::Pan { .. }, _) => CursorIcon::Grabbing,
                (Gesture::Move { .. }, _) => CursorIcon::AllScroll,
                (Gesture::Reshape { k, cur, .. }, _) => key_cursor(*k, cur, scale, &self.fonts),
                (Gesture::Crop { mode, .. }, _) => crop_cursor(*mode),
                (Gesture::Draw { .. }, _) => CursorIcon::Crosshair,
                (Gesture::Idle, Tool::Move) => {
                    if let Some(d) = &doc {
                        let skip = self.typing.as_ref().and_then(|ty| ty.key);
                        self.hover = hit_test(d.body(), d.keys(), q, xf.zoom, &t, &self.fonts, region, true, skip);
                    }
                    match self.hover {
                        Hit::Key(k, kp) => {
                            let d = doc.as_deref().unwrap();
                            key_cursor(kp, d.element(k).unwrap(), scale, &self.fonts)
                        }
                        Hit::Item(_) => CursorIcon::AllScroll,
                        Hit::None => CursorIcon::Grab,
                    }
                }
                (Gesture::Idle, Tool::Crop) => match &doc {
                    Some(d) => crop_cursor(crop_hit(d.body().crop.unwrap_or(IRect::of_size(canvas)), d.body().crop.is_none(), q, xf.zoom, &t)),
                    None => CursorIcon::NotAllowed,
                },
                (Gesture::Idle, Tool::Text) if in_region => CursorIcon::Text,
                (Gesture::Idle, _) if in_region => CursorIcon::Crosshair,
                // Outside the image: the host's own cursor.
                (Gesture::Idle, _) => CursorIcon::Default,
            });
            if cursor == Some(CursorIcon::Default) {
                cursor = None;
            }
        }
        if let Some(cur) = cursor {
            ctx.set_cursor_icon(cur);
        }
        out.cursor = cursor;

        self.paint(ui, doc.as_deref(), xf, clip, region, scale, &t, fonts_ready);
        if self.typing.is_some() {
            self.text_ui(ui, doc.as_deref_mut(), xf, region, scale, fonts_ready, &mut out);
        }
        if self.tool == Tool::Crop
            && let Some(d) = doc.as_deref_mut()
        {
            let mut show_all = false;
            let has_crop = d.body().crop.is_some();
            guilab_kit::toast(ui, self.id.with("crop-bar"), clip, 0, |ui| {
                ui.label("Drag to crop");
                show_all = ui.add_enabled(has_crop, egui::Button::new("Show all")).on_hover_text("Remove the crop").clicked();
            });
            if show_all {
                let e = d.crop_edit(None);
                apply(d, Ok(e), &mut out);
            }
        }
        out.wants_keys = doc.is_some() || self.typing.is_some();
        out
    }

    fn cancel_gesture(&mut self, out: &mut CanvasOut) {
        if matches!(self.gesture, Gesture::Draw { .. }) && self.began && self.pending.is_empty() && self.typing.is_none() {
            self.began = false;
            out.abandon = true;
        }
        self.gesture = Gesture::Idle;
    }

    #[allow(clippy::too_many_arguments)]
    fn press(&mut self, p: Pos2, double: bool, mut doc: Option<&mut Doc>, canvas: [u32; 2], scale: f32, xf: Xf, region: IRect, t: &Theme, out: &mut CanvasOut) {
        let q = xf.invert(p);
        let in_region = region.to_rect().contains(q);
        // A press outside the text box ends typing. With the text tool that's all it does.
        if let Some(ty) = self.typing.take() {
            self.finish_typing(ty, doc.as_deref_mut(), out);
            if self.tool == Tool::Text && !double {
                return;
            }
        }
        if !matches!(self.gesture, Gesture::Idle) {
            return;
        }
        // Double-click: edit the text under the pointer, or (drawing tools) new text.
        if double && matches!(self.tool, Tool::Move | Tool::Text) || double && self.tool.draws() {
            let hit = doc.as_deref().and_then(|d| text_at(d.body(), d.keys(), q, &self.fonts));
            if let (Some(k), Some(d)) = (hit, doc.as_deref()) {
                self.typing = Some(Typing::edit(d, k, self.tool, region, &self.fonts, (p, true)));
                return;
            }
            if self.tool.draws() && in_region {
                self.start_text(q, doc.is_none(), scale, region, out);
                return;
            }
        }
        match self.tool {
            Tool::Interact => {}
            Tool::Move => {
                let hit = match doc.as_deref() {
                    Some(d) => hit_test(d.body(), d.keys(), q, xf.zoom, t, &self.fonts, region, true, None),
                    None => Hit::None,
                };
                self.gesture = match (hit, doc.as_deref()) {
                    (Hit::Key(key, k), Some(d)) => {
                        let el = d.element(key).unwrap().clone();
                        let at = el.key_points(scale, &self.fonts).into_iter().find(|kp| kp.key == k).map_or(q, |kp| kp.at);
                        Gesture::Reshape { key, k, cur: el.clone(), orig: el, grab: q - at }
                    }
                    (Hit::Item(key), Some(d)) => {
                        let el = d.element(key).unwrap().clone();
                        Gesture::Move { key, cur: el.clone(), orig: el, start: q }
                    }
                    _ => Gesture::Pan { last: p },
                };
            }
            Tool::Text => {
                if let Some(d) = doc.as_deref()
                    && let Some(k) = text_at(d.body(), d.keys(), q, &self.fonts)
                {
                    self.typing = Some(Typing::edit(d, k, self.tool, region, &self.fonts, (p, false)));
                } else if in_region {
                    self.start_text(q, doc.is_none(), scale, region, out);
                }
            }
            Tool::Crop => {
                if let Some(d) = doc.as_deref() {
                    let c = d.body().crop.unwrap_or(IRect::of_size(canvas));
                    let mode = crop_hit(c, d.body().crop.is_none(), q, xf.zoom, t);
                    self.gesture = Gesture::Crop { mode, orig: c, cur: c, start: q };
                }
            }
            Tool::Pen | Tool::Box | Tool::Ellipse | Tool::Line | Tool::Arrow => {
                if !in_region {
                    return;
                }
                let kind = match self.tool {
                    Tool::Pen => Kind::Pen(vec![q]),
                    Tool::Box => Kind::Box(Rect::from_corners(q, q)),
                    Tool::Ellipse => Kind::Ellipse(Rect::from_corners(q, q)),
                    Tool::Line => Kind::Line(q, q),
                    _ => Kind::Arrow(q, q),
                };
                self.gesture = Gesture::Draw { el: Element::new(self.color, kind), start: q };
                if doc.is_none() && !self.began {
                    self.began = true;
                    out.begin = true;
                }
            }
        }
    }

    fn start_text(&mut self, q: Pt, no_doc: bool, scale: f32, region: IRect, out: &mut CanvasOut) {
        self.typing = Some(Typing::new(q, self.color, scale, region, self.tool, &self.fonts));
        if no_doc && !self.began {
            self.began = true;
            out.begin = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn drag_to(&mut self, p: Pos2, mods: Modifiers, canvas: [u32; 2], scale: f32, xf: Xf, region: IRect, out: &mut CanvasOut) {
        let q = xf.invert(p);
        let r = region.to_rect();
        let clamp = |q: Pt| pt(q.x.clamp(r.min.x, r.max().x), q.y.clamp(r.min.y, r.max().y));
        match &mut self.gesture {
            Gesture::Idle => {}
            Gesture::Draw { el, start } => {
                let start = *start;
                let q = clamp(q);
                match &mut el.kind {
                    Kind::Pen(pts) => {
                        if pts.last().is_none_or(|l| (*l - q).len() * xf.zoom >= PEN_STEP) {
                            pts.push(q);
                        }
                    }
                    k => {
                        let q = if mods.shift { clamp(constrain(k, start, q)) } else { q };
                        match k {
                            Kind::Box(rr) | Kind::Ellipse(rr) => *rr = Rect::from_corners(start, q),
                            Kind::Line(_, b) | Kind::Arrow(_, b) => *b = q,
                            _ => {}
                        }
                    }
                }
            }
            Gesture::Move { orig, cur, start, .. } => {
                let mut d = q - *start;
                if mods.shift {
                    if d.x.abs() > d.y.abs() { d.y = 0.0 } else { d.x = 0.0 }
                }
                *cur = orig.translated(d);
            }
            Gesture::Reshape { k, orig, cur, grab, .. } => {
                *cur = orig.drag_key(*k, q - *grab, Mods { shift: mods.shift }, scale, &self.fonts);
            }
            Gesture::Pan { last } => {
                out.pan += p - *last;
                *last = p;
            }
            Gesture::Crop { mode, orig, cur, start } => {
                *cur = crop_drag(*mode, *orig, *start, q, canvas);
            }
        }
    }

    fn release(&mut self, doc: Option<&mut Doc>, xf: Xf, out: &mut CanvasOut) {
        let g = std::mem::replace(&mut self.gesture, Gesture::Idle);
        match g {
            Gesture::Draw { el, .. } => {
                let tiny = frame_of(&el).is_none_or(|f| f.size.w.hypot(f.size.h) * xf.zoom < MIN_MARK);
                if tiny {
                    if self.began && self.pending.is_empty() && self.typing.is_none() {
                        self.began = false;
                        out.abandon = true;
                    }
                    return;
                }
                match doc {
                    Some(d) => {
                        let e = d.add_edit(el);
                        apply(d, e, out);
                        self.began = false;
                    }
                    None => self.pending.push(el),
                }
            }
            Gesture::Move { key, orig, cur, .. } | Gesture::Reshape { key, orig, cur, .. } => {
                if cur != orig
                    && let Some(d) = doc
                {
                    let e = d.replace_edit(key, cur);
                    apply(d, e, out);
                }
            }
            Gesture::Crop { orig, cur, mode, .. } => {
                let Some(d) = doc else { return };
                if mode == CropMode::New && (cur.w < 2 || cur.h < 2) {
                    return;
                }
                let new = if cur == IRect::of_size(d.body().canvas) { None } else { Some(cur) };
                if new != d.body().crop && (cur != orig || mode == CropMode::New) {
                    let e = d.crop_edit(new);
                    apply(d, Ok(e), out);
                }
            }
            Gesture::Idle | Gesture::Pan { .. } => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(&mut self, ui: &Ui, doc: Option<&Doc>, xf: Xf, clip: egui::Rect, region: IRect, scale: f32, t: &Theme, fonts_ready: bool) {
        let shown = xf.rect(region.to_rect()).intersect(clip);
        let painter = ui.painter_at(shown);
        let mut shapes = Vec::new();
        let ppp = ui.ctx().pixels_per_point();
        let ctx = ui.ctx().clone();
        // Without a doc there are only pending marks, no pictures.
        let assets: &dyn guilab_image::Assets = match doc {
            Some(d) => d,
            None => &guilab_image::NoAssets,
        };
        let mut pc = Ctx { xf, assets, textures: &mut self.textures, ctx: &ctx, fonts_ready, ppp };
        let fonts = self.fonts.clone();
        let mut prims = Vec::new();
        let halo = Some((t.colors.hover_outline, 3.0));
        // The element a drag would move, or is moving, gets a glow.
        let lit = match (&self.gesture, self.hover) {
            (Gesture::Move { key, .. } | Gesture::Reshape { key, .. }, _) => Some(*key),
            (Gesture::Idle, Hit::Key(k, _) | Hit::Item(k)) => Some(k),
            _ => None,
        };
        let editing = self.typing.as_ref().and_then(|ty| ty.key);
        let mut outlines = Vec::new();
        if let Some(d) = doc {
            let body = d.body();
            for (i, el) in body.elements.iter().enumerate() {
                let key = d.keys()[i];
                if Some(key) == editing {
                    continue;
                }
                let el = match &self.gesture {
                    Gesture::Move { key: k, cur, .. } | Gesture::Reshape { key: k, cur, .. } if *k == key => cur,
                    _ => el,
                };
                element_prims(el, scale, &fonts, &mut prims);
                if lit == Some(key) {
                    if el.is_picture() || matches!(el.kind, Kind::Text { .. }) {
                        outlines.push(xf.rect(el.bounds(scale, &fonts)));
                    } else {
                        for p in &prims {
                            paint::prim_shapes(p, &mut pc, &mut shapes, halo);
                        }
                    }
                }
                for p in prims.drain(..) {
                    paint::prim_shapes(&p, &mut pc, &mut shapes, None);
                }
            }
        }
        for el in self.pending.iter().chain(match &self.gesture {
            Gesture::Draw { el, .. } => Some(el),
            _ => None,
        }) {
            element_prims(el, scale, &fonts, &mut prims);
            for p in prims.drain(..) {
                paint::prim_shapes(&p, &mut pc, &mut shapes, None);
            }
        }
        for r in outlines {
            paint::outline(&mut shapes, r, t.colors.hover_outline, 2.0);
        }
        painter.extend(shapes);

        // Overlays draw over the whole clip (key points on the region's edge stay whole).
        let mut over = Vec::new();
        if self.tool == Tool::Move
            && let Some(d) = doc
        {
            let body = d.body();
            let hot = match (&self.gesture, self.hover) {
                (Gesture::Reshape { key, k, .. }, _) => Some((*key, *k)),
                (Gesture::Idle, Hit::Key(key, k)) => Some((key, k)),
                _ => None,
            };
            let r = region.to_rect();
            let mut lit_points = Vec::new();
            for (i, el) in body.elements.iter().enumerate() {
                let key = d.keys()[i];
                if body.is_base(i) || Some(key) == editing {
                    continue;
                }
                let el = match &self.gesture {
                    Gesture::Move { key: k, cur, .. } | Gesture::Reshape { key: k, cur, .. } if *k == key => cur,
                    _ => el,
                };
                for kp in el.key_points(scale, &fonts) {
                    if !r.expand(0.5).contains(kp.at) {
                        continue;
                    }
                    let is_hot = hot == Some((key, kp.key));
                    if lit == Some(key) {
                        lit_points.push((xf.apply(kp.at), is_hot));
                    } else {
                        paint::key_point(&mut over, xf.apply(kp.at), is_hot, t);
                    }
                }
            }
            // The lit element's points go on top.
            for (p, hot) in lit_points {
                paint::key_point(&mut over, p, hot, t);
            }
        }
        if self.tool == Tool::Crop
            && let Some(d) = doc
        {
            let c = match &self.gesture {
                Gesture::Crop { cur, .. } => *cur,
                _ => d.body().crop.unwrap_or(IRect::of_size(d.body().canvas)),
            };
            let whole = xf.rect(IRect::of_size(d.body().canvas).to_rect());
            let cr = xf.rect(c.to_rect());
            paint::shade(&mut over, whole, cr, t.colors.crop_dim);
            over.push(Shape::rect_stroke(cr, 0.0, egui::Stroke::new(t.size.line, t.colors.crop_line), egui::StrokeKind::Outside));
            for p in crop_corners(c.to_rect()) {
                paint::key_point(&mut over, xf.apply(p), false, t);
            }
        }
        ui.painter_at(clip).extend(over);
    }
}
