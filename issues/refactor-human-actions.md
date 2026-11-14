# Human actions: one server API for the viewer and `guilab debug`

Priority: medium. Touches `server/src/viewer.rs` `apply_viewer_actions`, which the viewer rebuild may also touch: coordinate (or do it right after).

## What's wrong
The human acts through two independent server paths with different rules:
- Viewer: `viewer.rs` `apply_viewer_actions` (~496). Input is injected directly, dropped while `lock.is_locked()` except releases, bumps `last_active`, adds `TouchFrame` after touches. Resize is refused while locked. Revoke ignores `revoke_lock`'s "not locked" error. Message → `requests::human_message` with `MsgImage::Drawing`. Freeze captures and holds a base image.
- Tests (`guilab debug`, `Command::Human`): `requests.rs` ~412-450. Input is queued as a `Wait::Input` chain with `InputSource::Human` (so `unlock`/`revoke` abort the *human's* chains too, via `abort_inputs`), refused while locked, without the release exception. Revoke errors when not locked. Message → `human_message` with `Spec`/`Draw`. No resize, no freeze.

So integration tests exercise rules the viewer doesn't use, and a rule change has to be made twice. `human_message` itself also lives in `requests.rs` as a free function taking `&mut Images, &mut Slot`, and holds images one by one: if image 2 fails, image 1 stays held for the session's life.

## Proposed shape
- `server/src/human.rs` with methods on `Server`: `human_input(session, InputEvent)` (gate + TTL + log), `human_resize(session, size, scale)`, `human_revoke(session) -> Result`, `human_message(session, text, Vec<MsgImage>)`, `human_freeze(session) -> (image, id)`. `apply_viewer_actions` and the `Command::Human` arm both become thin adapters.
- `debug input` keeps chains (tests need sleeps/glides) but each event goes through `human_input`, and human chains aren't aborted by agent lock changes (`end_lock`'s abort should take only `key.is_some()` jobs).
- `human_message`: resolve all images first, then hold them all.
