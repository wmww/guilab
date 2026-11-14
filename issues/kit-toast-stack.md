# Kit toasts: stack automatically instead of by hand-picked slot numbers

**Land before phase 6** (small; otherwise the viewer has to know the editor's internals).

`guilab_kit::toast(ui, id, clip, slot, ..)` (kit/src/widgets.rs:255) places a toast at a slot number the caller picks. Every caller showing toasts over the same area has to know every other caller's toasts. The viewer already does this by hand: `slot = usize::from(p.sketch.drawing_open())` (viewer/src/ui.rs:471) because markup's bar takes slot 0. The editor's crop toast also hardcodes slot 0 (editor/src/canvas.rs:359). In phase 6 the viewer would have to copy the editor's condition (`tool == Crop && doc open`) to avoid drawing "Agent using app" (shown while the human crops a locked session's screenshot) over it. Each new toast makes this worse.

Shape: the kit keeps a per-frame toast stack per area (ctx temp data keyed by a caller-given stack id, e.g. the view's id, reset at frame start via the pass number). `toast(ui, id, stack, clip, add)` takes the next free position, using the real height of the toasts below it (last frame's rects) rather than the fixed `toast_slot`. Callers then just call `toast()` in the order they want, bottom first. Drop the `slot` parameter.
