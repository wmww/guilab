# Command policy in one table; validate on the server

Priority: medium (each new command has to be added in ~6 places; forgetting one is silent).

## What's wrong
What a `Command` is allowed to do and how it's treated is spread across two crates:
- `proto/src/lib.rs` `Command::access()` (lock rule) and `Command::log_text()` (logged or not, with a hand-kept "not logged" list).
- `server/src/requests.rs` `handle` (~100-160): which commands are global vs per-session (a match, then `unreachable!()` for the global ones at ~451 inside `handle_session`), which replies carry deliveries (`let attach = !matches!(Human | Log | Poll)`, ~156).
- `handle_session` (~165-180): which need the agent to be working (`Lock | Input`), which count as agent activity for the viewer's "quiet" timer (`!Human`), all of them bump TTL and `dirty`.
- Reply types exist only as doc comments ("Reply: [`ShotInfo`]"); `Response::data` is `serde_json::Value`, and the CLI's `format::print` re-matches on the command to guess the type.
- Input validation lives only in the CLI parser (`proto/src/input.rs` `parse`: `ms` ≤ 60000, finger ids ≤ 1000, …). The server deserializes `Vec<Action>` and trusts it: `move 0 0` with `ms = 4e9` from any client makes `actions.rs` `glide` allocate ~250M steps on the server thread. The CLI is the only client today, but the rule should live where it's enforced.

## Proposed shape
- One `Command::policy(&self) -> Policy` in proto:
  ```rust
  pub struct Policy { scope: Scope /* Global | Session */, access: Access, delivers: bool, needs_working: bool, agent_activity: bool }
  ```
  `handle` matches on `scope` (no `unreachable!`), `handle_session` reads `needs_working`/`agent_activity`, `attach` reads `delivers`. Exhaustive match, no `_ =>` arm, so a new command can't be forgotten.
- Split `Command` into `GlobalCmd` and `SessionCmd` if that reads better than `scope`; either way the compiler should force the decision.
- Server-side validation: `Action::check(&self) -> Result<(), String>` in `proto/src/input.rs`, used by `parse` and by `Session::check_chain` (`compositor/src/actions.rs`), so the limits have one home. Same for `Command::New` size/scale (checked in `requests.rs new_session` and again in `compositor/src/frame.rs set_output`; make one `OutputInfo::validate`).
- Optional, if it stays cheap: a `Reply` associated type per command via a small trait so `Client::call_as` and `format::print` stop re-deriving it. Don't build a framework.
- Fix the stale bits while here: `Command::Human` / `HumanOp` / CLI `Debug` docs say "until the viewer exists" (it does; they're the tests' stand-in for it); `Event::by` says "it may be a lock token" (it's a delivery key).
