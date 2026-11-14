# Doc: element keys break when edits are replayed, and `set_base` corrupts history

**Land before phase 6.** Two real bugs in `crates/image/src/edit.rs`, both hit by the phase-6 flows in `image-system-design.md`.

## 1. `apply` doesn't advance the key counter (duplicate keys)
`Doc::new_key` (edit.rs:58) is the only thing that bumps `next`. `apply`/`revert` insert elements with the key stored in the `Edit` (`insert`, edit.rs:84) but leave `next` alone. So applying edits to a *different* `Doc` instance than the one that built them leaves `next` behind the keys already in the doc.

Phase 6 does exactly that. Copy-on-edit undo/redo: open past image P (n elements) → scratch `from_body` (next = n) → draw a box (`Add{key n}`) → `Op::Copy` + `Op::Edit`. Undo drops the draft. Redo rebuilds the draft from a fresh `from_body(P)` (next = n) and applies `Add{key n}`, and `next` stays n. The next mark gets key n again. From then on `index_of(n)` finds the first box: moves, reshapes, deletes and undo hit the wrong element.

Fix: in `insert`, `self.next = self.next.max(key.0 + 1)`, plus `debug_assert!(self.index_of(key).is_none())`. Add a test: build edits on one doc, replay them on a fresh `from_body`, add another element, and check the keys are unique.

## 2. `set_base` invalidates the positions stored in edits
`Edit::Add`/`Remove` store an index (`at`). `set_base` (edit.rs:125) inserts the capture under marks that are already there, which shifts every index by one. Edits recorded before it now point one slot too low. Redo of such an `Add{at: 0}` inserts the mark *under* the screenshot, where it's invisible. The test at image/src/tests.rs:829 checks only the key lookup, not undo/redo across `set_base`.

It isn't needed: the editor already keeps marks in `Editor::pending` until a doc arrives (`late_doc_gets_pending_mark`). So the glue can make the draft with `Doc::from_body(Body::screenshot(..))` when `captured` comes back.

Fix: delete `set_base` and its test, and update `image-system-design.md` (Edits block, "Draw on the live app" flow). If a base under existing marks is ever needed again, make it an `Edit`, so history stays consistent.

## 3. (same change) Give a Doc an identity
The editor relies on the host calling `Editor::reset()` whenever it switches docs (lib.rs:238). If the glue forgets once (undo opening another draft, switching images mid-drag), a gesture on key 3 of doc A commits a `Replace` on key 3 of doc B: keys are small integers starting at 0 in every doc, so the editor's "gone" check (canvas.rs:209) passes. Give `Doc` a process-unique `id()` from an atomic counter: a clone gets a fresh id, and moving the doc keeps it. The editor remembers the id its gesture/typing/pending state belongs to and resets itself when it changes. Copy-on-edit *moves* the scratch doc into the draft, so the id stays the same and typing carries on seamlessly. Keep `reset()` for an explicit drop.
