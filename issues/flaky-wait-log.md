# Flaky: app-log waits under load

`xwayland::x11_clipboard_bridging` and `input::gtk_input_and_clipboard` each failed once in a full `cargo test --workspace` (a `wait_log` timeout, `tests/it/input.rs`) while other worktrees were building (load ~7); both passed alone and on the rerun. Probably timeouts too tight under load. Next time, note which needle timed out.
