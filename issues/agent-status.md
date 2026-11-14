# Agent status: working / done

Queued; not started. Replaces `say`. Touches the CLI, the server (session state, event log), the viewer feed, the skill doc and design.md.

Every agent message also declares the agent's state, so the human always knows whether the agent is busy or waiting.

## CLI
- `guilab work "TEXT" [-m MSG] [--image PATH]...` sets the status ("running tests") and marks the agent as working. With `-m`, it also posts MSG as an agent message. Run it again to update the status.
- `guilab done ["MSG"] [--image PATH]...` posts an optional message, clears the status, and marks the agent as waiting for the human. It also releases the lock if held (and says so), since done means hands off.
- `say` is removed. Its error should point to `work -m` / `done`.

## Rules (errors say exactly what to run)
- `lock` and `input` need the working state: "not working; run `guilab work "what you're doing"` first".
- A blocking `poll` while working is an error ("still marked working; run `guilab done` first"). `poll --follow` is allowed (background monitors).
- When `poll` delivers a human message, the agent is marked working with the status "read your message". The human sees it was picked up, and the agent must still end with `done`.

## Viewer
- While working: an animated dots indicator at the bottom of the feed, the status text next to it in the agent's color, and the elapsed time. Show a dimmed "no guilab activity for Nm" hint if the agent goes quiet for a while (compiles are normal, so don't auto-clear).
- Status changes appear in the feed as small grey lines like commands. `-m` messages and `done` messages appear as agent messages.

## Scope
One status per session (one agent per session is the norm; the lock enforces it for input).
