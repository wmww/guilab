# CLI rough edges

Found by testing the skill doc with a fresh agent.

- `lock` prints a sentence, so `TOKEN=$(guilab lock)` doesn't work. Consider printing just the token on stdout and the explanation on stderr.
- `ps` right after `kill` still shows the app as `running` (the process hasn't exited yet). `kill` could wait briefly for the exit.
- After a click that closes a popup, `input` reports the pointer over the surface below, not the popup that got the click. That's confusing when checking whether a menu item was hit.
- Screenshots don't warn when a window is larger than the output and gets cropped. Apps with minimum sizes (gtk4-widget-factory is 1368 wide) hit this at the default 1280x720.
