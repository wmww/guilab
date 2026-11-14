# Screenshot right after `run` misses the new app's window

`check_waiters` only waits for a first window (`Settle::NoWindow`). If a window is already up, `guilab run -- app2 && guilab screenshot` settles at once, before app2 has connected or mapped anything, so the shot lacks it. Seen with X apps (Xwayland startup of a client takes a few hundred ms) and applies to Wayland apps too.

Ideas: treat a `run` in the last N ms (say 2 s) with no new window since as busy, capped like the first-window wait; or count apps spawned since the last map.
