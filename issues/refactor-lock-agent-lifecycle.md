# Lock and agent-state lifecycle: one path per transition (has bugs)

Priority: high (two real bugs, and every new lock/agent feature copies the pattern).

## What's wrong
Ending a lock is open-coded four times in `crates/server/src/`, each slightly different:

| path | where | logs | `release_all` | `abort_inputs` |
|---|---|---|---|---|
| `unlock` | `requests.rs` `Command::Unlock` (~202) | Unlock (if held) | yes | yes |
| `done` | `requests.rs` `Command::Done` (~377) | Unlock, Done | yes | yes |
| revoke | `lib.rs` `revoke_lock` (~450) | Revoke | yes | yes |
| `lock --force` | `requests.rs` `Command::Lock` (~186) | Lock{forced} | **no** | yes |

Bugs that fall out of this:
1. **`done --image BAD` drops the lock silently.** `Command::Done` calls `slot.lock.release(..)` before `agent_message(..)`. If the image fails (bad id, unreadable file), the command errors but the lock is already gone: no Unlock event, no `release_all`, queued chains keep running for nobody.
2. **`lock --force` leaves the old holder's keys/buttons/touches pressed** for the new holder (no `release_all`).
3. Aborted chains can leave a touchpad gesture open: `actions.rs` queues `PinchBegin`/`SwipeBegin` … `End` as separate steps; `abort_inputs` drops the rest and `Session::release_all` (`compositor/src/input.rs`) only releases buttons, touches and keys. Clients see a gesture that never ends.

Agent status is the same story: `Slot::working` is set in `Command::Work`, in `read_messages` (`requests.rs` ~793, a copy of Work's match), cleared in `Done`; the "needs working" rule lives in `handle_session` (~166) for Lock|Input and separately inside `Command::Poll` (~325).

## Proposed shape
- Move agent/lock state transitions onto `Slot` (new `server/src/agent.rs` or `slot.rs`), each doing logging + input cleanup in one place:
  - `Slot::end_lock(&mut self, why: LockEnd) -> Option<&'static str>` where `LockEnd = Unlock | Done | Revoke | TakenOver`; logs the right event, calls `session.release_all(Agent)` (extended to end an open gesture: track it in `Held`), and returns the error text for `abort_inputs`. The server wrapper then aborts the chains. All four paths above call it.
  - `Slot::set_working(status, key)` used by `Work` and `read_messages`; `Slot::done(..)`.
- `Done`: validate/attach images first, then release the lock. Same rule for any command that has side effects: do all fallible work before mutating.
- Keep `LockState` (`lock.rs`) as the pure token logic; make its fields private (`current` is poked from `viewer.rs` and `requests.rs` to read `shared`/`is_some`; give it `is_locked()`).
- Coordinate with `remove-share.md` (drops `shared`, `Server::share`, `HumanOp::Share`): do that first or together; it removes one more lock path.
- Tests (`tests/it/events.rs` `work_done_lock_and_revoke`, `input.rs` `lock_rules`): add `done --image /nonexistent` keeps the lock; `lock --force` while a `keydown` is held releases it (check via a GTK input app log, or `debug input` + key events in the log).
