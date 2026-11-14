# Viewer window: how it works and what to know

Built in plan step 6 (`crates/viewer`, `crates/server/src/viewer.rs`). Versions: sctk 0.20, egui/egui_glow 0.35 (glow 0.17), smithay pinned master.

## Shape
- Our own sctk client on a separate `Connection` (not smithay's winit backend: its renderer dies with the window, and winit refuses a second `EventLoop` per process, so show/hide/show would be impossible).
- The one `GlesRenderer` (render node, GBM `EGLDisplay`) draws sessions and the viewer. Hiding drops the window, connection, buffers, egui context and painter; the renderer stays.
- `connect` (socket connect + registry roundtrip) runs on a helper thread; the result comes back over a calloop channel tagged with an attempt number, so stale results after hide/show are ignored. A 10 s deadline fails the pending `show`.
- `source.rs` replaces calloop-wayland-source: that one returns host I/O/protocol errors from `EventLoop::dispatch`, which would stop the server. Ours reports `HostEvent::Lost` once and removes itself; the server then hides the viewer. Note `Connection::protocol_error()` is checked after dispatch (a protocol error leaves nothing to read).
- Drawing happens from the server tick, only when the host is ready (configured, frame callback back) and something changed: host input, resize/scale, egui's repaint request, or the signature of the shown sessions (names, lock states, sizes, cursor, the current session's `frame_seq`). New input runs the UI even while waiting for a frame callback (so actions apply without waiting); egui texture deltas from frames that couldn't be presented are kept for the next one. GTK4's fading text caret makes a focused entry commit continuously, so the viewer redraws at the frame rate then; that's the app, not us.

## Presentation (host.rs)
- dmabuf: `Swapchain<GbmAllocator>` (ARGB8888) with modifiers from the host's default `zwp_linux_dmabuf_feedback_v1` format table ∩ `egl_context().dmabuf_render_formats()`, only if the feedback's main device is our GPU (render and card nodes compared via `/sys/dev/char/M:m/device`). Each slot caches its `wl_buffer` (`create_immed`) in userdata, destroyed with the slot; slots stay busy until `wl_buffer.release`.
- shm fallback (no v4 feedback within 500 ms, other GPU, no common modifier, or `GUILAB_VIEWER_SHM=1` in the server's env): render to an offscreen `GlesTexture`, `copy_framebuffer` + `map_texture`, swizzle RGBA → BGRA into an sctk `SlotPool` buffer (max 3).
- egui_glow draws GL-style (row 0 at the bottom); the surface has `set_buffer_transform(Flipped180)` (a vertical flip). Both paths share the same memory layout, so the same transform works.
- HiDPI: `wp_fractional_scale_v1` + `wp_viewporter` when available (buffer = logical × scale, viewport destination = logical size), else integer `wl_surface` buffer scale. egui `pixels_per_point` = scale.
- egui is painted inside a smithay frame (`frame.with_context`), after its clear. Afterwards `Gfx::paint` resets the GL state egui leaves (scissor, program, VAO, buffers, texture, blend func) so smithay's next session render isn't affected.

## egui on smithay's context (gfx.rs)
- smithay's glow (0.16) and egui_glow's (0.17) differ, so we make our own `glow::Context` on the same EGL context via `smithay::backend::egl::get_proc_address`.
- **egui_glow deletes every texture registered with it** when freed or replaced (`free_texture`, `replace_native_texture`, `destroy`). So session textures are never registered directly: each is a "mirror", an egui-owned texture that `glBlitFramebuffer` fills from the smithay texture when the session's `frame_seq` changed (only the current tab's). Mirror filter: NEAREST at ≥ 2 host pixels per session pixel, else LINEAR.
- egui debug builds outline unaligned widgets in red (`style.debug.show_unaligned`); we turn that off.
- `ctx.run_ui` may run several passes; the UI consumes host input only in the first (it's moved into the closure's first call).

## Input (host.rs, ui.rs)
- Host input is one ordered list (`HostInput`: egui events and raw evdev keys), so "click, type, click" stays in order when one frame has all of it. egui gets the egui events (minus Key/Text when keys go to the session); the UI walks the full list.
- Keys are forwarded as raw evdev codes (+8 = xkb code), so the session keeps its own keymap. Modifiers reach egui via `update_modifiers`.
- The UI tracks the buttons and keys it pressed in a session and releases them on tab switch, lock (blocked), keyboard leave and hide.
- Pinch (`zwp_pointer_gesture_pinch_v1`, bound by hand) becomes egui `Zoom`; never forwarded. Cursor: the session's named cursor (`CursorImageStatus::Named`) maps to egui `CursorIcon` and then to `wp_cursor_shape_v1`.
- Wheel vs touchpad: axis events with value120/discrete or source Wheel become egui `Line` units (notches), else `Point`. Forwarded wheel = 15 logical px per notch (like agent `scroll`); smooth scroll is scaled so content follows the fingers.

## Testing
- Host = a second guilab server's session (tests: `tests/it/viewer.rs`; it pins the coordinate mapping by moving the host pointer and reading the viewed session's human motion event). Lab commands drop `WAYLAND_DISPLAY`, so a test can never open on the real desktop.
- Manually: `XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR/glv-h guilab new host -s 1400x900`, then `guilab show` with `WAYLAND_DISPLAY=<that session's socket>` against another runtime dir, drive the host session with `input`, screenshot it.
- The real desktop's `WAYLAND_DISPLAY` is usually set in agent shells; never run `guilab show` without overriding it.
