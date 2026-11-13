# smithay: core compositor reference

What smithay and the reference compositors give us for the core compositor (rendering, displays, xdg-shell, frame timing, input, protocols). Xwayland, host presentation and egui are in other notes.

All of the following was checked against smithay master `9ff2c5a` (Nov 2026). A throwaway prototype verified it: two `Display<Session>`s in one calloop loop, GLES via GBM on renderD128 (and pixman), offscreen render plus readback. GTK4, kitty (GL/dmabuf), weston-simple-shm/egl all rendered correctly, and kitty received injected text.

## Version
- crates.io `smithay 0.7.0` dates from 2025-06, with 527 commits since. Too old: it lacks `delegate_dispatch2!`, `KeyboardHandle::inject_text_keysyms`, xdg-decoration v2, `with_committed_state` and more. Master has many breaking changes vs 0.7 (see `CHANGELOG.md` "Unreleased").
- niri pins git `65aa86fa` and cosmic-comp pins git `8a27023`. Both are recent master.
- **Use git with a pinned rev** (e.g. `9ff2c5a410e91c592e19d6fb126c299308dce119`). MSRV is 1.88. Bump the rev deliberately.
- Features: `default-features = false`, `["wayland_frontend", "desktop", "backend_egl", "backend_gbm", "renderer_gl", "renderer_pixman", "xwayland"]`. Runtime libs: libgbm, libpixman, libxkbcommon, libdrm, plus libEGL, which is dlopened.
- Smithay's `AI.md` asks contributors not to submit LLM-written code. That matters only if we upstream fixes.

## Reference code
- `smithay/smallvil/` (~1.2k lines) is the minimal skeleton: state, handlers, xdg, grabs. Start from it.
- `smithay/anvil/src/` holds the full feature set. Worth copying: `shell/xdg.rs` (popup unconstrain + popup grab), `shell/mod.rs` (commit handler, `ensure_initial_configure`), `state.rs` (global init, `pre_repaint`/`post_repaint`, `update_primary_scanout_output`, fractional scale), `input_handler.rs`, `cursor.rs` + `drawing.rs` (cursor element for `--cursor` screenshots).
- niri (`src/backend/headless.rs`, `src/render_helpers/mod.rs` `render_and_download`) and cosmic-comp are GPL-3.0. Read them for ideas only; everything we need exists in MIT smithay/anvil.

## Licenses
smithay and anvil are MIT. niri and cosmic-comp are GPL-3.0. Nothing below requires GPL code, so guilab can stay MIT.

## Handler boilerplate
Since 2026-04, one `delegate_dispatch2!(Session);` replaces all the `delegate_*!` macros. Examples older than that (and docs.rs 0.7) use the old macros.

## Headless rendering (verified)
```rust
let f = OpenOptions::new().read(true).write(true).open("/dev/dri/renderD128")?;
let fd = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(f)));  // Clone, so GbmDevice<DrmDeviceFd> is Clone
let gbm = GbmDevice::new(fd)?;                      // smithay::backend::allocator::gbm
let egl = unsafe { EGLDisplay::new(gbm.clone())? }; // keep a clone for GbmAllocator (dmabuf targets)
let mut r = unsafe { GlesRenderer::new(EGLContext::new(&egl)?)? };
// offscreen target + readback
let mut rb: GlesRenderbuffer = r.create_buffer(Fourcc::Abgr8888, size)?;   // Offscreen trait
let mut fb = r.bind(&mut rb)?;                                              // Bind trait
smithay::desktop::space::render_output::<_, WaylandSurfaceRenderElement<_>, _, _>(
    &output, &mut r, &mut fb, 1.0, age, [&space], &custom_elems, &mut damage_tracker, clear)?;
let map = r.copy_framebuffer(&fb, Rectangle::from_size(size), Fourcc::Abgr8888)?; // ExportMem
let bytes = r.map_texture(&map)?;   // RGBA8 in memory order
```
- No session, libseat or KMS is needed. Opening the render node needs no special permissions.
- To pick a node, scan `/dev/dri/renderD*`, or use `DrmNode` + `EGLDevice::enumerate()`.
- `OutputDamageTracker::from_output(&output)` follows mode and scale changes.
- Keep a persistent target per session and pass a real `age`, so `RenderOutputResult.damage == None` means nothing changed. A fresh buffer each frame means age 0 and full redraws.
- A `GlesTexture` target also works (`Offscreen<GlesTexture>`) and can be drawn directly by the viewer.
- A dmabuf target (`GbmAllocator::create_buffer`, then `r.bind(&mut dmabuf)`) is what host presentation would use.
- **Pixman fallback** (verified with GTK4): `PixmanRenderer::new()`, target `pixman::Image` via `create_buffer`, then the same `render_output`/`copy_framebuffer`. Make the renderer an enum or generic. Without the dmabuf global, Mesa clients fall back to software GL over shm by themselves, with noisy warnings.
- **linux-dmabuf in headless mode** (verified, v6 with feedback; Mesa clients use it):
  ```rust
  let node = EGLDevice::device_for_display(r.egl_context().display())?.try_get_render_node()?.unwrap();
  let fb = DmabufFeedbackBuilder::new(node.dev_id(), r.dmabuf_formats()).build()?;
  dmabuf_state.create_global_with_default_feedback::<Session>(&dh, &fb);
  // DmabufHandler::dmabuf_imported: r.import_dmabuf(&buf, None) then notifier.successful()/failed()
  ```
  Copy anvil's `new_surface` pre-commit hook (`shell/mod.rs`): it adds a dmabuf fence blocker (`generate_blocker`), so we never sample half-rendered buffers.
- niri's headless backend uses `EGLSurfacelessDisplay`. That needs no device, but there's no render node, so no dmabuf feedback. GBM on the render node is better.

## Several displays in one loop (verified)
**Recommendation: one `Display<Session>` per session.** The calloop data is `Server { sessions, renderer, .. }`. Every smithay handler trait is implemented on `Session`, and each display source dispatches into its own session:
```rust
lh.insert_source(Generic::new(display, Interest::READ, Mode::Level), move |_, d, srv: &mut Server| {
    let s = srv.sessions.get_mut(&id).unwrap();
    unsafe { d.get_mut().dispatch_clients(s).unwrap(); }
    Ok(PostAction::Continue)
});
lh.insert_source(ListeningSocketSource::with_name("guilab-NAME")?, move |stream, _, srv| {
    srv.sessions[&id].dh.insert_client(stream, Arc::new(ClientState::default())).unwrap();
});
```
- This gives full isolation for free: separate globals, client lists, `Seat<Session>`, `Output`, `Space`, `PopupManager` and later `X11Wm<Session>`. No global filters or per-client tagging needed.
- Shared resources (the renderer) live in `Rc<RefCell<..>>` held by each session, since handlers like `dmabuf_imported` need them.
- Handlers that need the loop store a `LoopHandle<'static, Server>` plus their session id.
- Remember the `RegistrationToken`s and remove them when a session ends. Dropping the `Display` disconnects its clients.
- Look sessions up by a stable id (a map), not a Vec index.
- The alternative (one Display, several sockets, `can_view` filters, per-client session lookup in every handler) is more code and leaks easily. Rejected.
- `SERIAL_COUNTER` is process-global. Harmless.

## xdg-shell, popups, placement
- **Toplevel** (tiled, undecorated, fills the output). In `new_toplevel`, set `size = output logical size` and the states `Activated`, `Maximized` and `Tiled{Left,Right,Top,Bottom}`. GTK then drops shadows and rounded corners. Map at `(0,0)` and give keyboard focus.
- Send the initial configure from the commit handler (`ensure_initial_configure` in anvil), never in `new_toplevel`.
- On output resize or scale change, update the pending size and call `send_pending_configure()`.
- Toplevels that aren't the first one, and dialogs (`parent` set), still need a policy: stack and center dialogs, or tile everything. The single-toplevel assumption breaks for GTK dialogs, file choosers and second windows.
- **Popups**: `PopupManager::track_popup` in `new_popup`, `popups.commit(surface)` in every commit, `popups.cleanup()` each frame.
- Constraints: copy anvil's `unconstrain_popup`. The target is the output geometry made relative to the popup's parent: `target.loc -= get_popup_toplevel_coords(..) + window_geo.loc`. Then `state.geometry = positioner.get_unconstrained_geometry(target)`. Smithay implements flip, slide and resize. Do the same in `reposition_request` followed by `send_repositioned(token)`.
- Grabs: copy anvil `XdgShellHandler::grab` (`PopupManager::grab_popup` + `PopupKeyboardGrab` + `PopupPointerGrab`). Clicking outside dismisses the chain. This works the same with injected input.
- Rendering and hit testing come free. `Window`'s render elements include all nested popups (`PopupManager::popups_for_surface` walks the tree), drawn above the toplevel. `Window::surface_under(p, WindowSurfaceType::ALL)` covers popups and subsurfaces.
- For `tree`: walk `space.elements()`, `PopupManager::popups_for_surface(root)` (gives the offset of each popup) and `with_surface_tree_downward` for subsurfaces.
- **xdg-decoration**: in `new_decoration`/`request_mode`, force `Mode::ServerSide` and draw nothing. Qt, kitty and GTK3 then drop CSD. GTK4 and libadwaita keep their headerbar, which is fine.

## Frame clock and presentation
There's no vblank. One calloop `Timer` per session at the output refresh rate (e.g. 60Hz) does this each tick, mirroring anvil `winit.rs`:
1. Render (skip readback unless a screenshot or viewer needs it).
2. Call `update_primary_scanout_output(..)` with `res.states` (anvil `state.rs`). This is needed so `surface_primary_scanout_output` works, which frame callbacks and fractional scale use.
3. Presentation: `take_presentation_feedback(&output, &space, &states).presented(now, Refresh::fixed(16.67ms), seq, Kind::empty())`. Create `PresentationState::new::<S>(&dh, CLOCK_MONOTONIC)`.
4. `window.send_frame(&output, now, Some(Duration::from_secs(1)), surface_primary_scanout_output)`. The throttle re-sends at least once a second to surfaces that aren't visible.
5. `space.refresh()`, `popups.cleanup()`, `dh.flush_clients()`.
- If we enable `fifo` / `commit_timing` (anvil does), we must signal their barriers per tick: copy `pre_repaint`/`post_repaint` from anvil. Skip both protocols at first.
- Optionally idle the timer when nothing is damaged and no callbacks are pending, since many sessions × 60Hz adds up. Clients waiting on callbacks still get them via the throttle.

## Settled screenshots
Smithay has nothing for this. Plan:
- Commit hook: record `last_commit` per session, and whether that commit requested frame callbacks.
- **Settled** = a toplevel is mapped, AND no unacked configure (role data `pending_configures().is_empty()`; also anvil-style resize state), AND no commits for a quiet period (~100 ms; a blinking cursor still passes), AND at least one render since the last commit, AND no dmabuf blockers pending.
- Clients only request frame callbacks while animating. "Last commit asked for a callback, we fired it, and no new commit followed" is a strong idle signal.
- Cap the wait (~2–3 s). Spinners and animations never settle, so return the frame anyway with an "unsettled" flag.
- Before first map, wait for map (also capped). This covers the race between `run` and an immediate `screenshot`.
- Optional extra check: if commits continue but `RenderOutputResult.damage` is `None` (same target, real `age`), the content didn't change, so treat it as quiet.

## Input injection (verified pointer, button, keys)
- Seat: `seat.add_keyboard(XkbConfig { layout: "us", .. }, delay, rate)`, `seat.add_pointer()`, `seat.add_touch()`. xkb needs xkeyboard-config data (`/usr/share/X11/xkb`) at runtime.
- **Pointer**: `ptr.motion(s, Some((surface, surface_origin_global)), &MotionEvent { location: global, serial, time })`, then `ptr.frame(s)`. Buttons use `ptr.button(s, &ButtonEvent { button: 0x110 /*BTN_LEFT*/, state, .. })`. Scrolling uses `ptr.axis(s, AxisFrame::new(time).source(AxisSource::Wheel).value(Axis::Vertical, dy).v120(..))`.
  - Surface-local coordinates: the client receives `location - surface_origin`. For `on SURFACE`, compute the surface's global origin (window location + popup offset from `popups_for_surface` + subsurface offset) and add the local point.
  - Normal moves should hit-test with `surface_under`. Passing an explicit focus bypasses that. An active grab (popup or DnD) still sees the event first.
- **Gestures**: `ptr.gesture_{swipe,pinch,hold}_{begin,update,end}`. These need a `PointerGesturesState` global.
- **Touch**: `touch.down(s, Some((surface, origin)), &DownEvent { slot, location, serial, time })`, then `motion` / `up` / `frame` (touch focus is fixed at `down`).
- **Time**: `InputTime::now()` (`smithay::backend::input`). Events take `InputTime`, not `u32`.
- **Keyboard**: `kb.input::<(), _>(s, Keycode::new(evdev + 8), KeyState::Pressed, serial, time, |_, _, _| FilterResult::Forward)`.
  - Chords: press the modifier keycodes in order.
  - `kb.keycode_for_keysym(sym)` does reverse lookup, but it doesn't say which level, so press Shift yourself for level-1 symbols (e.g. check `key_get_syms_by_level`).
  - Keys held without a release auto-repeat on the client side (kitty showed it). Consider repeat rate 0 for deterministic agent input, or document it.
- **Typing any text**: `kb.inject_text_keysyms(s, &keysyms, time)` (master only) builds a throwaway keymap, sends it to the focused client, taps, and restores the seat keymap. Verified: kitty typed `é→`.
  - Gotcha: it binds keysyms to xkb codes starting at 9, which is evdev KEY_ESC, then digits. Clients that read raw keycodes see Esc: weston demos quit on it.
  - Fix: copy that function (~80 lines, MIT) and use spare codes (e.g. 200–255, chunked). Prefer real key presses for characters in the current keymap, and use injection only for the rest.
  - It's a no-op without keyboard focus.
  - For Xwayland clients, keymap switches are slow and racy (wtype has the same issue).
- **Text input** (`TextInputManagerState`) only routes to an IME client. Smithay has no compositor-side `commit_string`, so it isn't a typing path. Still enable it, since GTK and Qt expect it.

## Protocols to enable
These are cheap with smithay, and anvil and niri both create most of them. Recommended set per session:
- **Must**: compositor (`CompositorState::new_v6`, needed for `preferred_buffer_scale`), subcompositor (comes with compositor), shm, xdg-shell, `OutputManagerState::new_with_xdg_output`, seat, linux-dmabuf, **data device**.
  - GTK4 refuses to start without `wl_data_device_manager` ("required interfaces" error, verified).
- **Should**: primary selection, viewporter, fractional-scale, presentation-time, xdg-decoration, cursor-shape (`CursorShapeManagerState`; otherwise we must render client cursor surfaces), xdg-activation (GTK and Firefox focus requests), single-pixel-buffer, pointer-gestures, relative-pointer + pointer-constraints (`PointerConstraintsHandler` is a required bound on `SeatHandler` anyway), text-input-v3, keyboard-shortcuts-inhibit, xdg-foreign (portals and dialogs), idle-inhibit, `FixesState`, data-control (`wlr` and `ext`) for `clip get/set` tooling if useful.
- **Skip**: layer-shell, session-lock, screencopy / image-copy-capture, output management, foreign-toplevel, security-context (unless sandboxed clients), drm-lease, fifo / commit-timing (initially).
- **Runtime scale change**:
  - `output.change_current_state(None, None, Some(Scale::Fractional(s)), None)` sends `wl_output.scale` (ceil) and xdg-output updates itself.
  - We must push surface state ourselves: for each surface, `send_surface_state(surface, data, scale.integer_scale(), transform)` (compositor v6) and `with_fractional_scale(data, |f| f.set_preferred_scale(s))`.
  - Then reconfigure the toplevel to the new logical size.
  - Anvil only sets fractional scale in `post_repaint` and `new_fractional_scale`. Niri does both (its `utils::send_scale_transform`, idea only).
  - The output resize path is the same: `change_current_state(Some(mode), ..)`, then reconfigure.
