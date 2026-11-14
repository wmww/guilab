# guilab design

guilab lets humans and agents work on a GUI app together. It replaces the gui-testing skill (`~/agent-skills/gui-testing`: `guibox` wrapping headless sway + grim + wdotool). It must do everything guibox does (disposable headless sessions, screenshots, mouse/keyboard input, resize/scale, Xwayland, independent parallel sessions, automatic cleanup) as one self-contained package with no external tools. On top of that it adds a human-facing viewer with chat and drawing over screenshots. Drawing is the most important extra.

Plan steps 1-5 (skeleton, headless compositor, input and locking, Xwayland, event log) are built; see `plan.md` for what's next and `code-map.md` for the code.

## Stack
- **Rust**, one `guilab` binary for both the server and the CLI. Split into workspace crates if it helps build times (see the reasonable-rust-builds skill).
- **smithay** compositor in-process. guilab *is* the compositor, so screenshots read our own buffers, input is injected directly (touch included), and frame timing is under our control.
- Reuse existing code for tricky logic (popup placement/constraints, Xwayland WM, grabs) rather than writing our own: smithay helpers first, then anvil (MIT), then cosmic-comp/niri.
- **egui** for the viewer, drawn with the same renderer as the compositor so the app's frames are just textures.
- GLES renderer on a DRM render node, pixman software fallback for sessions. The viewer always needs GL (egui has no software renderer); Mesa llvmpipe covers GPU-less machines.
- Details and verified APIs: `smithay.md` (core compositor), `viewer.md` (viewer window, egui).
- Public repo. MIT by default; switching to GPL is fine if borrowing from cosmic-comp/niri needs it.

## Server
- Runs without any parent Wayland connection. The viewer window is optional: `--show` at startup, `guilab show`/`hide` at runtime. Opening or closing the host connection must never affect sessions. Host disconnects or protocol errors close the viewer only, and never stop the server. `show` sends the CLI's own `WAYLAND_DISPLAY`, since the auto-started server's environment may be stale.
- While the viewer is hidden, the server's own clock drives frame callbacks, so apps behave the same either way.
- Starts automatically on the first CLI command that needs it, and then exits after 10 minutes without sessions. `guilab server --stop` stops it (ending all sessions).
- Sessions end after an idle TTL (default 30 minutes without commands targeting them; `new --ttl SECS`, 0 = never). Ending a session kills everything spawned in it, including double-forked daemons.
- Each session has its own dir, which is also its apps' `XDG_RUNTIME_DIR` (Wayland socket, app logs, screenshots). Apps don't see the user's D-Bus, audio or compositor.
- **Multiple sessions per server.** Each session has its own Wayland display socket, output, seat, Xwayland and app processes. Clients see only their own session. Each session is its own smithay state type (`Display<Session>`, handlers on `Session`), not one shared state with lookups, and has its own calloop loop (data = the session) nested in the server's loop, which is what X11Wm and smithay's per-display getters need.
- **Window policy:** the first toplevel fills the output (tiled states, no decorations). Dialogs (with a parent) and later toplevels float centered on top. X windows follow the same rule (transient, modal or non-Normal window types float); X override-redirect windows (menus, tooltips) go where X puts them, above everything.
- **Xwayland** (smithay's X11Wm) is opt-in per session: `new -x` starts it and replies once it's ready; apps then get `DISPLAY`. Not on by default: it costs startup time and memory, and with `DISPLAY` set, apps that support both may silently pick X11 when the agent meant to test Wayland. Not lazy (spawn on first X connection): smithay's `XWayland::spawn` owns the listening sockets, so that would mean reimplementing it. Display numbers come from `:100` up (smithay alone stops at `:32` machine-wide).
- X apps render at scale 1 and are scaled up on HiDPI outputs (blurry, but the right size; core-X apps ignore XSETTINGS DPI anyway). X clients may always read the clipboard (no focus check): sessions are disposable and scripts like `xsel` have no window.
- Key repeat is off by default, so a held key in an agent input chain doesn't auto-repeat.
- Floating windows always stack above tiled ones, and dialogs above their parents; clicking a window focuses and raises it within that order.
- **Session resolution:** `--session NAME` > `$GUILAB_SESSION` > the session created from the cwd's git worktree root > the only session, if there's exactly one. A new session defaults to the worktree's name. Goal: one agent per worktree, each with its own session, and no naming needed.

## IPC
- One control socket per server, under `$XDG_RUNTIME_DIR/guilab/`, using line-delimited JSON requests and responses. Each request names its session.
- The CLI is the only client. No MCP.

## Locking
- `guilab lock` returns a short random token. One holder per session, so two agents can't drive the same session. `lock` again with the token returns the same token; `lock --force` takes over (the old token stops working); `unlock` releases everything still held (keys, buttons, touches).
- `input` always needs the lock (it competes with the human's input). Other changing commands (run, kill, output changes, clip set, end) need the token only while the session is locked. Read-only ones (screenshot, tree, ps, clip get, log, poll) never do. Why not lock for everything: the build/kill/run loop and scripts on an unlocked session stay token-free, and run/kill don't fight the human's input. A stale or revoked token is always an error, so an agent learns it lost the lock.
- Token passing: `-t TOKEN` on any command (agent shells don't keep env between calls) or `$GUILAB_TOKEN`. Errors say exactly what to run.
- While locked, the human's input to that session is blocked by default. They can still view, screenshot, draw and send messages.
- The viewer has two buttons:
  - **Revoke:** drops the agent's lock. Its token stops working until it locks again.
  - **Share:** unblocks the human's input while the agent keeps its lock. Both act at once.

## Event log
- Each session has an append-only, timestamped (unix ms), numbered (`seq`) log: agent commands with args, human input, app start/exit (code, signal, after `kill` or not), window open/close/title (toplevels and X windows; diffed each frame), output changes, lock/unlock/revoke/share, human messages, `say`, screenshots, the expiry warning. In memory, capped at 100k events (oldest dropped); it dies with the session.
- Human input is logged per event, with motion coalesced into one event per run (`path` of `[x, y, ms]`, new run after a 500 ms gap or any other event) and consecutive scrolls summed. Agent input is logged only as its `input ...` command.
- Three tiers:
  - **Wake:** ends a blocking `poll` (or emits a `--follow` line). Defaults: human message, lock revoked, app exited or crashed on its own, session about to expire (warned `min(60 s, ttl/2)` before; a waiting poll doesn't count as activity, so the warning reaches it and the agent can keep the session alive).
  - **Passive:** rides along with the next delivery. Defaults: human input, windows, output, app start, app ended by `guilab kill`, lock/unlock/share.
  - **Log:** only in `guilab log`: commands, `say`, screenshots (the agent's own actions).
- A delivery is a poll result or the reply to any agent command (not `log`, `poll` itself or the human's actions). It carries every non-log event since that client's previous delivery, except events its own commands caused (each event records the delivery key that caused it; e.g. its own `run` doesn't come back as "app started"). Replies print them on **stderr** (`[guilab events since your previous guilab command at HH:MM:SS; not errors]`), so stdout stays parseable; `--json` puts them as one JSON line on stderr.
- **Delivery keys:** the lock token if the command has one, else one shared anonymous key per session. A new lock token starts at the end (it has seen everything); an unknown token starts at the anonymous position. 32 keys kept per session, least recently used dropped.
- **Several pollers on one key:** each wake event reaches all of them (a poll fires on any wake event after it started, even if a sibling already consumed it); passive events go to whichever delivery comes first. Two agents sharing a session without tokens therefore split passive events between them; use tokens to keep them apart.
- `poll --wake`/`--passive KINDS` override tiers for that poll only (kind or group names: `input`, `window`, `app`, `all`). Use this tier split anywhere new event kinds appear, rather than ad-hoc rules.
- Text output (`proto/src/text.rs`) is written for agents reading it cold and was checked with context-free sub-agents: chronological, one line per event, human input runs merged into one line of phrases (`moved pointer A → B (0.8s); double-clicked at X,Y in window 7; typed "hi"; pressed ctrl+s; dragged from A to B`), messages in full with image paths, annotations and a `guilab say` hint.
- Frames are not persisted and there's no scrubbing UI. Frame history would add a `frame` event kind (seq + file path or ring-buffer index) to the same log; nothing else assumes events are small.

## Messaging
- `guilab poll [--timeout SECS]` blocks until a wake event, prints it and everything else since the previous delivery, and exits 0 (2 on timeout, still printing what happened). `--follow` keeps going with one line per wake event (passive events appended as `(meanwhile: ...)`), for Monitor-style tools.
- A human message has text, image attachments (copied into the session dir as `msg-SEQ-N.EXT`, path given) and text annotations (`{text, image?, x?, y?}`, positions in image pixels). How the viewer delivers annotation text (baked into the image, as data, or both) is decided in step 7; the event carries both kinds.
- `guilab say TEXT [--image PATH]` logs an agent → human message (images copied as `say-SEQ-N.EXT`) for the viewer's feed.
- Until the viewer exists, the hidden `guilab debug message|input|revoke|share` acts as the human (tests use it). Human input goes through `Session::inject(.., InputSource::Human)`, which logs it; it's refused while locked and not shared.

## Viewer
Start with basic functionality. Don't build UI or features we aren't sure we need.

Layout:
- **Top:** one tab per session.
- **Main area:** the session. The viewer's size never changes the session's output size; that changes only via the CLI or an explicit control. Scroll to zoom, drag to pan, touchpad pinch/pan. The human uses the app directly here, unless locked out. Lock controls (revoke/share) live here too.
- **Left edge:** a narrow drawing toolbar for diagramming, not art. One fixed-width pen, box, ellipse, line, arrow, text (as a tool or by double-click), and a few colors. Nothing more.
- **Bottom:** the message input, plus a feed of the conversation: human messages, agent `say` output, and each agent command as a log line, so you can see what the agent is doing. A drag handle resizes it.

Drawing flow:
- Starting to draw (any tool) freezes a screenshot of the session (whole output, no cursor) and the drawing goes over it.
- "Queue" shrinks it to a thumbnail next to the input box. A queued thumbnail can be deleted. All queued screenshots go with the next message.
- Sending a message while a drawing is still open sends that drawing with it.
- Keep annotation + queuing (and the chat feed, if it fits naturally) in its own crate(s), easy to copy into another egui app: depends only on egui and image types, takes a screenshot in, gives annotated images, text annotations and queue events out. No compositor, IPC or session types.
- Each screenshot reaches the agent as one composited image file. Text annotations either get baked into the image, or sent as text with a position, or both. Decide by testing: give fresh, context-free sub-agents each format and see which they understand better.

## Screenshots
- Agent screenshots take options: target (output or surface), cursor on/off, and settle-wait on/off. The default waits for the app to finish drawing, includes no cursor, and captures the whole output.
- Human screenshots are always whole output with no cursor, taken when the action triggers them.

## CLI
- Built for agents first. No compatibility with xdotool/wdotool. Ship a short skill doc.
- **Input data is positional args and flags, never JSON**, so it's terse and needs no quoting. Input actions chain within one command (like wdotool) so held buttons and modifiers work.
- Output is compact text made for agents to read. `--json` gives structured output for scripts.
- All coordinates agents see or give are output pixels, i.e. screenshot pixels, at any scale.
- Errors are short and say what to do.
- Rough command set (to be refined during implementation):
  - server: `server [--show] [--stop] [--idle-exit SECS]`, `show`, `hide`
  - sessions: `new [NAME] [-s WxH] [--scale S] [--ttl SECS] [-x]`, `end`, `ls`; `-S NAME` / `--session NAME` on any command
  - apps: `run -- CMD...`, `kill [ID]`, `ps` (with exit status). No restart or watch mode: `build && guilab kill && guilab run ...` is enough.
  - control: `lock`, `unlock`, `output [-s WxH] [--scale S]` (no args: show)
  - input chain: `input ACTION...`, grammar and docs in `guilab input --help` (`proto/src/input.rs` `HELP`): `move X Y [MS]` (MS = glide), `down/up [BTN]`, `click [BTN] [N]`, `scroll DX DY` (wheel notches), `scrollpx DX DY` (touchpad), `key CHORD`, `keydown/keyup KEY`, `type TEXT`, `touch down|move|up ID [X Y] [MS]`, `tap X Y`, `pinch SCALE [DEG]`, `swipe DX DY [FINGERS]`, `on ID|output` (surface-local pixels from the surface's `tree` origin; events still go to whatever is on top), `sleep MS`. Chains are parsed and key names checked before anything runs. Sleeps don't block the server; a session's chains run one at a time in order. Replies with the pointer position, surface under it and keyboard focus.
  - inspect: `log [--since 5m|14:05] [--after SEQ] [--kind K,..] [-n N]` (default newest 100), `tree` (toplevels topmost first, with popups and subsurfaces nested; X windows as `x11` / `x11_popup` with xid, class, title, `_NET_WM_PID`; all with ids and pixel geometry), `screenshot [-o PATH] [--surface ID] [--cursor] [--no-settle]` (default path: a new file in the session dir)
  - clipboard: `clip get`, `clip set [TEXT]` (stdin without TEXT); text only, clipboard selection only (no primary)
  - messaging: `poll [--follow] [--timeout SECS] [--wake K] [--passive K]`, `say TEXT [--image PATH]`

## Later
- Private D-Bus per session (portals, AT-SPI accessibility tree as a text view of the UI).
- Frame history and scrubbing.
