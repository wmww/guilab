# Idle sessions still render at 60 Hz

`Server::tick` calls `Session::tick` for every session every 16 ms, which runs `render_output` even when nothing committed. The damage tracker makes it cheap (two idle sessions with GTK + kitty cost ~2% of a core), but it scales with sessions.

Fix: skip the render (not the frame callbacks or cleanup) when there was no commit, map/unmap, focus or output change since the last render. Keep a `dirty` flag on `Session`, set in the commit handler, `shell.rs` placement/focus and `set_output`. Mind the settle check, which wants "a render after the last commit".
