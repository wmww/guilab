# Viewer cursors

Spec from the user. Most of it already works; the missing part is the app's own cursor.

| Pointer over | Cursor |
|---|---|
| guilab's own UI | normal egui cursor (arrow, text beam, …): works |
| the app, input blocked (locked, not shared) | "not allowed": works |
| the app, input goes through | **whatever the app sets**, through the session's compositor. Currently it falls back to the arrow |
| the app, but a tool won't send input (drawing, moving, frozen screenshot) | a tool cursor (crosshair, move, grab…), **never the arrow**, since the app's own cursor is usually an arrow and that would look like input goes through |

App cursor forwarding:
- Apps set cursors with `wp_cursor_shape_v1` (a named shape) or `wl_pointer.set_cursor` (a surface), or hide the cursor (null surface).
- Named shapes map to the host's cursor-shape protocol if it has one, else to egui's `CursorIcon`.
- Surface cursors: render the cursor surface and set it as the host cursor with the right hotspot and scale. Don't apply the viewer's zoom, since real cursors aren't zoomed.
- Hidden app cursor → hide the host cursor over the app.
- Follow cursor changes live, including animated cursors if cheap.
