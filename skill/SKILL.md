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
guilab screenshot                   # waits for apps to finish drawing; prints the PNG path (read it) and the image id (see Images)
guilab tree                         # windows/popups: ids, app_id, title, pixel geometry
guilab work "testing the login form"   # shown live to the human; needed before lock/input
guilab lock                         # input needs the lock; prints a token, e.g. gfm7b785
guilab -t gfm7b785 input move 200 150 click type 'hello' key Return
guilab done "Login works now, have a look" --image k3f9   # message the human (image optional), release the lock
guilab poll --timeout 300           # wait for their reply
guilab end                          # kill everything in the session
```

- Commands pick the session automatically: `-S NAME`, else `$GUILAB_SESSION`, else the one started from this worktree, else the only one (outside git, `new` names sessions s1, s2, ...). Outside git with several sessions, pass `-S` every time. The server starts itself.
- Screenshots go in the session dir, which is deleted by `end`. Use `screenshot -o PATH` for anything you want to keep. `--crop 'X,Y WxH'` shows only that part, `--max WxH` shrinks the PNG, `--cursor` draws the mouse cursor in.
- `new` prints the session's `WAYLAND_DISPLAY`. You only need it to run a client by hand; `guilab run` sets it up for you.
- Coordinates everywhere (screenshots, `tree`, `input`) are screenshot pixels, at any scale.
- Shells don't keep variables between your tool calls, so pass the literal token with `-t` on every command that needs it. While the session is locked, `run`, `kill`, `output`, `clip set` and `end` need it too. Read-only commands never do. If your token stops working, the human revoked it: run `poll` or ask before you `lock` again. Lost your token? `guilab done` releases the lock without it.
- After a rebuild: `guilab kill && guilab run -- CMD` (`kill` returns once the app is gone).
- The first window fills the output, like a maximized window, whatever size it asks for. Dialogs and later windows float on top at their own size. A window can be bigger than the output (apps have minimum sizes); `screenshot` then says it's cut off. Make the output bigger with `guilab output -s WxH`.
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

`guilab -t TOKEN output -s 800x600 --scale 1.5` resizes live (the token is needed only while locked). The human can resize the session in the viewer too, but not while you hold the lock; you see their changes as `output` events. `guilab clip get` / `clip set TEXT` reads or sets the clipboard. `guilab log` shows the session's full history. Every command has `--help` and `--json`.

## Working with a human

The human runs `guilab show` to open the viewer. There they can watch, use the app when you don't hold the lock, revoke your lock, and send messages with screenshots they drew on. The viewer always shows whether you're working (and on what) or waiting for them.

- `guilab work "STATUS"` marks you working, with STATUS shown live (e.g. "rebuilding after the toolbar fix"). Run it again to update it. Add `-m "MSG"` (and `--image ID`) to also post a message while you keep working. `lock` and `input` need it.
- `guilab done "MSG"` posts MSG (what you did, or what you need from them), releases your lock and shows the human you're waiting for them. Then `guilab poll`. It refuses while a human message is unread. Not needed before `end`.
- `guilab poll --timeout 300` waits for the next message, revoke, app exit or session expiry. It prints that plus everything else that happened. It exits 2 on timeout; then just poll again. Keep the timeout below your shell tool's own time limit. A blocking poll needs `done` first. `poll --follow` prints one line per event, for tools that watch output (allowed while working).
- When a human message arrives, every command says so on stderr (`[guilab: a human message waiting since ...; read it now: guilab poll]`), until you run `guilab poll`, which then returns it at once, even while working. Only `poll` shows messages. Reading one marks you working ("read your message"), so finish with `done`.
- `--image` takes an image id (a screenshot, or one you made with `image make`) or a PNG/JPEG/SVG file.
- Messages may come with drawings; see Images.
- Short messages refer to what the human is looking at: read the human input lines printed with them (where they clicked, where the pointer rests). A message without images comes with the image id of their screen as they sent it, their pointer drawn in; look at it.
- Every command prints, on stderr, what happened since your previous command (human clicks and typing, windows opening, apps exiting). Those lines are context, not errors.

## Images
Screenshots, the human's drawings and images you send are *images*: an id (like `k3f9`), a canvas size, an optional crop, and a numbered list of elements, drawn in order (later ones on top). Images never change. Every edit makes a new image with a new id, and its `parent` is the image it came from.

```
image m2x8 1280x720 scale 2, parent k3f9, crop 300,200 700x400
  1 raster r7qa 0,0 1280x720 (screenshot 14:05:12)
  2 red box 600,380 100x60
  3 red arrow 412,300 to 640,410
  4 green pen 300,500 120x60 (23 points)
  5 red text 640,410 "too big" (box 632,402 98x52)
```
- All coordinates are canvas pixels, even when the image is cropped or rendered smaller. Canvas pixels are screenshot pixels and `guilab input` coordinates. `X,Y WxH` is a rectangle: its top-left corner and size.
- `scale 2` is the UI scale of the screen the image came from. Line widths and the default text size are multiplied by it, so marks look the same size as on screen. Images you make from scratch have scale 1.
- `crop` is the part of the canvas the image shows. Elements outside it are kept, just not shown.
- `(...)` at the end of a line is extra information, such as a text's box or a pen's point count. It's ignored when you write it.

### Reading
`guilab screenshot` makes a new image each time and prints its header line under the PNG path. `guilab image ID` renders the image to PNG and prints the path, plus which part of the canvas the PNG shows. `--crop 'X,Y WxH'` renders that part of the canvas instead of the image's own crop. It may reach outside the crop, and `--crop none` renders the whole canvas. `--max WxH` shrinks the result. A crop at full size shows more detail than a shrunk whole image, for less context. `--list` prints the element list instead of rendering.

### Element lines (you read and write the same syntax)
- Marks. COLOR is red, yellow, green, blue, white, black or #rrggbb, default red.
  - `[COLOR] box X,Y WxH` and `[COLOR] ellipse X,Y WxH`. The ellipse fills that rectangle.
  - `[COLOR] line X,Y to X,Y` and `[COLOR] arrow X,Y to X,Y`. The arrow points at the second point.
  - `[COLOR] pen X,Y X,Y X,Y ...` is a freehand stroke through these points. Listings show only its bounding rectangle `X,Y WxH`; `--list --points` shows the points.
  - `[COLOR] text X,Y [size N] [width N] "TEXT"`. X,Y is the text's top-left. `size` is the font size in pixels; the default is 18 × scale, and a size you give is used as is. `width` wraps the text at that many pixels; by default it doesn't wrap. Inside the quotes, `\n` is a newline and `\"` a quote. Text is drawn on a dark backing, so it's readable on any background.
  - Options you leave out take their defaults. Listings also leave out options that have their default.
- Pictures:
  - `raster ID|FILE X,Y [WxH] [from X,Y WxH]` places the pixels of a raster or of a PNG/JPEG file at X,Y. Raster ids start with `r`, and every screenshot is a raster. By default one source pixel is one canvas pixel. WxH scales it. `from` takes only that part of the source, in the source's pixels, and the default size is then that part's size.
  - `svg ID|FILE|<svg ...</svg> X,Y [WxH]` is an SVG picture, drawn and moved as a unit. SVG ids start with `s`. Inline SVG runs from `<svg` to the matching `</svg>` and may contain spaces and quotes; `xmlns` is optional. Keep the `svg` keyword in front: `'svg <svg width="20" height="20"><circle cx="10" cy="10" r="9" fill="red"/></svg> 40,40'`. The size defaults to the SVG's own width/height or viewBox.
  - FILE paths must contain a `/` or an extension, e.g. `./mock.png`. Files are read once, when the image is made. A PNG that guilab rendered is refused; use its image id instead.

### Making images
```
guilab image make [--from ID] [--canvas WxH] [--color C] [--crop 'X,Y WxH' | --crop none] [--drop N,N...] [ELEMENT...]
```
It prints the new image's listing and the path of a preview PNG. Check the preview before sending.
- `--from ID` starts from a copy of image ID: its canvas, scale, crop and elements.
  - An element that starts with a number replaces that element of ID, in place. Copy its line and change what you need. A pen line with a new `X,Y WxH` moves and scales that stroke into the rectangle, each axis on its own.
  - Elements without a number are added on top.
  - `--drop 3,5` removes elements by number.
  - Numbers always refer to ID's listing, and the new image is numbered afresh. Don't drop and replace the same number.
  - `--crop` replaces the crop.
- Without `--from`, `--canvas` sets the size. By default the canvas is just big enough for the elements, counting from 0,0. `--color` sets the background; the default is transparent.
- Each ELEMENT is one shell argument, e.g. `'blue box 10,10 200x50'` or `'5 red text 640,410 "much too big"'`. A lone `-` reads more element lines from stdin, one per line, at that position. Numbered lines work there too.
- To send an image to the human: `guilab done "MESSAGE" --image ID`.

When the human sends a drawing, `poll` shows its listing and a command that renders it cropped to the marks. Run it: the listing says where the marks are, the pixels show what they point at.

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
- Screenshots wait up to 3 s for drawing to stop, and for the window of an app you just `run`. Apps that keep animating (spinners, video) are detected after about a second and shot once your input had 0.3 s to land ("the app keeps animating: this is one frame of it"). Use `--no-settle` when you don't need to wait.
- X11-only apps need a session started with `new -x`. Prefer Wayland otherwise; with `DISPLAY` set, some apps quietly pick X.
- Each session has its own private D-Bus session bus with no services on it (no portals, notifications or accessibility). Apps logging that such a service "was not provided" is expected.
- "incompatible build" errors: the server is from another guilab version. `guilab server --stop` (ends all sessions), then retry.
