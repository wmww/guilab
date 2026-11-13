# Code map

## Crates (`crates/`)
- `proto` (`guilab-proto`): control protocol types (`Request`/`Command`/`Response` + reply structs), socket paths, blocking `Client`. serde only. Everything the CLI and server agree on lives here.
- `compositor` (`guilab-compositor`): smithay. `Session` = one Wayland display. No IPC, no CLI.
  - `session.rs`: `Session` struct, globals, socket, seat, output. `Session::new` returns the session plus its own `EventLoop<Session>`.
  - `handlers.rs`: all smithay handler traits except xdg-shell, `delegate_dispatch2!`, surface ids (`SurfaceId` in each surface's data map, assigned in `new_surface`), dmabuf blocker pre-commit hook.
  - `shell.rs`: xdg-shell + decoration. Window policy, focus, popups (anvil's unconstrain + grab), `relayout` after output changes.
  - `frame.rs`: `tick()` (render, presentation feedback, frame callbacks), settle detection, PNG writing, `set_output`.
  - `render.rs`: `Gpu` (GLES on a render node, pixman fallback; `GUILAB_RENDERER=pixman|/dev/dri/renderD*`), per-session offscreen `Frame` with a damage tracker.
  - `tree.rs`: `tree` reply. `procs.rs`: run/kill/ps and cleanup.
- `server` (`guilab-server`): the daemon. Outer calloop loop: control socket, connections, a 16 ms timer, signals, each session's nested loop. `requests.rs`: request dispatch, session resolution, `new`, screenshot waiters.
- `guilab`: the binary. `main.rs` (clap CLI), `client.rs` (connect + autostart, stale-server warning, `server`), `format.rs` (text output). Integration tests in `tests/it/` (one test binary).
- Planned: a viewer crate (egui, host window) and an annotation/queue crate depending only on egui + image types (see design.md).

## Runtime layout
`$XDG_RUNTIME_DIR/guilab/`: `control.sock`, `server.lock` (flock: one server per runtime dir), `server.log` (autostarted server's stderr), `s/<session>/` = the session dir: `wayland-0` socket, `app-N.log`, default screenshots `shot-N.png`. The session dir is the apps' `XDG_RUNTIME_DIR`, so apps don't see the user's bus, pipewire or compositor. It's deleted when the session ends. Socket paths must stay under 108 bytes, hence session names are capped at 32 chars and tests use `$XDG_RUNTIME_DIR/glt-*` rather than the scratchpad.

## Event loops
- Server loop data is `Server`. Each session's `EventLoop<Session>` is inserted as a `Generic` source (an EventLoop is `AsFd`); on readiness the server dispatches it with `&mut slot.session`, then flushes clients. The session's display, socket, pidfds and dmabuf blockers are sources in the session loop.
- **Never put a calloop `Timer` in a session loop**: calloop timers live in the poll timeout, not in an fd, so they never wake the server loop. Timed work goes through the server's 16 ms tick (`Server::tick`: session `tick()`, `reap_tick()`, screenshot waiters, graveyard, TTL/idle once a second).
- No source closure in a session loop may capture its `LoopHandle` (Rc cycle, the session would leak). Use `s.handle` from the `&mut Session` argument.
- smithay APIs taking `LoopHandle<'static, D>` (e.g. `X11Wm::start_wm`) get `session.handle`.

## Seams for later steps
- **Lock/input/log**: all requests go through `Server::handle` (`requests.rs`) after `resolve`, so a lock check or a log entry has one place to go. Input will live in the compositor (`Session` has `seat`, pointer, keyboard (repeat rate 0) and touch already).
- **Xwayland**: per session, in the session loop. `client_compositor_state` must also accept `XWaylandClientData`. `SeatHandler::KeyboardFocus` is `WlSurface` today; X11 focus needs an enum target (anvil's `focus.rs`) so `X11Surface` does SetInputFocus. `Procs::spawn` will need `DISPLAY`. Window policy in `shell.rs` decides at the initial configure; X11 windows need the same tiled/floating rule.
- **Viewer**: `render::Frame` already keeps a persistent GLES texture target per session that the viewer can sample.
- **Event log**: app exits are detected in `Session::reap` (pidfd), window map/unmap in `shell.rs`.

## Behaviours worth knowing
- Settle (`Session::settle_state`): a toplevel has a buffer, no configured-but-unmapped toplevel, no unanswered configure (not acked, or acked but not yet committed: GTK acks a scale change and commits ~20 ms later; ignored once `CONFIGURE_WAIT` = 1 s passed since we last configured, because clients may ack without committing), 150 ms without commits, and a render after the last commit. The server caps waits at 3 s (10 s before the first window). Animated apps always hit the cap.
- Apps get their own process group and `GUILAB_TAG=<tag>`. Kill = SIGTERM to the group, SIGKILL after 2 s. Ending a session also signals every process carrying the tag (`/proc/*/environ` scan), like guibox. Apps die with the server via PDEATHSIG.
- The server blocks SIGTERM/SIGINT/SIGHUP (calloop signalfd) and ignores SIGPIPE; `std::process::Command` doesn't reset either, so `spawn` clears the mask in `pre_exec`. Without that, apps ignore SIGTERM.
- At fractional scales a tiled window can be one pixel larger than the output (500 px / 1.5 = 333.3 logical, rounded up to 334). `tree` reports it honestly.
- `tree` lists toplevels topmost first, coordinates in output pixels (edges rounded half-down).
- Window policy: a parentless toplevel tiles if no other tiled one exists; otherwise it floats, centered, with `bounds` = output (so an oversized default size fills the output anyway).
- Popups are positioned by smithay's positioner (flip/slide/resize) against the output, relative to their parent window.
- Edition 2024 `if let` keeps temporaries alive through the block: `if let Some(w) = self.space.elements().find(..).cloned() { self.space.map_element(..) }` fails to borrow-check; bind the result first.

## Build and test
- smithay is git master pinned in the root `Cargo.toml` (crates.io 0.7 lacks `delegate_dispatch2!`, `inject_text_keysyms`, etc.). Bump the rev deliberately.
- Only `cargo test --workspace` (narrow with `--test it <module>::` or a name filter) and `cargo clippy --workspace --all-targets`. No `-p`, no feature flags, no per-command env (each combination builds another copy of the tree). Dev profile only; deps at opt-level 2, line-tables debuginfo, compressed debug sections (`.cargo/config.toml`).
- compositor and server libs have `test = false`: each lib test binary would link all of smithay. Prefer integration tests in `crates/guilab/tests/it/`, or unit tests in `proto`.
- Integration tests start a private server per test (`Lab`, runtime dir `$XDG_RUNTIME_DIR/glt-<pid>-<n>`) and skip tests whose apps aren't installed. Test apps: `tests/it/apps/gtk_app.py` (PyGObject GTK4: `--menu` popover in the bottom-right corner, `--dialog`, `--second`), `qt_app.py` (PySide6: `--menu`).
- Numbers (24-core machine): clean build ~75 s; no-op 0.05 s; one-line edit in compositor or server + test build ~0.6 s; `cargo test --workspace` runs in ~5.5 s; `target/` ~1.1 GB. 3x worse at low load means something's wrong.
- Manual testing: the CLI against the real runtime dir works fine; `guilab server --stop` after rebuilding (the CLI warns when the server's binary is older than itself). `GUILAB_LOG=warn,guilab_compositor=trace guilab server` runs a foreground server with commit tracing.
