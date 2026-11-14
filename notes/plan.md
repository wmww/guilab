# Initial version plan

Each step ends usable and tested. Record research findings in notes, then delete this file once v1 is done.

## 0-6. Done
Research (notes `smithay.md`, `viewer.md`), skeleton (workspace, daemon, autostart, JSON socket, session resolution, TTL, cleanup), headless compositor (per-session display and nested loop, xdg-shell with the window policy, popups, frame clock, dmabuf, pixman fallback, `run`/`kill`/`ps`/`tree`/`screenshot`/`output`), input and locking (`lock`/`unlock`, the `input` chain, `clip`, DnD with icon). Xwayland (`new -x`: X11Wm, X window policy, menus, typing, clipboard bridging, cleanup). Event log (`log`, tiers, coalescing, deliveries on every reply, `poll`/`--follow`/`say`, `debug` as the human's stand-in). Viewer window (`show`/`hide`/`server --show`, tabs, zoom/pan/pinch, input forwarding, lock bar; dmabuf and shm presentation). Tested against GTK4, Qt6 (PySide6, also xcb), GTK3 (x11), Tk, xeyes/xclock, Chromium (x11), kitty, weston demos, in parallel sessions; see `code-map.md`.

## 7. Chat and drawing
Bottom feed (messages, `say`, agent commands: all in the session log), sending messages (replace `HumanOp::Message` from `guilab debug`), drawing toolbar, queued screenshots. Run the sub-agent test to pick how annotation text is delivered.

## 8. Ship
Skill doc. Port guibox's use cases as integration tests (GTK, Qt, a browser, an X11 app). Then replace the gui-testing skill.
