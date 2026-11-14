# Dogfood: agent-side friction

Friction found while using guilab as an agent (outer guilab on the real desktop, inner guilab + gtk4-demo running inside an outer session). Each entry is self-contained.

## Nesting / developing guilab under guilab is undocumented
Skill doc and `--help` say nothing about running guilab inside a guilab session. What works: `guilab run -- /path/guilab server --show` in the outer session (the inner server inherits the session's `XDG_RUNTIME_DIR`, so its control socket is separate), and driving the inner one from the agent shell with `XDG_RUNTIME_DIR=<outer session dir> WAYLAND_DISPLAY=wayland-0 guilab ...`. Had to read `crates/proto/src/lib.rs` to learn the control socket lives under `$XDG_RUNTIME_DIR/guilab`. Suggest: a `--runtime-dir`/`GUILAB_DIR` override, or at least a short "testing guilab itself" note in notes/skill.

## Rebuilding the binary under a running outer server
`cargo build` replaces `target/debug/guilab`, which is both the outer CLI and the inner server. To keep the outer CLI matching the outer server I copied the binary elsewhere. Unknown whether a CLI/server protocol mismatch is detected; if not, it should be (version handshake with a clear "restart the server" error).

## First command dumps human pointer noise
First `run` printed "[guilab events since the session started]" with a long line of pointer moves and scrolls. Pointer-move spam is low value to an agent; clicks/typing matter. Consider collapsing moves/scrolls harder or omitting them unless they end in a click.
