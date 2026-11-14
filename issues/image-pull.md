# Images are pulled, not pushed

Queued; not started. Do together with `annotation-marks.md` (the mark list is what lets an agent decide what to pull).

- Every screenshot (human drawing, human screenshot, agent `screenshot`) goes into one per-session image store: base pixels + marks, with an id. Kept until the session ends or a TTL/size cap expires it. It lives in the session dir, which is in `$XDG_RUNTIME_DIR` (tmpfs), so it's as cheap as memory and survives nothing it shouldn't. This is also the natural base for frame history later.
- Messages and deliveries give text only: image id, size, the mark list, and a one-line hint on how to fetch.
- `guilab image ID [-o PATH] [--crop X,Y,W,H] [--no-marks] [--max WxH]` renders a variant on demand and prints its path. Crop coordinates are screenshot pixels. Pulling a crop at full resolution gives more detail than a downscaled whole image, and uses less context. `--max` only shrinks (fits within WxH): a cheap overview of a big/HiDPI screenshot before pulling a crop.
- `guilab screenshot` returns an id too and takes the same `--crop`/`--max` flags, so agents use one model for all images.
- Risk: an agent skips looking when it should. Keep the fetch hint visible in `poll`, and say in the skill doc that a human drawing should normally be pulled (at least cropped to its marks).
