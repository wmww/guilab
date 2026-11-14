# Image system design

The design for `issues/image-system.md`, `chat-image-flow.md`, `move-tool.md` and `chat-draw-gaps.md`, plus the shared toolkit from `ui-toolkit-pass.md`. Those issues hold the user's spec, and this note is how it gets built. When a phase lands, move what's true into `code-map.md`/`design.md` and trim this note.

**Status:** phases 1 (`crates/image`) and 3 (agent side: server store, CLI, protocol) are built and tested; the core, server and CLI sections below describe the code as it is. Phases 2, 4–7 not started.

## Principles
- **One object, many views.** An image is immutable data in a store. Thumbnails, the editor, PNG files and listings are all views of it. A pending image is a *draft*: a mutable body that only the viewer holds, frozen into an image when it's sent.
- **Data in, events out.** Widgets never own app data. The host passes in what to show and gets events back, and the host applies them. That way undo, copy-on-edit and animations are glue decisions, not widget hacks.
- **Renderings are a different type from rasters.** Nothing converts one into the other (see "Invariant").
- **One geometry source.** Core turns elements into a display list. The CPU renderer and the egui editor both paint that list, so arrowheads, text backings and line breaks can't drift apart.

## Crates

| crate | deps | role |
|---|---|---|
| `guilab-image` (`crates/image`) | tiny-skia, resvg/usvg, skrifa, png, zune-jpeg, epaint_default_fonts (font bytes only) | model, ids, store, text format, geometry, display list, CPU render, edit ops. **No egui.** |
| `guilab-kit` (`crates/kit`) | egui | theme tokens (light/dark, motion), animation helpers, `History<Op>`, shared widgets (menu bar, toast, icon button, close badge, send button, tab, thumb frame) |
| `guilab-editor` (`crates/editor`) | egui, kit, image | the image editor widget: toolbar, canvas, tools, move tool, key points, text editing, crop tool. Emits `Edit`s. |
| `guilab-chat` (`crates/chat`) | egui, kit | feed and composer, generic over attachment keys. Knows nothing about images. |
| `guilab-viewer` | + image, editor, chat, kit, proto | the glue: per-session workspace (drafts, open image, history, flights) |
| `guilab-server` | + image | owns the `Store`. Capture, render files, `image`/`image make`, freezing drafts, events |
| `guilab-proto` | unchanged deps | carries image ids and listing *text* (snapshots). It doesn't depend on `guilab-image`, so its tests stay cheap. |

Dependency edges (A → B = A uses B). There are no cycles, and nothing generic depends on guilab crates:
```
editor → image, kit        chat → kit
viewer → image, editor, chat, kit, proto
server → image, viewer, proto, compositor        guilab (CLI) → proto, server
```
`guilab-markup` is deleted at the end (phase 6). Reusability: editor + image + kit make a mobile image editor (the host supplies a `Store` and a pan/zoom view). Chat + kit make another agent harness's chat (the host supplies keys and thumbnails).

## guilab-image (core)

### Ids
- `ImageId`, `AssetId`: 4 chars from `23456789abcdefghjkmnpqrstuvwxyz`. Raster ids start with `r` and SVG ids with `s`. Image ids never start with `r` or `s`, so you can tell the kind from the id, and errors can say "r7qa is a raster, not an image".
- They're random (`IdGen`, seeded from `/dev/urandom`). The store keeps every id it has issued, so an expired id is **never reused**: a stale id is an error, never a different image. Since ids are never reused, a kind gets 5-char ids once half its 4-char space is issued (rasters: after ~15k).

### Model
```rust
pub struct Pt { pub x: f32, pub y: f32 }            // canvas pixels
pub struct Size { pub w: f32, pub h: f32 }
pub struct Rect { pub min: Pt, pub size: Size }
pub struct Color(pub [u8; 4]);                      // straight RGBA; PALETTE: red yellow green blue white black

pub struct Image { pub id: ImageId, pub parent: Option<ImageId>, pub body: Arc<Body>, pub made: Made }
pub enum Made { Screenshot { at_ms: u64, by_agent: bool }, Human, Agent }   // for listings ("your screenshot")
pub struct Body {
    pub canvas: [u32; 2],
    pub scale: f32,               // UI scale of the source screen; stroke = 3*scale, default font = 18*scale
    pub background: Option<Color>,
    pub crop: Option<IRect>,      // non-destructive viewport over the canvas (integer px, inside the canvas)
    pub elements: Vec<Element>,   // bottom to top; listing number = index + 1
}
pub struct Element { pub color: Color, pub kind: Kind }   // color is unused for pictures
pub enum Kind {
    Pen(Vec<Pt>), Box(Rect), Ellipse(Rect), Line(Pt, Pt), Arrow(Pt, Pt),   // arrow: tail, tip
    Text { at: Pt, size: f32, width: Option<f32>, text: String },          // at = top-left of the text
    Raster { asset: AssetId, dst: Rect, src: Option<IRect> },
    Svg { asset: AssetId, dst: Rect },
}
```
- A **screenshot** is `Body { canvas: output px, scale: output scale, elements: [Raster{asset, dst: whole canvas}] }`, with an optional crop.
- **Base lock** is a rule, not a flag: element 0, if it's a raster covering the whole canvas, can't be hit, moved or deleted in the editor.
- `width: None` means no wrapping (only explicit `\n`). The editor stores a width only when the typed text actually wrapped.
- Limits are checked on every insert: 10k elements, 100k pen points, canvas at most 16384 px per side.

### Assets and the store
```rust
pub enum Asset { Raster(Raster), Svg(Box<SvgDoc>) }
pub struct Raster { size: [u32; 2], px: Arc<[u8]>, origin: Origin }   // premultiplied RGBA8; private fields
pub struct SvgDoc { text: Arc<str>, size: Size, tree: usvg::Tree, origin: Origin }
pub enum Origin { Capture { at_ms: u64 }, File { name: String } }

pub trait Assets { fn asset(&self, id: AssetId) -> Option<Arc<Asset>>; }

pub struct Store { /* images, assets, issued ids, refcounts, bytes, last use */ }
impl Store {
    pub fn new(limits: Limits) -> Store;
    pub fn add_asset(&mut self, a: Asset, group: Group) -> AssetId;
    pub fn add_image(&mut self, parent: Option<ImageId>, body: Body, made: Made, group: Group,
                     carried: &[(AssetId, Arc<Asset>)]) -> Result<ImageId, StoreError>; // checks every asset ref; re-adds carried assets that were evicted
    pub fn image(&self, id: ImageId) -> Result<Arc<Image>, Lookup>;  // Lookup::{Unknown, Expired}; marks it used
    pub fn lookup(&self, s: &str) -> Result<Arc<Image>, Lookup>;     // + Lookup::Id(IdError) for malformed/other-kind ids
    pub fn get_asset(&self, id: AssetId) -> Result<Arc<Asset>, Lookup>; pub fn status(&self, id: ImageId) -> Result<(), Lookup>;
    pub fn parent_note(&self, img: &Image) -> Option<&'static str>;  // "your screenshot" | "screenshot" | "expired"
    pub fn hold(&mut self, id: ImageId); pub fn release(&mut self, id: ImageId);
    pub fn sweep(&mut self, now: Instant) -> usize;                  // expiry + budget, returns bytes freed
    pub fn drop_group(&mut self, g: Group);                          // session end
}
impl Assets for Store {}
```
- `Group` is an opaque u64. The server uses one per session, so one server-wide store has one id space and one memory budget, and session end drops a group.
- Assets are refcounted by the live images that use them. An asset no image uses (added, then the image failed) goes at the next `sweep`. A raster is shared by every image built on it, and that sharing is the link between related images.
- **Expiry** (`Limits`, all tunable): an image that isn't held expires after `idle` (30 min) without use. When decoded asset bytes exceed `budget` (1 GiB server-wide), unheld images go first, least recently used first. Held images go only when held images alone exceed the budget, oldest first. A `parent` link doesn't keep the parent alive: listings say `parent q81c (expired)`.
- **Holds**: the server holds every image referenced by a message event (human or agent) until its session ends. Drafts aren't in the store. A draft carries `Arc<Asset>`s for what it uses, so eviction can't break it, and `add_image(.., carried)` re-adds them on send.
- Limits on pixels: rasters from files at most 16384 px per side and `MAX_PIXELS` (64 Mpx); renders over 64 Mpx are shrunk automatically.
- Rasters are kept decoded. If memory becomes a problem, cold rasters can be QOI-compressed behind the same `Assets` trait without changing any API.

### Invariant: renderings never become editable
- `render()` returns `Rendered`, which can be read as pixels, encoded as PNG or uploaded as a texture. There's no `Rendered → Raster`.
- Only two functions create a `Raster`. `Raster::from_capture(w, h, rgba, at_ms)` is called from exactly one place: the server's capture of a live session frame (keep it that way in review). `asset::load_file(path)` is the other.
- Every PNG that guilab writes from a rendering (the `image`/`screenshot`/`make` outputs, the viewer's File → Save) gets a `tEXt` chunk `guilab-render=<image id>`. `Rendered::png(Option<ImageId>)` is the only PNG encoder and always marks (`draft` without an id), so there's no unmarked path. `load_file` refuses such files (chunk found by a raw chunk scan) with `LoadError::Rendering(id)`, and the CLI says "that file is a rendering of image k3f9; use --from k3f9". Tools outside guilab that strip the chunk (GIMP, ImageMagick) are the allowed way around it, as the spec says.
- The editor only opens `Body`s built from store images or drafts. Thumbnails and the main view are always `Rendered`/textures, never inputs.

### Geometry, text, display list
- `text::Fonts` holds egui's default proportional fonts (Ubuntu-Light, then NotoEmoji and emoji-icon-font as fallbacks; `text::FONT_DATA`), from `epaint_default_fonts`. That's the same bytes the editor's egui uses. `Fonts::layout(text, size, width) -> TextLayout { lines: Vec<Line { text, y, w, glyphs }>, font_size, ascent, line_height, size }` measures with skrifa advances (no kerning). **Core decides line breaks** (greedy at spaces, inside over-long words), and each backend draws the given lines unwrapped, so wrapping is identical everywhere. The CPU renderer fills the glyph outlines (`Fonts::outline`) at exactly the measured positions; egui may differ by kerning within a line.
- `geom` on `Element`, all in canvas px. Most take the body's `scale` (stroke width and text padding depend on it) and `&Fonts`:
  - `bounds(scale, fonts)`: strokes include half their width (arrows their head), text its backing
  - `frame(scale, fonts)`: the rect key points sit on and `fit_to` maps (no stroke width; text = its backing)
  - `hit(p, tol, scale, fonts)`: within `tol` + half the stroke of a stroke (boxes on their outline only); inside the backing for text; inside the rect for pictures
  - `key_points(scale, fonts) -> Vec<KeyPoint { key: Key::{Corner(0..4 from top-left, clockwise), End(0|1)}, at }>`: frame corners for box, ellipse, text, pen and pictures; ends for line and arrow
  - `drag_key(k, to, Mods { shift }, scale, fonts) -> Element`: the move tool's reshape rules (below)
  - `translated(d)`
  - `fit_to(rect, scale, fonts)`: pen points scaled into the rect (each axis); text scales `size` and `width` by the height ratio, at the rect's top-left
- `draw::display_list(&Body, &Fonts, marks: bool) -> Vec<(usize, Prim)>`, where the usize is the element index:
  ```rust
  pub enum Prim {
      Raster { asset: AssetId, dst: Rect, src: Option<IRect> },
      Svg { asset: AssetId, dst: Rect },
      Stroke { pts: Vec<Pt>, closed: bool, width: f32, color: Color },
      Fill { poly: Vec<Pt>, color: Color },                    // arrowheads
      Ellipse { rect: Rect, width: f32, color: Color },
      RoundRect { rect: Rect, radius: f32, fill: Color },      // text backing
      Text { layout: Arc<TextLayout>, at: Pt, color: Color },   // size is layout.font_size
  }
  ```
  Open strokes have round caps and joins, closed ones (boxes) mitred corners, and a one-point pen is a dot. Arrowhead: `draw::arrow_head` (16×scale, at most 0.6 of the length; the shaft stops halfway into the head).
  The text backing is translucent black, or translucent white behind very dark text (as today). The constants live in core because they're part of how the image looks, not UI theme.
- `render::render(&Body, &dyn Assets, &Fonts, &RenderOpts) -> Result<Rendered, MissingAsset>`, where `RenderOpts { region: Region::{ImageCrop, Canvas, Rect(IRect)}, max: Option<[u32; 2]>, marks: bool }`. `RenderError::{MissingAsset, Outside}`; `Region::Rect` is clipped to the canvas (error if it misses it). It uses tiny-skia: rasters via `draw_pixmap` (bicubic, nearest for whole-pixel 1:1 placement so screenshots are exact, with a box prefilter when shrinking more than 2x), SVG via resvg, text as glyph outlines from the bundled fonts (deterministic), and marks via paths. `Rendered { pixmap (private), shows: IRect, zoom: f32 }` with `size()`, `premultiplied()`, `rgba()`, `pixel()`. `shows`/`zoom` give the canvas mapping the CLI prints. `Rendered::png(Option<ImageId>)` adds the render mark.
- `svg_pixmap(&SvgDoc, [w, h]) -> Rendered`: for editor textures. The SVG's own `<text>` uses the system fonts (mockups need real fonts): usvg converts text when parsing, so `SvgDoc::parse` loads them once (process-wide `OnceLock`) for SVGs that contain `<text`.

### Edits (`guilab_image::edit`)
```rust
pub struct ElemKey(u64);                    // stable while editing, unlike indices
pub struct Doc { body: Body, keys: Vec<ElemKey>, next: u64, rev: u64 }   // rev++ on every apply/revert
pub enum Edit {
    Add { key: ElemKey, at: usize, el: Element },
    Remove { key: ElemKey, at: usize, el: Element },
    Replace { key: ElemKey, old: Element, new: Element },
    Crop { old: Option<IRect>, new: Option<IRect> },
}
impl Doc {
    pub fn from_body(b: Body) -> Doc; pub fn body(&self) -> &Body;
    pub fn apply(&mut self, e: &Edit); pub fn revert(&mut self, e: &Edit);
    pub fn set_base(&mut self, el: Element);   // inserts the capture raster at index 0; not an edit (see the viewer flow)
    // builders: add_edit(el) (on top), remove_edit(key), replace_edit(key, new), crop_edit(new); keys(), index_of(key), element(key), new_key()
}
```
Drags produce one `Replace` per drag, at release. The editor shows the in-progress state from its own interaction state, so history doesn't fill up with per-frame edits.

## Text format
Agents read and write the same lines. Tested on 6 fresh opus agents over two rounds. Each got a docs excerpt, a `poll` message with a listing, and 7–8 tasks: read the marks, render crops, replace, drop, scale a pen, build from scratch with raster `from`, SVG file and inline SVG, multi-line text, uncrop, stdin. All 6 answered every task correctly, and their answers were nearly identical. Their doubts are resolved here:
- Round 1 → round 2: one crop syntax everywhere, `scale` explained, a text box comment, the inline SVG rule, what `--from` keeps, `--drop` as one comma list.
- Round 2, now stated in the docs: an explicit `size` is absolute pixels (only the default scales); `raster … from R` defaults to R's size; raster pixels map 1:1 to canvas pixels at the default size; later elements draw on top; stdin lines go where `-` stands and may be numbered; a pen's rect form scales each axis separately and only works when replacing; dropping and replacing the same number is an error.
- The skill doc's "Images" section (`skill/SKILL.md`) is that tested text, with these answers worked in. Keep its wording when editing: each sentence answers a question a tester asked. `doc_examples_parse` parses its listing lines.

### Listing
```
image w8dn 1600x1000 scale 2, parent h2kc (your screenshot), crop 200,100 1200x700
  1 raster rq4m 0,0 1600x1000 (screenshot 14:04:50)
  2 blue box 260,180 520x90
  3 blue line 800,270 to 800,300
  4 blue text 820,262 "30px gap, should be 16" (box 812,254 391x52)
  5 red ellipse 1010,610 300x110
  6 green pen 1180,380 140x260 (48 points)
  7 raster r8kd 40,40 240x120 from 600,380 120x60 (from mock.png)
  8 svg s3pa 600,40 64x64
```
- Header: `image ID WxH`, then `scale S` if S isn't 1, `, parent ID` with a note (`(your screenshot)`, `(expired)`), `, background C`, and `, crop X,Y WxH`.
- Options at their default are left out (text `size` at 18×scale, `width` none). Numbers print as integers when whole, else with one decimal.
- A trailing `(...)` is derived information and is ignored on input: a text's box (backing included), a pen's point count, a raster's origin. `--points` prints pen points instead of the bounds.

### Grammar (input; output is the canonical subset)
```
line     = [NUM] [color] mark | [NUM] picture   , then optional trailing "(" ... ")"
mark     = "box" rect | "ellipse" rect | "line" pt "to" pt | "arrow" pt "to" pt
         | "pen" (pt pt {pt} | rect)            ; rect form only when replacing a pen (NUM)
         | "text" pt {"size" NUM | "width" NUM} string
picture  = "raster" (RASTER_ID | PATH) pt [size] ["from" rect]
         | "svg" (SVG_ID | PATH | INLINE) pt [size]
color    = red|yellow|green|blue|white|black | "#" 6 hex digits      ; default red
pt       = NUM "," NUM            size = NUM "x" NUM          rect = pt size
NUM      = -?digits(.digits)?
string   = '"' { char | \" | \\ | \n } '"'
PATH     = token containing "/" or a ".ext"
INLINE   = "<svg" ... matching "</svg>"   (spaces and quotes allowed; xmlns added if missing)
```
- Tokens are split on whitespace. Strings and INLINE are single tokens. Keywords and colors are case-insensitive. Also accepted as input: `->` or `→` for `to`, and `X,Y,W,H` for a rect (common slips, unambiguous).
- Errors quote the line (shortened to 60 chars), name the token and show the expected form, e.g. `in 'red box 600,380 100,60': "100,60" isn't a size; expected WxH like 100x60 (box X,Y WxH)`. Quoting beats `element N` because numbered lines (`5 red box …`) made "element N" ambiguous.
- A pen is printed in rect form only in listings without `--points`; that line copied back as a numbered replacement leaves the pen unchanged. `print → parse → print` is identical with `--points` (tested over every kind).
- In code (`format`): `listing`/`store_listing`/`header`/`element_line` print; `parse_lines(text) -> Vec<Line { num, spec: Spec, src }>` parses (one string may hold several lines; strings and inline SVG may span lines); `parse_crop`, `parse_color`. `Spec` keeps sources (`Source::{Id, Path, Inline}`) and defaults unresolved; `make` resolves them.

### `image make` semantics
1. Start from a copy of `--from` (canvas, scale, background, crop, elements; parent = from). Otherwise start empty with scale 1.
2. Numbered lines replace element NUM of `--from` in place. The rect form of a pen means `fit_to(rect)` of the old stroke, each axis scaled on its own. A number without `--from`, one out of range, one given twice, or one also in `--drop` is an error.
3. `--drop N,N` removes elements. Numbers always refer to the `--from` listing.
4. Unnumbered lines are appended in order. `-` inserts lines read from stdin at that position, skipping blank lines. There are no comment lines, since a line may start with a `#rrggbb` color. The trailing `(...)` is stripped only outside quotes and inline SVG.
5. `--crop 'X,Y WxH'|none` and `--color C|none` replace those fields. `--canvas WxH` sets the canvas (default without `--from`: the bounding box of the elements from 0,0).
6. Files are read once, here. They become assets, and the listing shows ids with `(from NAME)`. A file that is a guilab rendering is refused. Inline SVG becomes an asset with no note.

In code: `make::make(&mut Store, &Fonts, &MakeReq { from, canvas, color, crop, drop, elements, cwd }, Made, Group) -> Result<ImageId, String>`. All strings arrive unparsed; errors are agent-ready sentences.

## guilab-kit
- `Theme` (`Arc`, stored in egui ctx data: `Theme::get(ctx)`, `Theme::install(ctx, Mode)` also sets egui `Visuals`/`Style` from the tokens):
  - `colors`: panel, view_bg, text, weak, strong, accent, danger, outline, hover_fill, selected_fill, glow, key_point, key_point_hot, hover_outline, me, agent
  - `space`
  - `size`: toolbar_w, tool_button, thumb_h (composer and feed), faint_scale, hit_slop, key_point_r, key_point_hit_r, close_badge
  - `radius`, `font`
  - `motion`: whoosh 220 ms ease-out-cubic, slot_grow 160 ms, fade 200 ms, hover_grow ×1.15 over 100 ms, hover_delay 0
  
  `Theme::dark()`/`light()`. Following the portal and the View menu choice is viewer work (`ui-toolkit-pass.md`).
- `anim`: `Easing`, and `tween(ctx, id, target, Motion) -> f32` on top of `ctx.animate_value_with_time`. `RectTween` lerps between two rects that are re-resolved every frame.
- `History<Op>`: `push(op)`, `begin_group()/end_group()`, `undo() -> Option<Op>`/`redo() -> Option<Op>` (the caller applies or reverts), `retain(|op| ..)` (to drop ops for sent drafts), and a limit of 500.
- Widgets moved here from markup/viewer: `menu` (the menu bar, unchanged API), `toast`, `send_button`, `session tab`, plus new `close_badge` (the × circle) and `thumb_frame` (rounded image with hover and outline states). All values come from `Theme`.

## guilab-editor
```rust
pub enum Tool { Interact, Move, Pen, Box, Ellipse, Line, Arrow, Text, Crop }
pub struct Editor { pub tool: Tool, pub color: Color, /* interaction state, texture cache, Fonts */ }
pub struct Canvas<'a> {
    pub doc: Option<&'a mut Doc>,     // None = live view: a drawing tool press asks for a doc
    pub assets: &'a dyn Assets,
    pub xf: Xf,                       // canvas px -> screen points (host owns zoom/pan)
    pub clip: Rect,                   // screen area the canvas owns
    pub new_doc: ([u32; 2], f32),     // canvas size and scale a new doc would have (live view)
    pub visible: bool,                // false while a flight shows the image instead
}
pub struct CanvasOut {
    pub edits: Vec<Edit>,             // already applied to doc; the host records them
    pub begin: bool,                  // a drawing press with doc None: create a doc (capture) now
    pub pan: egui::Vec2,              // move tool dragging empty space
    pub cursor: Option<egui::CursorIcon>,
    pub wants_keys: bool,             // text editing, or a doc is open
}
impl Editor {
    pub fn new(fonts: Arc<Fonts>) -> Editor;
    pub fn toolbar(&mut self, ui: &mut Ui) -> Option<Tool>;          // Some = the tool changed
    pub fn canvas(&mut self, ui: &mut Ui, c: Canvas) -> CanvasOut;
    pub fn insert(&mut self, doc: &mut Doc, asset: AssetId, natural: Size, center: Pt) -> Edit; // File > Insert image
    pub fn delete_hovered(&mut self, doc: &mut Doc) -> Option<Edit>; // Delete/Backspace with Move (see open questions)
}
```
- It paints `display_list(doc)`. Rasters are textures cached per `AssetId` (immutable, so the cache is exact). SVGs are textures from `svg_pixmap` at the on-screen size, rounded up to √2 steps. Marks are egui shapes, crisp at any zoom. Text uses an egui font family `"image-text"` registered with the same bytes as core.
- A drawing press with no doc keeps the in-progress mark in the editor's own state and returns `begin`. The host creates the doc this frame (or next), and the mark is committed into it as `Edit::Add` at release. A tiny mark (accidental click) commits nothing, and the host then drops the doc (see the viewer flow).
- Undo/redo keys are the host's (Edit menu, Ctrl+Z), since history is the host's. Text editing is in place: double-click a text with Move or Text, and committing makes one `Replace`.
- Crop tool: drag a rect to get `Edit::Crop`. While it's selected, the whole canvas shows, dimmed outside the crop, and the toolbar shows "Show all" (crop none).

### Move tool (spec: `move-tool.md`)
On press, the first match wins:
1. **Key point** (hit radius `key_point_hit_r` < drawn radius) → reshape:
   - box, ellipse and pictures: corner resize. Pictures keep their aspect ratio unless Shift is held; marks keep it only with Shift.
   - text: scale `size` and `width` by the height ratio, anchored at the opposite corner.
   - pen: scale the points into the new bbox.
   - line/arrow: move that end.
   
   Corners can't cross the opposite one (min 4 px).
2. **Item or near it** (`hit_slop` screen px + half stroke; topmost first, i.e. last in the list) → move it. The locked base is never hit.
3. **Otherwise** (empty canvas, outside it, or the live view with no doc) → `pan`.

Key points are drawn as small circles only while Move is selected, and a hovered one lights up (`key_point_hot`). The item a drag would move gets `hover_outline` (a glow along its bounds). Hover and drag use the same hit order. Cursor: `Move` over an item, `Grab`/`Grabbing` for pan, resize cursors on key points, and never the arrow over the app (`viewer-cursors.md`).

## guilab-chat
```rust
pub struct Chat<K: Clone + Eq + Hash> { /* items, draft text, scroll, per-slot animation state */ }
pub struct Item<K> { pub author: Author, pub time: String, pub text: String, pub attachments: Vec<K>, pub faint: bool }
pub enum Author { Me, Them { name: String }, Activity }        // colors from Theme (me / agent)
pub enum SlotId { Feed { item: u64, index: u16 }, Composer }   // identity = (SlotId, K); composer slots follow their key when others are removed
pub enum SlotState { Shown, Away { outlined: bool } }           // Away = empty slot (its image is in the editor or in flight)
pub struct Thumb { pub texture: Option<TextureId>, pub aspect: f32 }   // None = placeholder (e.g. expired)
pub trait Thumbs<K> {
    fn thumb(&mut self, ctx: &Context, key: &K, height_px: f32) -> Thumb;
    fn state(&self, slot: SlotId, key: &K) -> SlotState;
}
pub enum ChatEvent<K> { Open { slot: SlotId, key: K, rect: Rect }, Remove { key: K }, Send { text: String } }
pub struct ChatOut<K> { pub events: Vec<ChatEvent<K>>, pub slots: Vec<(SlotId, K, Rect)> }   // every visible slot's rect this frame
impl<K> Chat<K> {
    pub fn push(&mut self, item: Item<K>) -> u64;                   // item id
    pub fn show(&mut self, ui: &mut Ui, pending: &[K], thumbs: &mut dyn Thumbs<K>) -> ChatOut<K>;
}
```
- The composer draws `pending` thumbnails **inside the text box, above the text**, at feed size, each with a `close_badge`. Send is enabled when the text isn't empty or `pending` isn't. Send clears the text and emits `Send`. The host takes the pending list itself.
- Sent items lay out the same way: images above the text. `faint` items (agent screenshots and previews) are dimmed and drawn at `faint_scale`.
- Inside the widget, keyed by `(SlotId, K)` with `Theme.motion`: a new composer key grows in, a removed one shrinks out, `Away ↔ Shown` cross-fades (a placeholder outline, then the image), and hover grows the thumbnail in a foreground layer without relayout. Clicking emits `Open` with the rect.
- The text field keeps its own undo while focused. Ctrl+Z elsewhere is the host's.

## Viewer glue (`crates/viewer`)
`ui.rs` splits into `ui/mod.rs` (tabs, menus, panels), `ui/view.rs` (session view: zoom/pan, input forwarding; viewer-sizing's code), `ui/workspace.rs` (below), `ui/flight.rs` (whooshes and the glow) and `ui/thumbs.rs` (texture cache: renders by image id, renders by draft key and revision, 2× display size).

```rust
struct Workspace {                    // per session (PerSession)
    editor: Editor, chat: Chat<Att>,
    pending: Vec<Draft>,              // composer order
    open: Open,
    history: History<Op>,
    flights: Vec<Flight>,
    doc_zoom: Zoom,                   // the open image's own zoom/pan (fit to its crop); never locked to the window
}
struct Draft { key: DraftKey, doc: Doc, parent: Option<ImageId>, carried: Vec<(AssetId, Arc<Asset>)> }
enum Att { Draft(DraftKey), Image(ImageId) }
enum Open { Live, Capturing { ticket: u64, key: DraftKey }, Draft(DraftKey), Past { image: ImageId, slot: SlotId, doc: Doc } }
enum Op { Edit(DraftKey, Edit), Create(Draft /* as created */), Remove { at: usize, draft: Draft }, Copy { from: ImageId, slot: SlotId, key: DraftKey } }
```
Store access: `Viewer::draw(renderer, sessions, store: &Store, now)`. Reads only, and thumbnails are rendered from `Arc<Image>` and assets.

### Flows (`chat-image-flow.md`)
- **Draw on the live app**: `CanvasOut.begin` → push `Action::Capture { session, ticket }` and set `Open::Capturing`. The session view keeps painting the live texture under the editor, so nothing changes visibly: no flash, no extra frame. The server answers `Viewer::captured(session, ticket, image)`. The glue makes a `Draft` from that screenshot (`parent` = it) via `Doc::set_base`. When the first edit commits, `Create` + `Edit` go in one history group, the slot is pushed into `pending` (its grow-in animation plays), and the state is `Away { outlined }`. If no edit commits (a tiny click), the draft is dropped and no slot appears.
- **Open an image** (`ChatEvent::Open`): a draft → `Open::Draft`. A past image → `Open::Past` with a scratch `Doc` (the store image itself is never touched). Either way a flight goes from the slot to the view, and the slot is `Away`.
- **Copy-on-edit**: the first `CanvasOut.edits` while `Open::Past`. The edit has already been applied to the scratch doc, so the glue moves that doc into a new `Draft` (`parent` = the past image). `Copy` + `Edit` go in one group. The feed slot goes back to `Shown` (the widget cross-fades it), and the new composer slot grows in as `Away { outlined }`. There's no flight. Undoing that group removes the draft and returns to `Open::Past` on a fresh doc.
- **Switch images**: one flight from the view back to the old slot and one from the new slot to the view, at the same time. **Close** (Interact tool selected, or a click on the open image's own slot) → flight back to the slot, then `Open::Live`.
- **Remove** (`ChatEvent::Remove`): `Op::Remove` (undo puts it back at its index). If the image is open, the editor closes without a flight. Removal never asks for confirmation.
- **Undo/redo** (Edit menu, Ctrl+Z while no text field has focus): apply or revert the op. If it changes a draft that isn't open, that draft opens first (with a flight), so a change never happens out of sight.
- **Send** (`ChatEvent::Send`): `Action::Message { session, text, drafts }`. The server freezes each draft (`Store::add_image(.., carried)`) and logs the message with `ImageRef`s. The glue clears `pending` and `history.retain`s ops of other drafts. The feed item arrives from the log the next tick, and its slots show the frozen images.
- The Queue/Discard bar and `Sketch` go away. "Hiding the viewer drops unsent state" goes away too: the server keeps the `ViewerUi` (no GPU state) across hide/show and drops only `Gfx`/`HostState`, and texture caches rebuild lazily.

### Flights and glow (`ui/flight.rs`)
```rust
struct Flight { att: Att, from: End, to: End, t0: f64, motion: Motion }
enum End { Slot(SlotId), View }   // resolved every frame: ChatOut.slots, or the doc's on-screen crop rect
```
- The chat panel runs before the central panel, and flights paint last in an `Order::Foreground` area. So both ends are this frame's rects, and a flight follows layout changes and pans mid-air. The flying texture is the image's thumbnail texture, upgraded to the full one when it's bigger than the thumbnail.
- While a flight touches the view, `Canvas.visible = false`. While it touches a slot, that slot is `Away`. When it lands, the view or slot takes over in the same frame, so nothing pops.
- **Glow**: while an image is open and nothing is flying, soft rays (a feathered quad, `colors.glow`) go from its `Away` slot rect to the doc's on-screen rect. Both rects are re-resolved every frame, so the glow tracks pan and zoom. It's hidden when the slot is scrolled out of view (then the slot's edge glows instead).
- Timing and easing come only from `Theme.motion`.

### Input routing
When a doc is open or a drawing tool is selected, the editor owns primary-button pointer input over the view. The host applies `CanvasOut.pan` to the doc's zoom, or to the session's zoom on the live view. The host keeps middle-drag, Ctrl+scroll and drags outside the image. `keys_to_egui` = `CanvasOut.wants_keys` || a menu is open. `Sketch::mark_at`, which the view called back into, goes away.

## Server glue (`server/src/images.rs`, built)
- `Server.images: Images { store, fonts, last_capture }`, one `Group` per session (`Slot::group`). The tick sweeps once a second; `end_session` drops the group.
- `capture(session, group, cursor, by_agent, crop) -> ImageId`: `Session::capture` (opaque RGBA, cursor drawn in if asked), `Raster::from_capture`, a screenshot body. While the session's `frame_seq` and size are unchanged (and no cursor), the last raster is reused. Agent `screenshot`, the viewer's `Freeze`, `debug message --draw` and the message `screen` all use it.
- `render_file(img, opts, dir, out)`: `s/<session>/img/<id>[-c<x>_<y>_<w>x<h>][-m<w>x<h>][-nomarks].png` (render mark included), reused if it exists; `-o PATH` always writes.
- `make` (core `make`, `Made::Agent`), `attach(spec)` (an id, or an absolute path: `load_file` → one picture element at natural size; a rendering is refused with "attach the image itself: --image ID"), `drawing(base, marks)` (viewer marks on a copy of the frozen base, `Made::Human`, parent = base), `image_ref` (listing snapshot + marks bbox).
- Every image a message (either way) references is held until the session ends, and so is each `Freeze` base (the drawing may be sent much later; a discarded one stays held until session end, which is cheap thanks to raster reuse).
- Viewer glue until phase 6: `Freeze` captures the base into the store and passes its id as the markup sketch's tag; `Annotated` carries `tag`, `marks`, `scale`; on send the server converts each markup `Mark` to an `Element` (coordinates rounded to whole pixels; text `width` kept only if the core layout wraps there) and ignores the markup composites. The feed renders message images from the store (at most 1600 px). Faint agent screenshots in the feed are phase 7; `image make` shows as a gray log line.

## CLI and protocol (built)
```rust
// proto (PROTO 3)
pub struct ImageRef { pub id: String, pub size: [u32; 2], pub listing: String, pub marks: Option<[i32; 4]> }  // fetch(), elements()
Command::Screenshot { path, settle, cursor, crop: Option<String>, max: Option<[u32; 2]> }   // Access::Read
Command::Image { id, path, crop /* "X,Y WxH" | "none" */, max, marks, list, points }      // Access::Read
Command::ImageMake { from, canvas, color, crop, drop: Vec<u32>, elements: Vec<String>, cwd }  // Access::Read; not logged as a command (ImageMade is)
Command::Work/Done { .., images: Vec<String> }      // ids, or absolute paths (the CLI absolutizes anything with / or . or an existing file)
HumanOp::Message { text, images: Vec<String>, draw: Vec<String> }   // draw: element lines on a capture taken now
// replies
ShotInfo { image, header, path, width, height /* PNG */, shows: [i32; 4], zoom, settled, animating, cropped }
ImageInfo { id, listing: Option<String>, render: Option<RenderInfo { path, width, height, shows, zoom }> }   // RenderInfo::mapping() = the "PNG shows" line
// events
EventData::Message { text, images: Vec<ImageRef>, screen: Option<String> }   // screen: capture with pointer, only when there are no images
EventData::AgentMessage { text, images: Vec<ImageRef> }
EventData::Screenshot { image: String, path }       // log tier
EventData::ImageMade { image: String }              // log tier, kind image_made (group agent)
```
Crop strings and element lines go to the server unparsed; core parses them. The CLI only reads stdin for `-` (once, reused for every `-`).

Output (stdout; `--json` gives the structs):
```
$ guilab screenshot --crop '100,50 200x100' --max 100x100
/run/…/s/demo/img/k3f9-m100x100.png
image k3f9 400x300 scale 2, crop 100,50 200x100
PNG shows canvas 100,50 200x100 at 0.5x, so PNG pixel x,y is canvas 100 + x/0.5, 50 + y/0.5

$ guilab image make --from w8dn 'yellow arrow 900,200 to 260,180'
image p4n2 1600x1000 scale 2, parent w8dn
  1 raster rq4m 0,0 1600x1000 (screenshot 14:04:50)
  …
preview: /run/…/img/p4n2.png
PNG shows canvas 0,0 1600x1000 at 1x
send it: guilab done "..." --image p4n2
```
- The "PNG shows" line is omitted for a screenshot that's the whole canvas at 1x; `image` always prints it.
- A human message in `poll` and deliveries: the message line, each image's listing indented, `(see what the marks point at: guilab image ID --crop '…')` (or `(see it: guilab image ID)` without marks), then for image-less messages `(their screen as they sent it, with their pointer: image ID; see it: guilab image ID)`, then the `guilab done` hint. One-line forms (`describe`, `--follow`, `log`): `[image w8dn: 6 elements]`, `[their screen: image ID]`, agent messages `[image ID]`.
- The fetch hint is the bbox of every non-base element (text boxes included, even outside the crop) plus 20 px, clamped to the canvas.

## Interface points with work in flight
- **viewer-cursors**: the editor reports `CanvasOut.cursor`, and the session view applies it over the image, so a tool cursor, never the arrow, shows while a tool takes input.
- **viewer-sizing**: the open doc has its own `doc_zoom` (fit to its crop) and ignores "lock size to window". Resize handles show on the live view only. Flights use the doc rect from that zoom.
- **ui-toolkit-pass**: phase 2 creates `guilab-kit` and moves existing widgets into it. The full sweep of hardcoded values and the portal light/dark wiring stay in that issue, done against kit.

## Phases
Each phase leaves main green (`cargo test --workspace`, clippy) and working end to end.

| # | phase | needs | touches |
|---|---|---|---|
| 1 | **(done)** **Core**: `guilab-image` with model, ids, store and expiry, format print/parse/patch, geometry, display list, renderer, edit ops, load_file and the render mark. Unit tests: round-trip of every listing line, the doc examples parse, patch rules, hit and key-point order, pixel tests (box edge, glyphs drawn, SVG drawn, crop/max mapping), refusal of a marked PNG, budget and expiry. No consumers yet. | none | new crate, root Cargo.toml |
| 2 | **Kit**: `guilab-kit` with Theme (tokens for everything moved), anim, History; move menu/toast/send button/tab; viewer and markup use it. | viewer-polish landed | kit, viewer, markup |
| 3 | **(done)** **Agent side**: store in the server, capture, `screenshot` → image, `image`, `image make`, `done`/`work --image ID\|PATH`, `ImageRef` in message/agent events, feed renders from the store, skill doc "Images", `tests/it/images.rs`. Also (beyond the original plan): human messages already go through the store (the current viewer's marks are rebuilt as elements on the frozen base, so nothing baked enters it), `Annotation` is gone, `debug message --draw`, and the `screen` id on image-less messages. Tested end to end on 2 fresh agents. | 1 | proto, server, guilab, viewer, markup, skill |
| 4 | **Editor**: `guilab-editor` with all tools, the move tool spec, key points and hover, text editing, crop tool, insert, edits out. Tests with a bare `egui::Context` (like markup's). | 1, 2 | new crate |
| 5 | **Chat**: `guilab-chat` with the generic feed, composer with slots in the box, × remove, send rules, slot states and animations, hover grow, slot rects. Bare-context tests. | 2 | new crate |
| 6 | **Integration**: Workspace (drafts, Open, History incl. composer ops, copy-on-edit), Capture/captured (replaces `Freeze` + sketch tag), Message with drafts → freeze (replaces the markup-mark conversion in `server/src/viewer.rs`), the old `Sketch`/`Chat`/`guilab-markup` deleted, keep `ViewerUi` across hide/show. Transitions are instant (empty slots and outlines, no flights). Update `viewer.rs` tests (`draw_and_send` already checks the listing) and the skill doc. | 3, 4, 5 | viewer, server, skill |
| 7 | **Motion and polish**: flights, glow, slot fades, feed hover grow, faint agent images, File → Insert image, dogfood through guilab itself (screenshots and input). | 6 | viewer (+ chat/kit tweaks) |

**Parallelism** (at most 3 worktrees): P2 once viewer-polish lands; then **P4 ∥ P5** (new crates, no overlap). **P6** alone (it touches everything). **P7** alone (viewer UI). `ui-toolkit-pass` comes after P7 or alongside P4/P5 if it stays out of `viewer/src/ui*`.

## Open questions
See the end of the issue (`image-system.md`). Defaults are used unless the user decides otherwise.
