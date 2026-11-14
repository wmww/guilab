# Dogfood: agent-side friction

Friction found while using guilab as an agent (outer guilab on the real desktop, inner guilab + gtk4-demo running inside an outer session). Each entry is self-contained.

## Nesting / developing guilab under guilab is undocumented
Skill doc and `--help` say nothing about running guilab inside a guilab session. What works: `guilab run -- /path/guilab server --show` in the outer session (the inner server inherits the session's `XDG_RUNTIME_DIR`, so its control socket is separate), and driving the inner one from the agent shell with `XDG_RUNTIME_DIR=<outer session dir> WAYLAND_DISPLAY=wayland-0 guilab ...`. Had to read `crates/proto/src/lib.rs` to learn the control socket lives under `$XDG_RUNTIME_DIR/guilab`. Suggest: a `--runtime-dir`/`GUILAB_DIR` override, or at least a short "testing guilab itself" note in notes/skill.

## Rebuilding the binary under a running outer server
`cargo build` replaces `target/debug/guilab`, which is both the outer CLI and the inner server. To keep the outer CLI matching the outer server I copied the binary elsewhere. Unknown whether a CLI/server protocol mismatch is detected; if not, it should be (version handshake with a clear "restart the server" error).

## First command dumps human pointer noise
First `run` printed "[guilab events since the session started]" with a long line of pointer moves and scrolls. Pointer-move spam is low value to an agent; clicks/typing matter. Consider collapsing moves/scrolls harder or omitting them unless they end in a click.

## `kill` returned before the app exited (fixed)
`guilab kill && guilab run -- CMD` (the skill's rebuild recipe) raced: `kill` replied right after SIGTERM, so a single-instance app (here: the inner guilab server) saw its old copy still alive and exited ("a guilab server is already running"). Then my next inner command autostarted a stray server outside the outer session. Fixed: the CLI's `kill` now waits (up to 3 s) until the killed apps are gone, and a server's shutdown no longer always sits out the full 2 s grace when its apps exited on SIGTERM (before, the outer SIGKILLed the inner exactly as its own grace ended).

## Inner CLI commands autostart a stray server
Any `guilab` command autostarts a server if none answers. When driving a nested guilab whose server is down or restarting, a stray `ls`/`new` silently creates a new server outside the outer session (no viewer, not an outer app). Had to wait for `control.sock` to appear by hand. Suggest an env/flag to disable autostart (e.g. `GUILAB_NO_AUTOSTART=1`), or a `guilab server --wait` that blocks until a server is up.

## `lock` printed a sentence, not a bare token (fixed)
Skill doc says `guilab lock` "prints a token", but both `lock` and `lock --force` printed `token gh9gm4uy (session guilab). Pass it to ...` on stdout, so the same capture yields garbage and the follow-up `-t $T input ...` fails with a confusing clap error ("unrecognized subcommand 'move'"). Fixed: stdout is now only the token; the hint goes to stderr.

## Lost token means `--force` on your own lock
I captured a token inside a command whose output I discarded; the next `lock` said the session is locked by someone. There's no way for the holder to recover its own token (e.g. `guilab lock` from the same worktree/agent could say "you already hold it: TOKEN", or `unlock --force` is the only exit). Minor, but cost a round trip.

## Typing right after a click in the viewer goes to the app, not the chat box
Driving the inner viewer: `input move X Y click type 'abc'` on the chat box gave the box focus, but the keys went to the session app (gtk4-demo) instead. With `sleep 200` after the click it works. The viewer apparently routes keys by last frame's focus. Humans rarely hit this, but scripted tests of the viewer do; ideally key routing follows egui focus as of the click.

## Messages split across two viewers are confusing to track
With nested guilabs, the human sometimes typed into the inner viewer's chat (goes to the inner `demo` session's log) vs the outer one (goes to me). Inner chat messages don't wake my outer `poll`. Not a bug per se, but the agent needs to also watch `ig poll` or check the inner log to see them.

## `log` output looked like it lacked a message that was there
I grepped `guilab log` of the inner session for "message" and found nothing, then told the human their inner-chat message was never sent. It had been (22:41, visible in their screenshot). Cause: `log` shows only the newest 100 events by default (`-n 0` for all), and pointer-motion events filled those 100. Nothing in the output says it was cut. Suggest: print "(N older events not shown; -n 0 for all)" when truncated, and/or count only non-motion events toward the limit.

## egui TextEdit: custom `.frame()` silently drops `.margin()`
(Code-side, but bit me in viewer work.) In egui 0.35 a TextEdit with `.frame(...)` ignores `.margin(...)`; padding must go in the frame's `inner_margin`. The chat box looked misaligned and clicks on its lower half missed it until fixed.

## Nested testing can't show fixes that need both levels updated
The outer guilab runs the old build for the whole session (it must stay up). Key repeat needed changes in both the session (repeat info sent to clients) and the viewer. The inner viewer is a client of the old outer session, which tells it "repeat off", so the fix can't be seen in the inner viewer's text boxes while dogfooding. Similar for anything where the outer's behavior as a compositor matters. A way to swap the outer server binary without losing sessions doesn't exist; worth knowing up front.

## The viewer can lose a click that arrives with the pointer's first move
`debug input move X Y click` on a viewer that had never seen the pointer didn't focus the chat box; `move X Y sleep 100 click` did. Humans rarely do that, but scripted viewer tests must add a pause.

## A human message was silently lost: delivered on `say`'s stderr, which I discarded
Severity: high. Every command prints "events since your previous command" on stderr, and that delivery *consumes* them: a later `poll` won't return them. I ran `guilab say "..." 2>&1 | tail -1` (to keep output short); a human message (with an image and two notes) that arrived just before was printed in the part I cut, and `poll` then never woke for it. The human had to ask 45 minutes later whether their message got lost. Agents routinely trim or drop output. Suggest: human messages (wake events) stay pending until a `poll` (or an explicit ack) returns them, even if another command's stderr already showed them; or at least `poll` should return immediately if a message arrived since the last *poll*. Also worth a line in the skill doc: "never discard guilab stderr".

## Short human messages are easy to misread without the viewer state they refer to
"also that black color" / "still no black color??" meant "the color picker has no black", but I read it as complaints about a black UI area twice. Text-only messages don't say what the human was looking at or pointing to. Idea: attach lightweight context to each message (selected tool, hovered widget/region, current tab), or encourage drawing in the hint. Not a bug, but costs round trips.

## Overlay tests: input went to a session the viewer wasn't showing
After restarting the inner guilab and creating sessions in order demo, colors, the viewer stayed on `demo` (current is fixed at the first frame, when only `demo` existed), while I assumed the alphabetically-first tab. Agent input to `colors` then showed nothing, and I spent several rounds thinking the overlay was broken. Nothing in the CLI says which session a viewer is showing. Suggest: `guilab show`/`ls` report the viewer's current tab, or a `--focus`/`show -S NAME` to pick it.

## `screenshot --no-settle` of a nested viewer can be a frame behind
Right after agent input to the inner session, an outer `screenshot --no-settle` often showed the inner viewer's previous frame (no key chips, chat not updated). Settled screenshots take 3 s with animations. Needed `sleep 0.3` by hand. A middle ground (`--wait-ms N`, or settle that ignores known overlay animation) would help.
