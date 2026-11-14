# "Renderings never become editable": two ways around it, and only convention guards the rest

Not blocking phase 6. Fix **before phase 7** (File → Insert image, and any paste/drop of image bytes, would widen the holes). Spec: `chat-image-flow.md` "Image model", `image-system-design.md` "Invariant".

## 1. SVGs can embed a rendering (real bypass)
`SvgDoc::parse` (asset.rs:138) uses usvg's default `image_href_resolver`. That loads `<image href="...">` files relative to `resources_dir` (the file's directory, or the agent's cwd for inline SVG, make.rs:154) and decodes `data:image/png;base64,...`. So an agent writing `svg '<svg ...><image href="/run/.../img/k3f9.png" width=.. height=../></svg>'`, or an SVG file doing the same, puts guilab's own rendering, with its marks baked in, into a new image as a picture. `load_file`'s render-mark check never sees those bytes. An agent trying to "attach the annotated version" is likely to do exactly this.
Fix: give usvg a custom `ImageHrefResolver` that runs `render_mark` on raster data (file or data URL) and refuses it. The resolver can't return an error, so record the refusal in a `Cell` and fail the parse afterwards with the same "that file is a rendering of image k3f9; use --from k3f9" wording. Test both the href-to-file and the data-URL forms.

## 2. `Raster::from_capture` takes any bytes
The types stop `Rendered → Raster` only by leaving out a conversion. But `Rendered::rgba()` hands out the pixels, and `Raster::from_capture(w, h, Vec<u8>, at_ms)` (asset.rs:82) is a public constructor that takes any pixels. A one-line bypass exists, and the only guard is a comment. The editor library also *ships* public callers: `guilab_editor::demo::fake_app` / `sample_raster` (demo.rs:211, 228) are in the public API.
Shape:
- Add `asset::load_bytes(data: &[u8], name: &str) -> Result<Asset, LoadError>` (`load_file` minus the read; same refusal), as the sanctioned entry for bytes without a path. Phase 7 paste/drop and test fixtures use it, so nobody reaches for `from_capture`.
- Rename `from_capture` to make it scary (`from_live_frame`) and keep its one caller in `server/src/images.rs`. Move the editor demo's synthetic screenshots to `load_bytes` of an in-memory PNG. Without the mark that's legal, but the server path is the better example.
- Enforce it: a cheap unit test in `guilab-image` that scans `crates/*/src` and fails if `from_capture`/`from_live_frame` appears outside an allowlist (`server/src/images.rs`, `image/src/tests.rs`). That turns "keep it that way in review" into a check.

## 3. Notes
Fix asset.rs:3-4 ("and nothing else") and the note's "Invariant" section to match.
