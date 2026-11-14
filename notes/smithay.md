# smithay: reference for upcoming work

What's already built (rendering, sessions, xdg-shell, frame clock, settle) is described in `code-map.md` and the code. This note keeps the verified research that later steps still need. Checked against smithay master `9ff2c5a` (the pinned rev).

## Reference code and licenses
- smithay and anvil are MIT; copy freely. niri and cosmic-comp are GPL-3.0: ideas only.
- anvil (`smithay/anvil/src/`): `input_handler.rs` (input), `cursor.rs` + `drawing.rs` (cursor render element for `screenshot --cursor`), `state.rs` (`pre_repaint`/`post_repaint` if fifo/commit-timing get enabled).
- Since 2026-04 one `delegate_dispatch2!(Session);` replaces all `delegate_*!` macros; older examples use the old ones.
- Smithay's `AI.md` asks contributors not to submit LLM-written code. Matters only if we upstream fixes.

## Protocols
Enabled per session (`Session::new`): compositor v6, subcompositor, shm, xdg-shell, xdg-decoration (forced server-side, nothing drawn), xdg-output, seat, linux-dmabuf with default feedback (GLES only), data device (GTK4 refuses to start without it), primary selection, wlr + ext data-control (for `clip`), xdg-activation, xdg-foreign, keyboard-shortcuts-inhibit, presentation-time, viewporter, fractional-scale, single-pixel-buffer, cursor-shape, pointer-gestures, relative-pointer, pointer-constraints, text-input-v3, idle-inhibit, wl_fixes.
- Skipped: layer-shell, session-lock, screencopy/image-copy-capture, output management, foreign-toplevel, security-context, drm-lease, fifo/commit-timing. If fifo/commit-timing are enabled, their barriers must be signalled every tick (anvil `pre_repaint`/`post_repaint`).
- For Xwayland glamor, linux-dmabuf must be v4+ with feedback naming our render node (it is).

## Input
Built (`input.rs`, `actions.rs`, `keys.rs`; behaviours in `code-map.md`). Still useful:
- `InputTime::now()` for synthetic events. Gestures: `ptr.gesture_{swipe,pinch,hold}_*`, then `ptr.frame`. Touch focus is fixed at `down`.
- Text-input-v3 only routes to an IME; smithay has no compositor-side `commit_string`, so it isn't a typing path.
- Not built: hold gestures, DnD icon `wl_surface.offset` (icon is drawn at the pointer).
