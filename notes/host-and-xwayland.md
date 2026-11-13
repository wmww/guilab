# Xwayland, host viewer window, egui

Research results for plan steps 4 and 6. Versions checked: smithay 0.7.0 (crates.io) and git main, sctk 0.20.0, egui/egui_glow 0.35, Xwayland 24.1.

## Licenses
- MIT, free to copy: smithay and anvil, smithay-egui, sctk, calloop-wayland-source. egui is MIT/Apache.
- GPL-3.0: cosmic-comp, niri. MPL-2.0: xwayland-satellite.
- Nothing below needs GPL code. The cosmic-comp logic worth having is a few lines each, so reimplement it rather than copy.

## 1. Xwayland: use smithay's built-in `X11Wm`
Recommendation: built-in (`xwayland` feature). It's mature: ICCCM/EWMH focus (`WM_TAKE_FOCUS`, input model), stacking, clipboard/primary bridging, DnD, XSETTINGS, `_NET_WM_SYNC`, randr primary. The only runtime dependency is the `Xwayland` binary on PATH (>= 23.1 for xwayland_shell_v1).

Why not xwayland-satellite (what niri uses, spawned on demand when a client hits niri's X socket): niri avoids writing an X11 WM (see "X11 is very cursed" in niri's docs/wiki/Xwayland.md). But satellite is an external binary that needs xcb libs. X windows become plain xdg toplevels and popups, so we'd lose X window ids, WM_CLASS and absolute positions for `tree`. Apps that place windows at absolute coordinates break. guilab tiles a single window per session, so a WM is only a few hundred lines for us.

### Startup (copy anvil `state.rs::start_xwayland`, about 40 lines)
```rust
let (xw, client) = XWayland::spawn(&session_dh, None /*or Some(n)*/, envs, extra_args,
    true /*abstract socket*/, Stdio::null(), Stdio::null(), |_ud| ())?;
handle.insert_source(xw, move |ev, _, st| match ev {
    XWaylandEvent::Ready { x11_socket, display_number } => {
        st.client_compositor_state(&client).set_client_scale(scale); // optional
        let wm = X11Wm::start_wm(loop_handle, &dh, x11_socket, client.clone())?;
        // wm.set_cursor(rgba, size, hotspot) for the root cursor
        // store wm + DISPLAY=:{display_number} on the session
    }
    XWaylandEvent::Error => { /* died during startup */ }
})?;
```
- smithay passes `-rootless -terminate -wm FD -displayfd FD -listenfd ...`. Its environment is cleared except PATH and XDG_RUNTIME_DIR, plus `envs`; use `spawn_with_inherited_env` to keep everything. Shutdown: drop the `XWayland` and the `X11Wm`, and Xwayland exits. Lock and socket files go away when the `XWayland` drops.
- `XWaylandShellState::new::<D>(&dh)` per session display (the xwayland_shell_v1 global). Also implement `XWaylandShellHandler`.
- For GPU (glamor) in Xwayland, the session must advertise linux-dmabuf v4 feedback whose main device is our render node. Otherwise Xwayland falls back to shm, which is slower but works.

### Display numbers
- `display = None`: smithay tries :0 through :32. For each it creates `/tmp/.X{n}-lock` with O_EXCL and writes its pid, then binds `/tmp/.X11-unix/X{n}` plus the abstract socket. A lock whose pid is dead gets reclaimed (src/xwayland/x11_sockets.rs). This is safe across sessions, guibox and the user's desktop, since all of them use the same lock files.
- The limit is 33 displays for the whole machine. To avoid it, loop over `Some(n)` ourselves (e.g. n in 100..): with `Some`, smithay tries only that number and returns AddrInUse on conflict.
- Apps in the session get `DISPLAY=:{n}`. Abstract sockets are visible to every process in the network namespace. That's fine, since the isolation is only for convenience.

### XwmHandler glue (template: anvil `anvil/src/shell/x11.rs`, 430 lines, MIT)
- `map_window_request`: `w.set_mapped(true)`, wrap it with `desktop::Window::new_x11_window(w)`, then `w.configure(output_rect)` to tile it. Treat dialogs like transient xdg toplevels: if `is_transient_for()` is set or `window_type()` is Dialog, float them over the parent.
- Override-redirect windows (menus, tooltips, DnD icons): `mapped_override_redirect_window` gives a surface already positioned in root coordinates (= output coordinates, since there's one output at 0,0). Keep them in a per-session `Vec<X11Surface>` drawn above everything. Keep it ordered with `configure_notify(.., above)`: move a window above the window whose id is `above`. cosmic-comp does this in `src/xwayland.rs` `configure_notify`, about 15 lines. anvil doesn't reorder them (it has a TODO). Never configure or restack OR windows.
- `configure_request`: give unmapped windows what they ask for, and re-send the tiled geometry to mapped tiled windows (`w.configure(rect)`). Ack fullscreen and maximize requests by just setting state, since the window already fills the output.
- `unmapped_window`: unmap it, and call `set_mapped(false)` unless it's OR. `destroyed_window`/`new_window`: no-op.
- Focus: X11Surface implements Keyboard/Pointer/TouchTarget. `keyboard.set_focus(x11surface)` makes smithay do SetInputFocus/WM_TAKE_FOCUS. Also call `w.set_activated(true)` and `xwm.raise_window(&w)`, or `update_stacking_order_upwards(iter)` for a full order. Pointer: the hit test is `Window::surface_under`/`X11Surface::surface_under`. Menus need nothing extra: X-side pointer grabs happen inside Xwayland, because all X surfaces belong to a single Wayland client.
- Clipboard: copy anvil verbatim. `XwmHandler::{send_selection,new_selection,cleared_selection,allow_selection_access}` from `shell/x11.rs`, plus `SelectionHandler::{new_selection,send_selection}` forwarding to `xwm.new_selection`/`xwm.send_selection` from `state.rs`, lines 255-285. Tag Wayland selections set by X with the `XwmId` as user data, like cosmic does, so `cleared_selection` only clears X-owned ones. `clip get` then works for X-owned selections through `request_data_device_client_selection`. `allow_selection_access` should return true when an X window of that xwm has keyboard focus.
- Scale (`--scale`): set the Xwayland client's `set_client_scale(s)`. Push `Xft/DPI` (96·1024·s), `Gdk/WindowScalingFactor` and `Gdk/UnscaledDPI` with `xwm.set_xsettings(..)`, then re-configure X windows. cosmic-comp's `update_xwayland_settings` is the reference (GPL, so reimplement it; it's small).
- `tree` data: `window_id()`, `title()`, `class()`, `instance()`, `geometry()`, `is_override_redirect()`, `window_type()`, `pid()`.
- Glue total: roughly 300-500 lines. Skip cosmic's key/button "eavesdropping" (global shortcuts) and xrdb.

### Gotcha: several sessions under one state type (affects the multi-Display design)
`XWaylandShellHandler::xwayland_shell_state(&mut self)` has no display or session argument. Its `by_serial` map is keyed by the surface serial, which each Xwayland counts from 1, so sessions sharing one map would collide. Every smithay `*_state(&mut self)` getter has the same shape. The same goes for `X11Wm::start_wm(LoopHandle<'static, D>, ..)`, which inserts sources whose callback gets `&mut D`. Options:
- **Per-session state type**: each session has its own `Display<SessionState>` and its own calloop `EventLoop<SessionState>`, nested in the server loop (calloop `EventLoop` is `AsFd`) and dispatched with `&mut session`. All getters then become trivial. This is cleanest if it fits the other research.
- **One D with a "current session" field**: smithay calls `XwmHandler::xwm_state(xwm_id)` before `xwayland_shell_state()` in both paths that use it (the X event handler and the commit hook), so `xwm_state` can set the current session. This works, but it's fragile.

## 2. Host viewer window: sctk on its own connection, presenting dmabufs from the headless renderer
Verified with a throwaway program (sctk 0.20 + smithay 0.7 GlesRenderer on renderD128 + egui_glow 0.35, shown in headless sway): zero-copy, correct output, Intel CCS 2-plane modifier.

Don't use smithay's winit backend. It builds the `GlesRenderer` on the window's EGL display, so the renderer dies with the window. Worse, winit refuses to create a second `EventLoop` per process (`EventLoopError::RecreationAttempt`), so show/hide/show is impossible. Nothing in `backend/winit` is reusable. `backend/x11/surface.rs` is a useful model: it presents GBM dmabufs to a host through a `Swapchain`.

Also rejected: a second EGL context on the host surface (wayland-egl). It means two renderers, re-importing every session buffer and duplicate egui textures. And shm as the main path: a CPU copy every frame.

### Pipeline
1. The one `GlesRenderer` on the render node (GBM `EGLDisplay`) also renders the viewer. Allocate with `GbmAllocator::new(gbm, GbmBufferFlags::RENDERING)` (`GbmDevice<Arc<File>>`, since `Device<File>` isn't Clone). Use `Swapchain::new(alloc, w, h, Fourcc::Argb8888, mods)` with 2-3 slots, and cache each slot's `wl_buffer` in `slot.userdata()`. Hold the `Slot` until `wl_buffer.release`.
2. Modifiers come from the host's `zwp_linux_dmabuf_v1` default feedback (`DmabufState::get_default_feedback`, `DmabufHandler::dmabuf_feedback`): the tranche format-table entries for ARGB8888, intersected with `renderer.egl_context().dmabuf_render_formats()`. Fall back to `Modifier::Linear`.
3. Draw: `renderer.bind(&mut dmabuf)`, `renderer.render(&mut fb, size, Transform::Normal)`, `frame.clear`, egui painting inside `frame.with_context(|_| painter.paint_and_update_textures(..))`, then `frame.finish()?.wait()`. A CPU wait is fine to start with.
4. Wrap it as a wl_buffer: `params = dmabuf_state.create_params(qh)`. For each plane, `params.add(fd, i, offset, stride, modifier)` from `dmabuf.handles()/offsets()/strides()`. Then `params.create_immed(w, h, fourcc, Flags::empty(), qh)`, followed by attach, damage_buffer, frame callback, commit.
5. Draw a new frame only on the host frame callback, and only when something changed. This is independent of the session frame clocks.
6. **Same GPU check**: the feedback's `main_device()` (a dev_t; it was renderD128's 226:128 here) must be the same GPU as our node. It may be the card node of that GPU, so compare via `/sys/dev/char/MAJ:MIN/device`. If it differs, or there's no linux-dmabuf, fall back to shm: render to an offscreen texture, `ExportMem::copy_framebuffer` + `map_texture`, then write into an sctk `SlotPool` buffer.

### Event loop integration
- Use a separate `EventQueue<HostState>` whose sctk handlers only record events. The server consumes them after dispatch. Insert it with `handle.insert_source(source, |_, q, st| q.dispatch_pending(&mut st.viewer.host))`; the source's D doesn't have to match the loop data type.
- **Don't use `WaylandSource` as is.** Host I/O errors, protocol errors and flush failures (including in `before_sleep`) come back as errors from `EventLoop::dispatch`, which would take down the whole server. Copy calloop-wayland-source's `lib.rs` (MIT, about 200 lines) and turn errors into "host lost": tear down the viewer and return `PostAction::Remove`.
- Do the initial `Connection::connect_to_env` + `registry_queue_init` (a blocking roundtrip) on a helper thread, then hand over `Connection` + `EventQueue` (both Send). A stuck host must never freeze sessions.
- The server's env isn't the user's. `guilab show` should send the CLI's `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` in the request, and the server connects there (`Connection::from_socket`). Refuse sockets that belong to our own sessions (that would deadlock the thread). To test the viewer inside guilab, use a second server process.
- `hide`: drop the window, the wl_buffers, the swapchain, the source and the `Connection`. The renderer and egui `Context`/`Painter` can stay.

### Host input (sctk 0.20)
- Pointer (`PointerHandler`, includes `axis` with source/stop for touchpad scrolling), keyboard (`KeyboardHandler`, `KeyEvent{raw_code, keysym, utf8}`; needs sctk's `xkbcommon` feature), touch, `cursor_shape`, relative pointer.
- Not wrapped by sctk, bind them by hand from `wayland_protocols`: pinch/swipe/hold (`wp::pointer_gestures::zv1::client`, `get_pinch_gesture(&wl_pointer)`), `wp_fractional_scale_v1`, `wp_viewporter`. For HiDPI, render at physical size, set the viewport destination to the logical size, and use egui `pixels_per_point = scale`. winit's Wayland code (Apache/MIT) shows the pinch handling.
- sctk's `get_keyboard_with_repeat` needs a `LoopHandle<HostState>`, so do key repeat for egui ourselves with a calloop Timer.
- Routing: if egui wants the input (`ctx.wants_keyboard_input()`/`wants_pointer_input()`, or the pointer is outside the session image), feed egui. Otherwise convert pointer positions through zoom/pan to session output coordinates and inject them into the session seat. Forward keys as raw evdev codes (`raw_code`) so the session keeps its own keymap. On host keyboard leave, release every key and button forwarded to the session (stuck keys otherwise). Pinch becomes viewer zoom (egui `Event::Zoom`) and is never forwarded.

## 3. egui: egui_glow 0.35 on the renderer's own GL context
- **smithay-egui**: MIT, low activity, still on egui 0.31. It draws egui into an offscreen `TextureRenderBuffer` via `GlowRenderer` and offers no user textures. Not worth depending on. Its `src/input.rs` has a keysym-to-`egui::Key` table worth copying.
- **Version clash**: smithay 0.7 `GlowRenderer` uses glow 0.16, egui_glow 0.35 uses glow 0.17, so you can't pass smithay's glow context to egui. Make our own on the same EGL context instead (verified):
  ```rust
  let gl = renderer.with_context(|_| unsafe {
      glow::Context::from_loader_function(|s| smithay::backend::egl::get_proc_address(s)) })?;
  let mut painter = egui_glow::Painter::new(Arc::new(gl), "", None, false)?;
  ```
  egui_glow has no default features, so no winit gets pulled in.
- Session frames as egui textures, zero copy: `painter.register_native_texture(glow::NativeTexture(NonZeroU32::new(gles_tex.tex_id())?))`, then `ui.image((id, size))`. After a resize, use `replace_native_texture`. Render each session to an offscreen `GlesTexture` with smithay (`Offscreen::create_buffer` + damage tracker; the same path as screenshots) and show that texture.
- **Gotcha, verified**: smithay sets texture filters only at draw time, so a fresh `GlesTexture` has the default mipmap MIN_FILTER. It's incomplete, so egui draws it black. Set `TEXTURE_MIN_FILTER`/`MAG_FILTER` yourself, inside `with_context`, before painting. NEAREST makes sense when zoomed in.
- **Gotcha, verified**: egui_glow draws GL-style with row 0 at the bottom, while smithay's textures and dmabufs have row 0 at the top. Draw the whole viewer through egui (an egui `CentralPanel` paints over earlier smithay draws anyway) and set `wl_surface.set_buffer_transform(Flipped180)` (a vertical flip) on the host surface. Smithay-rendered textures then look right in egui images, with no uv flip.
- GL state: egui_glow changes blend, scissor, program and VAO. Paint egui last in a frame, after any smithay rendering. Smithay after egui in the same context hasn't been tested.
- Hand-built input, which is all egui needs: `egui::RawInput { screen_rect, time, modifiers, focused, events, max_texture_side: Some(painter.max_texture_side()), viewports (native_pixels_per_point), .. }`. Then `ctx.run_ui(input, |ui| ..)` (0.35 API; panels use `show_inside(ui, ..)`), `ctx.tessellate(out.shapes, ppp)`, `painter.paint_and_update_textures([w,h], ppp, &prims, &out.textures_delta)`. Events are `PointerMoved(pos in points)`, `PointerButton{pos,button,pressed,modifiers}`, `MouseWheel{unit, delta, phase, modifiers}`, `Key{key, physical_key, pressed, repeat, modifiers}`, `Text`, `Zoom`, `PointerGone`, `WindowFocused`. Handle `out.platform_output` (cursor icon via cursor_shape, copied text via the host data device) and `out.viewport_output[ROOT].repaint_delay` for scheduling.
- egui has no CPU renderer, so the viewer needs GL. Without a GPU, a Mesa software EGL device (llvmpipe) still gives a GlesRenderer.
