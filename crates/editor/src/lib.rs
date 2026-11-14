//! The image editor widget: drawing tools, the move tool (move, reshape by key points,
//! pan), text editing, crop and inserted pictures, over a host-owned [`Doc`].
//!
//! Data in, edits out: the host passes the doc, assets and the view transform each frame
//! ([`Canvas`]) and gets back the [`Edit`]s already applied to the doc ([`CanvasOut`]). Undo
//! is the host's (e.g. a `guilab_kit::History<Edit>`), and so are zoom and pan: the editor
//! only reports pan drags. No guilab app types; [`demo::DemoHost`] is a complete host.

mod canvas;
pub mod demo;
mod paint;
mod text;
mod toolbar;

use std::sync::Arc;

use egui::{CursorIcon, Id, Pos2, Vec2, pos2};
use guilab_image::{AssetId, Assets, Color, Doc, DocId, Edit, ElemKey, Element, Fonts, IRect, Kind, Pt, Rect, Size, pt};

pub use paint::{FAMILY, install_fonts};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// No editing: the host uses pointer input itself (e.g. the live app).
    Interact,
    Move,
    Pen,
    Box,
    Ellipse,
    Line,
    Arrow,
    Text,
    Crop,
}

impl Tool {
    pub const ALL: [Tool; 9] = [Tool::Interact, Tool::Move, Tool::Pen, Tool::Box, Tool::Ellipse, Tool::Line, Tool::Arrow, Tool::Text, Tool::Crop];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Interact => "Use the app",
            Tool::Move => "Move, resize, or drag to pan",
            Tool::Pen => "Pen",
            Tool::Box => "Box",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Text => "Text",
            Tool::Crop => "Crop",
        }
    }

    /// Single-key shortcut, active while the canvas has the keyboard.
    pub fn key(self) -> Option<egui::Key> {
        use egui::Key as K;
        Some(match self {
            Tool::Interact => return None,
            Tool::Move => K::V,
            Tool::Pen => K::P,
            Tool::Box => K::R,
            Tool::Ellipse => K::O,
            Tool::Line => K::L,
            Tool::Arrow => K::A,
            Tool::Text => K::T,
            Tool::Crop => K::C,
        })
    }

    /// Tools that draw a new mark on a press.
    pub fn draws(self) -> bool {
        matches!(self, Tool::Pen | Tool::Box | Tool::Ellipse | Tool::Line | Tool::Arrow | Tool::Text)
    }
}

/// Canvas pixels to screen points: `screen = canvas * zoom + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub offset: Vec2,
    pub zoom: f32,
}

impl Xf {
    pub fn apply(&self, p: Pt) -> Pos2 {
        pos2(p.x * self.zoom + self.offset.x, p.y * self.zoom + self.offset.y)
    }
    pub fn invert(&self, p: Pos2) -> Pt {
        pt((p.x - self.offset.x) / self.zoom, (p.y - self.offset.y) / self.zoom)
    }
    pub fn rect(&self, r: Rect) -> egui::Rect {
        egui::Rect::from_min_max(self.apply(r.min), self.apply(r.max()))
    }
    /// The transform that fits canvas rect `r` centered in `screen`, at most `max_zoom`.
    pub fn fit(r: IRect, screen: egui::Rect, max_zoom: f32) -> Xf {
        let zoom = (screen.width() / r.w.max(1) as f32).min(screen.height() / r.h.max(1) as f32).min(max_zoom);
        let size = egui::vec2(r.w as f32, r.h as f32) * zoom;
        let min = screen.center() - size / 2.0;
        Xf { offset: min.to_vec2() - egui::vec2(r.x as f32, r.y as f32) * zoom, zoom }
    }
}

/// What the host gives the canvas each frame.
pub struct Canvas<'a> {
    /// `None` = no document yet (e.g. the live app): a drawing press asks for one
    /// ([`CanvasOut::begin`]), and the mark is committed once it's there.
    pub doc: Option<&'a mut Doc>,
    pub assets: &'a dyn Assets,
    pub xf: Xf,
    /// The screen area the canvas owns.
    pub clip: egui::Rect,
    /// Canvas size and scale a new doc would have (sizes marks while `doc` is `None`).
    pub new_doc: ([u32; 2], f32),
    /// false while the host shows the image some other way (e.g. flying to a slot): no
    /// painting, no input.
    pub visible: bool,
}

#[derive(Debug, Default)]
pub struct CanvasOut {
    /// Edits made this frame, already applied to the doc. The host records them for undo.
    pub edits: Vec<Edit>,
    /// A drawing started without a doc: create one now (or soon).
    pub begin: bool,
    /// The drawing that asked for `begin` ended with nothing to commit (a stray click or
    /// Esc): the host may drop the doc it made.
    pub abandon: bool,
    /// Screen points to pan the view by (move tool dragging empty space).
    pub pan: Vec2,
    /// The cursor over the canvas this frame (also set on the context). Never the arrow
    /// while a tool takes input there.
    pub cursor: Option<CursorIcon>,
    /// The canvas wants keyboard input (a doc is open or text is being typed).
    pub wants_keys: bool,
    /// Screen rect of the part of the canvas shown ([`Editor::view_region`]).
    pub shown: Option<egui::Rect>,
}

/// What a press would grab, by priority.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// A key point: reshape the element.
    Key(ElemKey, guilab_image::geom::Key),
    /// The element or near it: move it.
    Item(ElemKey),
    /// Nothing: pan.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CropMode {
    New,
    Move,
    /// Handle: corners 0..4 clockwise from top-left, then edges 4..8 (top, right, bottom,
    /// left).
    Handle(u8),
}

#[derive(Clone, Debug)]
enum Gesture {
    Idle,
    Draw { el: Element, start: Pt },
    Move { key: ElemKey, orig: Element, cur: Element, start: Pt },
    Reshape { key: ElemKey, k: guilab_image::geom::Key, orig: Element, cur: Element, grab: Pt },
    Pan { last: Pos2 },
    Crop { mode: CropMode, orig: IRect, cur: IRect, start: Pt },
}

pub struct Editor {
    pub tool: Tool,
    pub color: Color,
    id: Id,
    fonts: Arc<Fonts>,
    /// The doc that gesture, typing and hover refer to (keys are only unique within a doc).
    doc_id: Option<DocId>,
    gesture: Gesture,
    typing: Option<text::Typing>,
    /// Marks finished while there was no doc, committed when one arrives.
    pending: Vec<Element>,
    /// `begin` was reported and nothing has been committed or abandoned since.
    began: bool,
    hover: Hit,
    /// Last pointer position over the canvas.
    pointer: Option<Pos2>,
    last_press: Option<(f64, Pos2)>,
    textures: paint::Textures,
}

/// A mark smaller than this (screen points) is a stray click.
const MIN_MARK: f32 = 3.0;
/// Pen points closer than this (screen points) are skipped.
const PEN_STEP: f32 = 1.0;
/// Text wraps at this many font sizes (or the right edge), and a new box gets at least
/// `MIN_TEXT_EMS` before the edge.
pub const WRAP_EMS: f32 = 16.0;
const MIN_TEXT_EMS: f32 = 6.0;

impl Editor {
    /// `id` must be unique among editors shown at once.
    pub fn new(id: impl Into<Id>, fonts: Arc<Fonts>) -> Editor {
        Editor {
            tool: Tool::Interact,
            color: Color::RED,
            id: id.into(),
            fonts,
            doc_id: None,
            gesture: Gesture::Idle,
            typing: None,
            pending: Vec::new(),
            began: false,
            hover: Hit::None,
            pointer: None,
            last_press: None,
            textures: Default::default(),
        }
    }

    pub fn fonts(&self) -> &Arc<Fonts> {
        &self.fonts
    }

    /// The canvas area to show: the crop (or whole canvas), but the whole canvas while the
    /// crop tool is selected. Hosts fit their view to it.
    pub fn view_region(&self, canvas: [u32; 2], crop: Option<IRect>) -> IRect {
        match crop {
            Some(c) if self.tool != Tool::Crop => c,
            _ => IRect::of_size(canvas),
        }
    }

    /// Whether a drag or text edit is in progress.
    pub fn busy(&self) -> bool {
        !matches!(self.gesture, Gesture::Idle) || self.typing.is_some()
    }

    /// What a press at the last pointer position would grab (move tool).
    pub fn hovered(&self) -> Hit {
        self.hover
    }

    /// Drops all interaction state without committing (e.g. the host decided not to make the
    /// doc `begin` asked for). Switching docs needs no call: the editor notices by
    /// [`Doc::id`].
    pub fn reset(&mut self) {
        self.gesture = Gesture::Idle;
        self.typing = None;
        self.pending.clear();
        self.began = false;
        self.hover = Hit::None;
        self.doc_id = None;
    }

    /// Notes the doc shown now. State about another doc's elements is dropped; a doc
    /// arriving where there was none (after `begin`) keeps what waits for it.
    fn sync_doc(&mut self, id: Option<DocId>) {
        if id != self.doc_id && self.doc_id.is_some() {
            self.gesture = Gesture::Idle;
            self.typing = None;
            self.hover = Hit::None;
        }
        self.doc_id = id;
    }

    /// Commits text being typed (e.g. before sending), returning the edits applied.
    pub fn commit(&mut self, doc: &mut Doc) -> Vec<Edit> {
        self.sync_doc(Some(doc.id()));
        let mut out = CanvasOut::default();
        for el in self.pending.drain(..) {
            let e = doc.add_edit(el);
            doc.apply(&e);
            out.edits.push(e);
        }
        if let Some(t) = self.typing.take() {
            self.finish_typing(t, Some(doc), &mut out);
        }
        self.began = false;
        out.edits
    }

    /// Adds a picture (File > Insert image) centered at `center`, at its natural size or
    /// shrunk to fit in `fit` (canvas px, e.g. the visible region). Returns the edit, applied.
    pub fn insert(&mut self, doc: &mut Doc, kind: Picture, natural: Size, center: Pt, fit: IRect) -> Edit {
        let k = ((fit.w as f32 * 0.9) / natural.w.max(1.0)).min((fit.h as f32 * 0.9) / natural.h.max(1.0)).min(1.0);
        let (w, h) = ((natural.w * k).max(1.0).round(), (natural.h * k).max(1.0).round());
        let mut dst = Rect::new((center.x - w / 2.0).round(), (center.y - h / 2.0).round(), w, h);
        // Cascade off pictures already at that spot, so a second insert doesn't hide the first.
        let step = (20.0 * doc.body().scale).round();
        while doc.body().elements.iter().any(|e| matches!(&e.kind, Kind::Raster { dst: d, .. } | Kind::Svg { dst: d, .. } if d.min == dst.min)) {
            dst = dst.translate(pt(step, step));
        }
        let kind = match kind {
            Picture::Raster(asset) => Kind::Raster { asset, dst, src: None },
            Picture::Svg(asset) => Kind::Svg { asset, dst },
        };
        let e = doc.add_edit(Element::picture(kind));
        doc.apply(&e);
        e
    }

    /// Removes the element a move-tool press would grab (Delete / Backspace).
    pub fn delete_hovered(&mut self, doc: &mut Doc) -> Option<Edit> {
        self.sync_doc(Some(doc.id()));
        let key = match self.hover {
            Hit::Key(k, _) | Hit::Item(k) => k,
            Hit::None => return None,
        };
        if self.busy() {
            return None;
        }
        let e = doc.remove_edit(key)?;
        doc.apply(&e);
        self.hover = Hit::None;
        Some(e)
    }

    /// Picks a color; with no drawing tool selected, also picks the pen.
    pub fn pick_color(&mut self, c: Color) {
        self.color = c;
        if !self.tool.draws() {
            self.tool = Tool::Pen;
        }
    }
}

/// An inserted picture's asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    Raster(AssetId),
    Svg(AssetId),
}

#[cfg(test)]
mod tests;
