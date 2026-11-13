# Screenshots of animating apps always wait 3 s

Apps that animate (spinners, `weston-simple-egl`, gtk4-widget-factory's spinner) never settle, so every settled screenshot waits the full `SETTLE_CAP` (3 s) and comes back "not settled". Agents can pass `--no-settle`, but they don't know in advance.

Ideas: treat a surface that has committed steadily at the frame rate for a while as "animating" and only wait for the rest; or settle on damage confined to a small area; or remember per session that it never settles and shorten the cap. Clients that ack a configure without committing also cost up to `CONFIGURE_WAIT` (1 s), e.g. weston-flower.
