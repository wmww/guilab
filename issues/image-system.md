# Image system

Queued; not started. The UI side is in `chat-image-flow.md`. This file is the user's spec. **The design, phases and parallel plan are in `notes/image-system-design.md`**, and the tested agent doc text is in `notes/image-format-doc.md`. When built, move the parts that end up true into notes.

## Model
- **Raster**: immutable pixels with a random id. Made by screenshots (human or agent) and by inserted files. Shared by every image that uses it; that sharing is the link between related images.
- **Image**: immutable. Canvas size, optional background color, an optional crop (a viewport rect over the canvas, non-destructive, so it can be undone in the editor), and an ordered list of elements. It has a random short id (no content addressing; nothing needs it) and a `parent` (the image it was edited from, if any).
- **No special "base"**: rasters are just elements (raster id, placement rect, optional source crop). A screenshot is a canvas the size of the output with one raster element filling it. In the editor, the bottom raster of a screenshot is locked against accidental drags.
- **Elements**: pen, box, ellipse, line, arrow, text (our marks), raster image (from a raster id or a file: PNG/JPEG/…), and opaque SVG (from a file or inline text). SVG renders with resvg, and can be moved, resized and deleted as a unit, but not edited inside.
- Every edit (human or agent) makes a new image with `parent` set. Nothing is mutated, so sent messages always show exactly what was sent.
- Screenshots keep the full raster even when the agent asked for a crop. The crop lives on the image, so the human can uncrop. Expiry bounds memory.
- **No persistence and no compatibility promises.** Everything lives in server memory or the session dir and dies with the session/server. PNG renderings are the only files, and they're outputs only. SVG export (marks as paths) may come later.
- **Invariant**: the editor only opens images, never rendered files. A baked annotation can never become an editable element, except through out-of-band tools.

## Text format (one format both ways)
Agents read marks and write them in **exactly the same syntax**, so they can copy what they've seen and have no wrong assumptions about what exists. No SVG-like input. Grammar and examples: design note "Text format" (tested on 6 fresh agents).

## Agent CLI
- `guilab image ID [-o PATH] [--crop X,Y,W,H] [--no-marks] [--max WxH]`: render to PNG, print the path. **Pull, not push.** Deliveries and `poll` give only text (image ids, sizes, element lists, a fetch hint). A crop at full resolution gives more detail than a shrunk whole image, for less context. `--max` only shrinks.
- `guilab screenshot` returns an image id and takes the same `--crop`/`--max`.
- `guilab image make [--from ID] [--canvas WxH [--color C]] [--crop …] [--drop N…] [ELEMENT…]`: new image. `--from` copies ID's elements, `--drop` removes elements by number, and the given elements (same syntax as above, including `raster FILE …` and `svg FILE|'<svg…>' …`) are appended. Prints the new id and a preview PNG path, so the agent can check it before sending. An image it likes is sent by id: `guilab done "…" --image ID` (see `agent-status.md`).
- Agent screenshots and previews appear in the chat dimmed and smaller. Images sent on purpose look normal.
- Skill doc and `poll` say that a human drawing should normally be pulled, at least cropped to its marks.

## Human side
- Text is baked into renderings like every mark (the earlier "text not in image" decision is reversed). Consider a semi-transparent background behind text.
- File → Insert image (SVG or raster) adds an element that can be moved and resized.
- Check with a few fresh sub-agents that the element list helps and the format is easy to write.

## Code structure (decouplable)
Split so each part could be pulled into another app (an egui harness, a mobile image editor) without architectural pain; it doesn't have to be a polished library today:
- **image core** (no egui): model, raster store interface, text format parse/print, CPU renderer (resvg/tiny-skia).
- **image editor widget** (egui, no guilab): tools, key points, move/resize, undo; depends on core.
- **chat widget** (egui, no guilab): feed and composer, generic over attachments (a small trait for thumbnails and clicks), not tied to the image core.
- **toolkit** (egui widgets + theme): shared by the editor, chat and viewer, and tied to none of them.
- guilab glues it together: the server owns the image store (the CLI needs it), and the viewer uses it in-process.

## Open questions for the user
The design assumes these defaults; none blocks phases 1–3.
- **Deleting an element in the editor** (the spec says pictures can be "deleted as a unit", but there's no UI for it): Delete/Backspace removes the item under the pointer while the Move tool is selected.
- **Cropping in the editor** (the spec says crops are non-destructive and can be undone, but there's no UI for it): a Crop tool. Drag a rectangle; while the tool is selected, the whole canvas shows dimmed outside the crop, with a "Show all" button to uncrop.
