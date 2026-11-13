# guilab design

guilab lets humans and agents work on a GUI app together. It replaces the gui-testing skill (`~/agent-skills/gui-testing`: `guibox` wrapping headless sway + grim + wdotool). It must do everything guibox does (disposable headless sessions, screenshots, mouse/keyboard input, resize/scale, Xwayland, independent parallel sessions, automatic cleanup) as one self-contained package with no external tools. On top of that it adds a human-facing viewer with chat and drawing over screenshots. Drawing is the most important extra.

Nothing is implemented yet. See `plan.md` for build order and open research.

## Stack
- **Rust**, one `guilab` binary for both the server and the CLI. Split into workspace crates if it helps build times (see the reasonable-rust-builds skill).
- **smithay** compositor in-process. guilab *is* the compositor, so screenshots read our own buffers, input is injected directly (touch included), and frame timing is under our control.
- Reuse existing code for tricky logic (popup placement/constraints, Xwayland WM, grabs) rather than writing our own: smithay helpers first, then anvil (MIT), then cosmic-comp/niri.
- **egui** for the viewer, drawn with the same renderer as the compositor so the app's frames are just textures.
- GLES renderer on a DRM render node, pixman software fallback.
- Public repo. MIT by default; switching to GPL is fine if borrowing from cosmic-comp/niri needs it.

## Server
- Runs without any parent Wayland connection. The viewer window is optional: `--show` at startup, `guilab show`/`hide` at runtime. Opening or closing the host connection must never affect sessions.
- While the viewer is hidden, the server's own clock drives frame callbacks, so apps behave the same either way.
- Starts automatically on the first CLI command that needs it. Sessions have a TTL like guibox. Ending a session kills everything spawned in it.
- **Multiple sessions per server.** Each session has its own Wayland display socket, output, seat, Xwayland and app processes. Clients see only their own session.
- **Session resolution:** `--session NAME` > `$GUILAB_SESSION` > the session created from the cwd's git worktree root > the only session, if there's exactly one. A new session defaults to the worktree's name. Goal: one agent per worktree, each with its own session, and no naming needed.

## IPC
- One control socket per server, under `$XDG_RUNTIME_DIR/guilab/`, using line-delimited JSON requests and responses. Each request names its session.
- The CLI is the only client. No MCP.

## Locking
- An agent must `guilab lock` before anything that changes a session (input, output size, run/kill). Lock returns a token, and those commands require it. One holder per session, so two agents can't drive the same session.
- Read-only commands (screenshot, tree, log, poll) need no lock.
- While locked, the human's input to that session is blocked by default. They can still view, screenshot, draw and send messages.
- The viewer has two buttons:
  - **Revoke:** drops the agent's lock. Its token stops working until it locks again.
  - **Share:** unblocks the human's input while the agent keeps its lock. Both act at once.

## Event log
- Each session has an append-only log of everything: human input, agent commands, app start/exit, messages, screenshots. Pointer motion is coalesced.
- Human actions since the agent's last command are attached to the agent's next response (any command or poll message), so the agent knows what changed under it.
- Frames are not persisted by default and there's no scrubbing UI. The log is designed so frame history could be added later.

## Messaging
- `guilab poll` blocks until the next human message, prints it and exits. `--follow` streams one line per event instead, for Monitor-style tools.
- A message has text, image attachments (written to files, path given), and the human's actions since the agent's last command.
- `guilab say` posts agent → human messages (text, optional images) into the viewer's chat.

## Viewer
- Tabs, one per session.
- The viewer's size never changes the session's output size; the output size changes only via the CLI or an explicit control. Scroll to zoom, drag to pan, touchpad pinch/pan.
- The human uses the app directly through the viewer, unless locked out.
- Chat panel per session.
- Drawing: when the human starts drawing, the server grabs a screenshot (whole output, no cursor) and the drawing goes over it. Sending attaches the composited image.

## Screenshots
- Agent screenshots take options: target (output or surface), cursor on/off, and settle-wait on/off. The default waits for the app to finish drawing, includes no cursor, and captures the whole output.
- Human screenshots are always whole output with no cursor, taken when the action triggers them.

## CLI
- Built for agents first. No compatibility with xdotool/wdotool. Ship a short skill doc.
- **Input data is positional args and flags, never JSON**, so it's terse and needs no quoting. Input actions chain within one command (like wdotool) so held buttons and modifiers work.
- Output is compact text made for agents to read. `--json` gives structured output for scripts.
- Errors are short and say what to do.
- Rough command set (to be refined during implementation):
  - server: `server [--show]`, `show`, `hide`
  - sessions: `new [NAME] [-s WxH] [--scale S] [-x]`, `end`, `ls`
  - apps: `run -- CMD...`, `kill [ID]`, `ps` (with exit status). No restart or watch mode: `build && guilab kill && guilab run ...` is enough.
  - control: `lock`, `unlock`, `output --size WxH --scale S`
  - input chain: `input [--surface ID] ACTION...`. Actions: `move X Y`, `down/up/click [BTN]`, `scroll DX DY`, `key CHORD`, `keydown/keyup KEY`, `type TEXT`, `touch down|move|up ID [X Y]`, `on SURFACE` (switches coordinates to that surface's local space), `sleep MS`
  - inspect: `tree` (toplevels, popups, subsurfaces, X windows: IDs, app_id/title, geometry), `screenshot [-o PATH] [--surface ID] [--cursor] [--no-settle]`, `log`
  - clipboard: `clip get`, `clip set`
  - messaging: `poll [--follow]`, `say TEXT [--image PATH]`

## Later
- Private D-Bus per session (portals, AT-SPI accessibility tree as a text view of the UI).
- Frame history and scrubbing.
