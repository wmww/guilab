# Compositor `Session`: split the god struct, give invariants one owner

Priority: medium (grows with every feature; several invariants are maintained by hand at each call site).

## What's wrong
`compositor/src/session.rs` `Session` has ~60 fields, nearly all `pub`, written from every module and from the server (`slot.session.dirty = true`, `slot.session.log.push`, `s.viz.push`). Specific problems:
- **Settle tracking** is 7 loose timestamps (`last_commit`, `last_render`, `last_configure`, `last_input`, `busy_since`, `last_spawn`, `last_map`) updated in `handlers.rs commit`, `input.rs inject`, `procs.rs spawn`, `tree.rs track_windows`, `frame.rs render`, and `last_configure = Instant::now()` by hand after each of 6 configure sites in `shell.rs` (~169, 250, 278, 320, 355, 418). A new configure path that forgets it makes screenshots wait wrongly. The *decision* is split too: `frame.rs settle_state` in the compositor, but the caps (`SETTLE_CAP`, `MAP_CAP`) and the "no window and no running apps → don't wait" rule in `server/src/requests.rs check_waiters` (~600).
- **Clipboard state** (`clip_mimes`, `clip_seq`) is updated in four places (`clipboard.rs clip_set`, `handlers.rs new_selection`, `xwayland.rs new_selection` and `cleared_selection`); `cleared_selection` clears `clip_mimes` but doesn't bump `clip_seq` (harmless for the viewer bridge today, but it shows the rule "every change bumps the seq" has no owner).
- **Window policy** "tile if parentless and no other tiled window" is written twice: `shell.rs apply_policy` (Wayland) and `xwayland.rs map_window_request` (X), with slightly different exclusions.
- **Scale conversions**: `self.output.current_scale().fractional_scale()` appears 17 times across compositor and server, with ad-hoc px↔logical rounding (`input.rs px`, `actions.rs point`/`to_logical`, `cursor.rs`, `frame.rs color_under_pointer`, `tree.rs node`).
- **Cleanup gap**: `Session::new` starts the private `dbus-daemon` (`bus.rs`) and then `start_xwayland()?`; if that fails (or `insert_session` fails in the server), the `Session` is dropped without `shutdown()` and the bus daemon is leaked (`std::process::Child` doesn't kill on drop; PDEATHSIG only fires when the server dies). Same for a child whose `pidfd_open`/`insert_source` fails in `procs.rs spawn` (spawned, never tracked).
- Dead or misleading: `last_states` is written (`frame.rs` ~105) and never read; `Session::dir()` is unused (field is pub); `Session::tag()` clones a pub field; `seen_windows` has two contradictory doc lines; `ClientState` is re-exported but only used inside the crate.

## Proposed shape
- Sub-structs owned by `Session`, fields private, with methods that keep their invariant:
  - `Settle` (`frame.rs` or new `settle.rs`): `note_commit/note_input/note_configure/note_spawn/note_map/note_render`, and `fn ready(&self, now, waited, settle: bool, running_apps) -> Option<ShotState>` that includes the caps now in `check_waiters`. Configures go through one `Session::configure(&ToplevelSurface)` helper that sends and notes.
  - `Clipboard` (`clipboard.rs`): `set_owner(mimes)` / `clear()` bump the seq; all four call sites use it.
  - `InputState` (`input.rs`): `held`, `key_repeat`, `extra_keys`, `click_in_grab`, `gesture`, `x_focus_at`.
  - `CursorState` (`cursor.rs`): `cursor_status`, `cursor_seq`, `cursor_image`, `named_cursor`.
- `fn scale(&self) -> f64` plus `to_px(Point<Logical>) -> (i32, i32)` / `from_px(f64, f64)` on `Session`, used everywhere.
- One `fn should_tile(&self, w: &Window, parentless: bool) -> bool` in `shell.rs` for both shells.
- `impl Drop for Session` as a safety net: kill the bus and any still-tracked children (the normal path, `shutdown()`, already took them). Track a spawned child before fallible pidfd setup, or kill it on that error.
- Server side: keep `Session::log` where it is (the compositor logs input and windows), but stop the server writing `dirty`/`viz` directly: `Session::touch()` for "a command ran" and a `Session::note(text)` for the overlay note in `requests.rs set_output`.
- Remove the dead items listed above.
