# Initial version plan

Each step ends usable and tested. Record research findings in notes, then delete this file once v1 is done.

## 0-2. Done
Research (notes `smithay.md`, `host-and-xwayland.md`), skeleton (workspace, daemon, autostart, JSON socket, session resolution, TTL, cleanup) and headless compositor (per-session display and nested loop, xdg-shell with the window policy, popups, frame clock, dmabuf, pixman fallback, `run`/`kill`/`ps`/`tree`/`screenshot`/`output`). Tested against GTK4, Qt6 (PySide6), kitty, weston demos, in parallel sessions; see `code-map.md`.

## 3. Input and locking
`lock`/`unlock` with tokens, the `input` action chain (pointer, buttons, scroll, keys, type, touch, surface-local coordinates), clipboard.
Also test with real input what step 2 couldn't: popup grabs (click outside dismisses the chain), a Qt `QMenu` (Qt opens none without an input serial: `tests/it/apps/qt_app.py --menu`), GTK context menus, focus moving between floating windows.

## 4. Xwayland
Per-session Xwayland with `-x`, X windows in `tree`, input to them.

## 5. Event log
Per-session log, wake/passive tiers, motion coalescing, passive events attached to the next delivery, `log`.

## 6. Viewer
`show`/`hide`/`--show`, session tabs, zoom/pan/pinch, human input forwarding, lock UI (revoke/share). Test the viewer by running it inside a guilab session (or with the gui-testing skill before that's possible).

## 7. Chat and drawing
`poll`/`poll --follow`/`say`, bottom feed (messages + agent command log), drawing toolbar, queued screenshots. Run the sub-agent test to pick how annotation text is delivered.

## 8. Ship
Skill doc. Port guibox's use cases as integration tests (GTK, Qt, a browser, an X11 app). Then replace the gui-testing skill.
