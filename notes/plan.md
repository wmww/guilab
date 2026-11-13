# Initial version plan

Each step ends usable and tested. Record research findings in notes, then delete this file once v1 is done.

## 0. Research (before writing code)
Find out what smithay (and cosmic-comp/niri/anvil) already provide, so we don't write tricky logic ourselves:
- xdg popups: positioner constraints, grabs, PopupManager
- Xwayland: smithay's X11Wm. What anvil/cosmic-comp do around it (override-redirect windows, menus, focus)
- headless GLES on a render node with no KMS; pixman fallback; dmabuf support for clients
- several `Display`s in one calloop event loop (one per session)
- presenting to a host compositor on demand without smithay's winit backend: sctk client plus our own buffers (dmabuf or shm)
- egui on smithay's GlesRenderer (state of the smithay-egui crate, or egui_glow directly)
- virtual keyboard (xkb keymap), pointer, touch and gestures on a smithay seat
- detecting "app finished drawing" for settled screenshots
- licenses of anything we'd copy

## 1. Skeleton
Cargo workspace, `guilab` binary, server daemon with autostart, control socket with the JSON protocol, session resolution, `new`/`end`/`ls`, TTL, cleanup.

## 2. Headless compositor
Per-session display, xdg-shell, single tiled toplevel filling the output, popups, own frame clock, `run`/`kill`/`ps`, `tree`, `screenshot` (PNG, settle wait), `output` resize/scale.

## 3. Input and locking
`lock`/`unlock` with tokens, the `input` action chain (pointer, buttons, scroll, keys, type, touch, surface-local coordinates), clipboard.

## 4. Xwayland
Per-session Xwayland with `-x`, X windows in `tree`, input to them.

## 5. Event log
Per-session log, wake/passive tiers, motion coalescing, passive events attached to the next delivery, `log`.

## 6. Viewer
`show`/`hide`/`--show`, session tabs, zoom/pan/pinch, human input forwarding, lock UI (revoke/share). Test the viewer by running it inside a guilab session (or with the gui-testing skill before that's possible).

## 7. Chat and drawing
`poll`/`poll --follow`/`say`, chat panel, drawing on screenshots, attachments.

## 8. Ship
Skill doc. Port guibox's use cases as integration tests (GTK, Qt, a browser, an X11 app). Then replace the gui-testing skill.
