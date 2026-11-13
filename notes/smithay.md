# smithay: reference for upcoming work

What's already built (rendering, sessions, xdg-shell, frame clock, settle) is described in `code-map.md` and the code. This note keeps the verified research that later steps still need. Checked against smithay master `9ff2c5a` (the pinned rev).

## Reference code and licenses
- smithay and anvil are MIT; copy freely. niri and cosmic-comp are GPL-3.0: ideas only.
- anvil (`smithay/anvil/src/`): `input_handler.rs` (input), `cursor.rs` + `drawing.rs` (cursor render element for `screenshot --cursor`), `focus.rs` (enum focus targets, needed for Xwayland), `shell/x11.rs` (X11 WM glue), `state.rs` (`pre_repaint`/`post_repaint` if fifo/commit-timing get enabled).
- Since 2026-04 one `delegate_dispatch2!(Session);` replaces all `delegate_*!` macros; older examples use the old ones.
- Smithay's `AI.md` asks contributors not to submit LLM-written code. Matters only if we upstream fixes.

## Protocols
Enabled per session (`Session::new`): compositor v6, subcompositor, shm, xdg-shell, xdg-decoration (forced server-side, nothing drawn), xdg-output, seat, linux-dmabuf with default feedback (GLES only), data device (GTK4 refuses to start without it), primary selection, wlr + ext data-control (for `clip`), xdg-activation, xdg-foreign, keyboard-shortcuts-inhibit, presentation-time, viewporter, fractional-scale, single-pixel-buffer, cursor-shape, pointer-gestures, relative-pointer, pointer-constraints, text-input-v3, idle-inhibit, wl_fixes.
- Skipped: layer-shell, session-lock, screencopy/image-copy-capture, output management, foreign-toplevel, security-context, drm-lease, fifo/commit-timing. If fifo/commit-timing are enabled, their barriers must be signalled every tick (anvil `pre_repaint`/`post_repaint`).
- For Xwayland glamor, linux-dmabuf must be v4+ with feedback naming our render node (it is).

## Input injection (verified pointer, button, keys)
- Seat already has keyboard (`us`, repeat rate 0), pointer and touch. xkb needs xkeyboard-config at runtime.
- **Pointer**: `ptr.motion(s, Some((surface, surface_origin_global)), &MotionEvent { location: global, serial, time })`, then `ptr.frame(s)`. Buttons: `ptr.button(s, &ButtonEvent { button: 0x110 /*BTN_LEFT*/, state, .. })`. Scroll: `ptr.axis(s, AxisFrame::new(time).source(AxisSource::Wheel).value(Axis::Vertical, dy).v120(..))`.
  - The client receives `location - surface_origin`. For `on SURFACE`, compute the surface's global origin (window location + popup offset from `popups_for_surface` + subsurface offset; `tree.rs` already does this walk) and add the local point.
  - Normal moves should hit-test with `space.element_under` + `Window::surface_under(p, WindowSurfaceType::ALL)` (covers popups and subsurfaces). An active grab (popup, DnD) sees events first.
  - CLI coordinates are output pixels (like `tree` and screenshots); divide by the output scale to get logical coordinates.
- **Gestures**: `ptr.gesture_{swipe,pinch,hold}_{begin,update,end}` (the pointer-gestures global exists).
- **Touch**: `touch.down(s, Some((surface, origin)), &DownEvent { slot, location, serial, time })`, then `motion`/`up`/`frame`; focus is fixed at `down`.
- **Time**: `InputTime::now()` (`smithay::backend::input`).
- **Keyboard**: `kb.input::<(), _>(s, Keycode::new(evdev + 8), KeyState::Pressed, serial, time, |_, _, _| FilterResult::Forward)`. Chords: press modifiers in order. `kb.keycode_for_keysym(sym)` doesn't say which level, so press Shift yourself for level-1 symbols (check `key_get_syms_by_level`).
- **Typing any text**: `kb.inject_text_keysyms(s, &keysyms, time)` builds a throwaway keymap, taps, restores. Verified (kitty typed `é→`). Gotcha: it maps keysyms to xkb codes from 9 = evdev KEY_ESC, so raw-keycode clients see Esc (weston demos quit). Copy the function (~80 lines, MIT) and use spare codes (200–255, chunked); use real key presses for characters in the keymap. No-op without keyboard focus. Slow and racy for Xwayland clients.
- Text-input-v3 only routes to an IME; smithay has no compositor-side `commit_string`, so it isn't a typing path.
- Popup grabs (`shell.rs`, from anvil) are untested with real input; Qt doesn't even open a `QMenu` without an input serial (step 3 should test both).
