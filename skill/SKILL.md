---
name: guilab
description: Run a Linux GUI app (Wayland or X11) in a private headless session, take screenshots, send mouse/keyboard/touch input, and talk with a human who watches, uses and draws on the app in a viewer window. Use to test or iterate on an app's rendering and interaction.
---

# guilab

`guilab` runs GUI apps in private headless sessions. Each session has its own Wayland display, so several agents can each work in their own session at once. A human can open a viewer window to watch sessions, use the apps, draw on screenshots and send you messages.

Requires the `guilab` binary on PATH (`cargo install --path crates/guilab` from the guilab repo). `-x` also needs `Xwayland`. If something is missing, ask the user to install it; don't fall back to their live desktop.

## Basics

```sh
guilab new                          # session named after this git worktree (-s 1280x720, --scale 2, -x for X11 apps)
guilab run -- ./target/debug/myapp  # start the app inside it; its log path: guilab ps
guilab screenshot                   # waits for apps to finish drawing, prints the PNG path: read it (--cursor: draw the mouse cursor in)
guilab tree                         # windows/popups: ids, app_id, title, pixel geometry
guilab work "testing the login form"   # shown live to the human; needed before lock/input
guilab lock                         # input needs the lock; prints a token, e.g. gfm7b785
guilab -t gfm7b785 input move 200 150 click type 'hello' key Return
guilab done "Login works now, have a look"   # message the human, release the lock
guilab poll --timeout 600           # wait for their reply
guilab end                          # kill everything in the session
```

- Commands pick the session automatically: `-S NAME`, else `$GUILAB_SESSION`, else the one started from this worktree, else the only one. Outside git with several sessions, pass `-S` every time. The server starts itself.
- Screenshots go in the session dir, which is deleted by `end`. Use `screenshot -o PATH` for anything you want to keep.
- `new` prints the session's `WAYLAND_DISPLAY`. You only need it to run a client by hand; `guilab run` sets it up for you.
- Coordinates everywhere (screenshots, `tree`, `input`) are screenshot pixels, at any scale.
- Shells don't keep variables between your tool calls, so pass the literal token with `-t` on every command that needs it. While the session is locked, `run`, `kill`, `output`, `clip set` and `end` need it too. Read-only commands never do. If your token stops working, the human revoked it: run `poll` or ask before you `lock` again. Lost your token? `guilab done` releases the lock without it.
- After a rebuild: `guilab kill && guilab run -- CMD` (`kill` returns once the app is gone).
- A window can be bigger than the output (apps have minimum sizes); `screenshot` then says it's cut off. Make the output bigger with `guilab output -s WxH`.
- Each session ends after 30 idle minutes. Don't end sessions you didn't start.

## Input

Actions chain in one `input` command, so held buttons and keys carry across them. `guilab input --help` lists them all. Common ones:

```sh
input move X Y [MS]  click [right|middle] [2]  down / up  scroll 0 3  scrollpx 0 120
input key ctrl+s  keydown shift / keyup shift  type 'any unicode →'
input tap X Y  touch down 1 X Y  touch move 1 X Y 200  touch up 1  pinch 2  swipe 300 0
input on 7 move 10 10 click      # coordinates relative to surface 7 from `guilab tree`
input click right sleep 300 move 230 180 click    # give menus time to open
```

Key repeat is off. `scrollpx` distances get scaled by toolkits (GTK scrolls about 2.5x). After input, take a screenshot to see the result.

## Other controls

`guilab -t TOKEN output -s 800x600 --scale 1.5` resizes live (the token is needed only while locked). `guilab clip get` / `clip set TEXT` reads or sets the clipboard. `guilab log` shows the session's full history. Every command has `--help` and `--json`.

## Working with a human

The human runs `guilab show` to open the viewer. There they can watch, use the app when you don't hold the lock (or when they press Share), revoke your lock, and send messages with screenshots they drew on. The viewer always shows whether you're working (and on what) or waiting for them.

- `guilab work "STATUS"` marks you working, with STATUS shown live (e.g. "rebuilding after the toolbar fix"). Run it again to update it. Add `-m "MSG"` (and `--image PATH`) to also post a message while you keep working. `lock` and `input` need it.
- `guilab done "MSG"` posts MSG (what you did, or what you need from them), releases your lock and shows the human you're waiting for them. Then `guilab poll`. It refuses while a human message is unread. Not needed before `end`.
- `guilab poll --timeout 600` waits for the next message, revoke, app exit or session expiry. It prints that plus everything else that happened. It exits 2 on timeout. A blocking poll needs `done` first. `poll --follow` prints one line per event, for tools that watch output (allowed while working).
- When a human message arrives, every command says so on stderr (`[guilab: a human message waiting since ...; read it now: guilab poll]`), until you run `guilab poll`, which then returns it at once, even while working. Only `poll` shows messages. Reading one marks you working ("read your message"), so finish with `done`.
- Messages may come with image files. Shapes (boxes, arrows) are drawn into the image. Text notes are **not** in the image; they arrive as lines like `red text annotation on image 1 at pixel 640,410: "too big"`. Match each note to the nearby mark of the same color.
- Short messages refer to what the human is looking at: read the human input lines printed with them (where they clicked, where the pointer rests) and take a screenshot.
- Every command prints, on stderr, what happened since your previous command (human clicks and typing, windows opening, apps exiting). Those lines are context, not errors.

## Testing guilab itself (nested)

Give the inner server its own directory with `GUILAB_DIR`, and set `GUILAB_NO_AUTOSTART=1` when driving it, so a command sent while it restarts fails instead of starting a stray server:

```sh
guilab run -- env GUILAB_DIR=/run/user/$UID/gl-inner /path/to/guilab server --show   # inner server + viewer as an outer app
export INNER="env GUILAB_DIR=/run/user/$UID/gl-inner GUILAB_NO_AUTOSTART=1"
$INNER guilab server --wait && $INNER guilab new demo && $INNER guilab run -- gtk4-demo
guilab screenshot                   # the outer session shows the inner viewer
```

- `guilab ls` marks the session the viewer shows; `guilab -S NAME show` switches it.
- Messages typed into the inner viewer go to the inner session: poll it too (`$INNER guilab poll`).
- Rebuilding replaces the binary the outer server runs; it keeps running the old code (commands warn about it) until `guilab server --stop`, which ends its sessions. So fixes that need the outer guilab to change (as a compositor) can't be seen in the inner viewer until then.

## Troubleshooting

- No window? Check the app's log (`guilab ps` gives the path), then `guilab tree`.
- Screenshots wait up to 3 s for drawing to stop, and for the window of an app you just `run`. Apps that keep animating (spinners, video) are detected after about a second and shot once your input had 0.3 s to land ("the app keeps animating"). Use `--no-settle` when you don't need to wait.
- X11-only apps need a session started with `new -x`. Prefer Wayland otherwise; with `DISPLAY` set, some apps quietly pick X.
- Apps have no D-Bus session bus. Warnings about it are expected.
- "incompatible build" errors: the server is from another guilab version. `guilab server --stop` (ends all sessions), then retry.
