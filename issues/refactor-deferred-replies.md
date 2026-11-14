# Deferred replies: one pending-request type instead of seven ad-hoc lists

Priority: high (every new waiting command adds another list, another `check_*` loop and another cleanup spot that's easy to miss).

## What's wrong
Requests answered later each have their own struct, `Vec` on `Server` and tick loop (`crates/server/src/lib.rs` ~70-130, `requests.rs` ~464-728):
`ShotWait`/`waiters` + `check_waiters`, `InputJob`/`inputs` + `run_inputs`, `ClipWait`/`clips` + `check_clips`, `KillWait`/`kills` + `check_kills`, `NewWait`/`news` + `check_news`, `PollWait`/`polls` + `check_polls`, plus the viewer's `show` waiters inside `ViewerState` (`viewer.rs`). All carry `session`, usually `key`, and a cloned `UnixStream`. Consequences:
- Cross-cutting rules are applied inconsistently. `end_session` (`lib.rs` ~400) drains five lists by hand (`news` is left to `check_news` noticing the session is gone). Only polls drop when the client hung up (`peer_gone`, `requests.rs` ~731); a Ctrl-C'd `screenshot` still settles, captures and renders a PNG nobody reads, a Ctrl-C'd `input` keeps driving the app. `attach` (events in the reply) is called in some paths and not others (`check_news`, `end_session`'s replies).
- Each `check_*` is a hand-rolled `while i < len` / `remove(i)` loop.
- `handle_session` returns `Option<Response>`, `None` = "someone pushed a waiter"; the doc comment on `handle` still says "(screenshot settle wait)".
- `stream.try_clone()` + error boxing is repeated six times (`clone()` closure in `handle_session`).

Related: `send` (`lib.rs` ~533) switches the shared socket to blocking with a 2 s write timeout, on the only thread. A `poll --follow` reader that stops reading (piped into a paused process) stalls every session's frames and input for 2 s per event.

## Proposed shape
- New `server/src/pending.rs`:
  ```rust
  struct Pending { session: String, key: Option<String>, stream: UnixStream, kind: Wait }
  enum Wait { Shot{..}, Input{chain, resume_at}, Clip{pipe, buf, deadline}, Kill{..}, New{deadline}, Poll{..} }
  enum Step { Wait, Reply(Response), Stream(Response) /* poll --follow */ }
  ```
  One `Server::pending: Vec<Pending>` and one `tick_pending(now)` that, per entry: drops it if the peer hung up (with per-kind cleanup: an abandoned input chain is aborted and `release_all`ed like an unlock), runs the kind's step, and on `Reply` attaches events uniformly and sends. Input's "one chain per session, in order" stays a rule inside that loop.
- `end_session` becomes one `extract_if(|p| p.session == name)` with a per-kind final response (kill → ok with what was killed, others → "session ended").
- `handle_session` returns `Reply::Now(Response) | Reply::Later(Wait)`; the dispatcher does the stream clone once.
- Non-blocking writes: give each connection an output buffer flushed when writable (calloop `Generic` with WRITE interest while non-empty), or at least drop a follower whose buffer exceeds a cap instead of blocking.
- Split `requests.rs` (836 lines) along the way: dispatch + resolve, `pending.rs`, and the agent lifecycle (see `refactor-lock-agent-lifecycle.md`).
- Tests: Ctrl-C on a slow `input ... sleep 3000 ...` stops the chain (`tests/it/input.rs` already spawns a slow chain at ~91; extend it).
