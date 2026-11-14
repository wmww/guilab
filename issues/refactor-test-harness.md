# Integration test harness: deterministic waits, one set of helpers

Priority: medium (flakiness under load; notes already say "reproduce flaky tests by running two `it` binaries at once").

## What's wrong
`crates/guilab/tests/it/`:
- **Fixed sleeps as synchronization.** E.g. `events.rs` `poll_blocks_and_follows` sleeps 500 ms "so the pollers are registered" and 300 ms before `end`; `cleanup.rs` sleeps 300/500 ms then asserts process state; `sessions.rs ttl_expires` sleeps 2500 + 500 ms; `apps.rs` 700 ms and 1300 ms; `xwayland.rs` 300 ms twice; `input.rs` 100/200/500 ms; `viewer.rs` 300 ms. Under load these either flake (poller not registered yet → misses the message) or waste time.
- **Polling loops are re-written each time**: `Lab::wait_tree` (`main.rs`), `input.rs wait_log`, inline loops in `events.rs window_events` (~321, ~329), `xwayland.rs` (~161), `viewer.rs mapping` (~66), `input.rs kitty_types_unicode` (~260). Each has its own interval and timeout message.
- **Duplicate runners**: `main.rs` has `try_run`/`try_run_env`/`run`/`json`; `events.rs` adds `status`/`status_in`/`run_in` on `Lab` (exit code instead of bool, and a cwd). `try_run` and `try_run_env` duplicate the output decoding.
- Tests share helpers across modules by `pub fn` in arbitrary files (`input.rs` owns `gtk4()`, `has_python_module`, `wait_log`, `center`, `Lab::lock/input`; `cleanup.rs` owns `alive`).
- Skips print to stderr and the test passes, so a machine missing GTK4/Qt/Xwayland silently runs a fraction of the suite.

## Proposed shape
- `tests/it/util.rs` (or keep in `main.rs`): `fn wait_until<T>(what: &str, secs: u64, f: impl FnMut() -> Option<T>) -> T` (30 ms interval, panics with `what` and the last observed state). Rebuild `wait_tree`, `wait_log` and the inline loops on it. Replace every sleep-then-assert with `wait_until` on the observable (`ps` state, `pgrep`, `ls`).
- For "the poller is registered": expose pending-request counts in `ServerInfo` (`ping`), e.g. `pending: { polls, inputs, shots }`, and wait on that. Cheap, also useful for debugging a stuck server. (Pairs with `refactor-deferred-replies.md`, which makes the count trivial.)
- One runner: `Lab::exec(args) -> Out { code, stdout, stderr }` with builder-ish `.env()`/`.cwd()`; `run`/`json`/`try_run` become one-liners on it. Move shared app/skip helpers (`have`, `gtk4`, `qt6`, `has_python_module`, `wait_log`, `center`) into one `apps.rs`-style helper module, separate from the `apps` test module.
- Skips: count them (static counter) and print a summary, or honor `GUILAB_REQUIRE_APPS=1` to turn skips into failures for CI-like runs.
- Seen flaky: `xwayland::x11_clipboard_bridging` timed out in `wait_log` once in a full run (machine busy with a nested viewer); passed 3/3 alone.
