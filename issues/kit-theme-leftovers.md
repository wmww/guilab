# Theme isn't yet the single source, and kit/editor still assume guilab

After phase 6, before or as part of `ui-toolkit-pass.md`. design.md: "Widgets never hardcode them". kit/lib.rs and Cargo.toml claim "no app types, so other egui apps can use it".

## Hardcoded visual values left
- kit `widgets.rs`: swatch hover +1 and selected ring +3 / width 2.0 (:292, :296); send arrow width 2.0 (:323), and its pressed fill equals its rest fill (:308-314, so there's no pressed state); close-badge × stroke 1.5 (:345); tab accent line 2.0 (:365); window-button glyph half-size 5.0 (:428); toast margin `md + 2.0` (:263).
- kit `menu.rs`: radio/check geometry 6.0 / 3.5 / 12.0 / 1.8 (:315-329), custom-row gap `sm + 2.0` (:373).
- kit `theme.rs` `style()`: hovered/active fg stroke 1.5 / 2.0, `interact_size`, `tooltip_width`, shadow offset/blur (:452-487). These are in the theme module but aren't tokens.
- editor: hover halo width 3.0 and outline 2.0 (canvas.rs:570, 615); key point hot +1.0, outline radius 2.0 (paint.rs:494, 501); the "missing asset" box uses literal grays (paint.rs:427-430); text field width fudge `1.02 + 2.0` (text.rs:189); tool icons use absolute point offsets inside the icon rect (toolbar.rs:355-401), so changing `tool_button`/`tool_icon_pad` breaks them. Draw them in unit coordinates scaled to `r`.
- editor: double-click is its own 0.35 s / 6 pt (canvas.rs:286). Use egui's `InputOptions` (`max_double_click_delay`, `max_click_dist`) so it matches every other egui widget.
- Stale comments: `Colors` "Editor (later phases)" (theme.rs:56), `Sizes` "(later phases)" (:134), and `thumb_h` "queued drawings" (:112; the queue goes away in phase 6).

## Hidden guilab assumptions (reusability)
- `send_button`'s disabled tooltip is guilab copy ("Type a message or draw something to send", widgets.rs:329). Take the tooltips as parameters.
- `theme::install` installs only guilab's values (theme.rs:511). Add `install_with(ctx, dark: Theme, light: Theme)` so another app can supply tokens. App-specific groups (`me`/`agent`, `slot_*`, `crop_*`, `key_point*`, `overlay`) are fine as fields, but should be documented as "used by the editor/chat/viewer".
- The editor toolbar always shows all `Tool::ALL`, including `Interact` labelled "Use the app" (editor/lib.rs:41). Let the host pass the tool list and the Interact label (a standalone image editor has no app).
- `guilab_editor::demo` and `guilab_kit::headless` are public library modules. That's acceptable for test hosts (the demo's pictures go through `load_bytes`).
