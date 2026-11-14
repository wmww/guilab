# Backend cleanup pass: duplicated tables, small waste, dead bits

Priority: low. One sitting; each item is small. (Session-struct dead fields are in `refactor-compositor-session.md`.)

## Event kinds are listed in four places
`proto/src/events.rs`: the `EventData` enum, `kind()`, the `KINDS` array and `GROUPS`; plus the hand-written kind list and default tiers in the CLI's `POLL_HELP` (`guilab/src/main.rs` ~262). `default_tier` ends in `_ => Passive`, so a new kind silently becomes passive. Fix: a unit test that every variant's `kind()` is in `KINDS` and in exactly the groups intended; build `POLL_HELP`'s kind/tier lines from `KINDS`/`GROUPS`/`default_tier` at runtime (clap `after_help` takes a `String`); make `default_tier` exhaustive.

## `Tiers::tier` re-parses names for every event
`Tiers::tier` (`events.rs`) calls `expand_kinds` (allocating, string compares) on each call; `EventLog::visible` calls it per event, and `check_polls` runs it for each waiting poll every 16 ms tick. Resolve `Tiers` once when the request arrives (`Tiers::check` → `ResolvedTiers` with two small sets) and pass that to the log.

## Rendering work done before the cache check
`server/src/images.rs render_file` (~114) renders the whole image, then skips writing if `img/<id>...png` already exists. Compute the path first and return early when it exists (the name depends on `r.shows`, which `guilab_image::render::region_rect` can give without rendering; expose it). Agents re-fetch the same crop often.

## Local time offset written three times
`guilab/src/main.rs tz_offset`, `server/src/lib.rs tz_offset`, and `server/src/viewer.rs chrono_like_stamp` (its own `localtime_r`). Put one `local_offset()` in `proto` (next to `text::Fmt`, which takes it).

## Small dead or stale bits
- `guilab_image::Store::release` is used only by tests; message holds are never released (by design: held until the session ends). Remove it, or document that holds are session-scoped and make `drop_group` the only release.
- `guilab_compositor::write_rgba_png` (`frame.rs`) is only used by the viewer's save dialog (`server/src/viewer.rs save_dialog`); the image crate already encodes PNGs (`Rendered::png`). Move it next to its one user or reuse the image crate.
