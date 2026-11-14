# Agent-built images, chat image display, SVG

Queued feature spec from the user. Builds on the image model in `chat-image-flow.md` / `image-pull.md`, and on `agent-status.md` (messages are sent via `work -m`/`done`).

## Agent images
The agent can reply with images made from the same image objects and marks the human edits:
- Base: a screenshot by id (including the human's, i.e. editing their drawing to ask a question), a solid-color canvas (`WxH`, color), or a file from disk (the agent downloads anything from the internet itself).
- Marks: the same set the human has (no new kinds for now), added from the CLI.
- Uses: pointing something out on a fresh screenshot, asking about the human's drawing before implementing it, a primitive mockup built from marks that the human then edits and sends back.
- The human opens and edits these like any other past image (implicit pending copy).
- Sketch of a CLI: `guilab image new --from ID | --canvas 800x600 [--color C] | --file PATH` prints a new id; marks are added with a terse text form or an SVG subset (see below); `done "…" --image ID`. Settle the details while building.

## Chat display
- Images sent on purpose (by either side) look normal. Screenshots the agent took for its own iteration also appear in the feed, but dimmed and smaller, like command lines. They're still clickable and editable.
- Several images in one message are laid out in a row, not stacked, for both sides.

## SVG (decided: SVG subset as an input format, not the model)
- **Source of truth stays our mark model.** Editing tools need semantic items (an arrow with two key points), and agents understand "red arrow 412,300 → 640,410" better than a path. SVG as the model would need metadata on every element plus a parse-back step, and editing arbitrary SVG means building half of Inkscape.
- **Agents may write marks as an SVG subset** they already know: `<rect>`, `<ellipse>`/`<circle>`, `<line>`, `<polyline>`/`<path>` (pen), `<text>`, and arrows as `<line marker-end=…>` or `data-kind="arrow"`. guilab parses it into marks. Anything outside the subset is an error that lists what's supported, unless baked as below.
- **Arbitrary SVG as one opaque element**: an agent can embed any SVG as a single mark, rendered with resvg (pure Rust, good quality). In the editor it can be moved, resized and deleted as a unit, not edited inside. This allows sophisticated mockups without a vector editor. The mark list describes it as e.g. `svg element 100,100 – 500,400 (from agent)`.
- **Messages to agents** keep the compact mark list from `annotation-marks.md`, not SVG.
- **Rendering**: since resvg comes in for embedded SVG, consider tiny-skia (its rasterizer) for the CPU export renderer too, if it beats our own on antialiasing/arrowheads/text. Keep egui painting for the live editor. Only switch if output quality improves; the screen and export must still match.
