# Host viewer window and egui

Research results for plan step 6. Versions checked: smithay 0.7.0 (crates.io) and git main (we now use the pinned master rev; APIs below may have shifted slightly), sctk 0.20.0, egui/egui_glow 0.35.

## Licenses
- MIT, free to copy: smithay and anvil, smithay-egui, sctk, calloop-wayland-source. egui is MIT/Apache.
- GPL-3.0: cosmic-comp, niri.
- Nothing below needs GPL code. The cosmic-comp logic worth having is a few lines each, so reimplement it rather than copy.

## 1. Host viewer window: sctk on its own connection, presenting dmabufs from the headless renderer
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

## 2. egui: egui_glow 0.35 on the renderer's own GL context
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
