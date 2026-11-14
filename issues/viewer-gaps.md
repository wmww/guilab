# Viewer gaps

Known limits of the step-6 viewer (see notes/viewer.md):
- No key repeat for held keys: sessions' seats have repeat rate 0 (so agent chains don't repeat), which also applies to keys the human holds in the viewer. Options: per-source repeat (synthesize repeats in the viewer as press events? not valid Wayland), or switch the seat's repeat info while the human types.
- A server on pixman (no render node) can't show the viewer. Could try a GLES renderer on a Mesa software EGL device (llvmpipe) for the viewer and upload session frames from CPU.
- No client-side decorations: on hosts without server-side decorations (GNOME) the window has no title bar.
- Host touch input isn't forwarded; app cursor surfaces (custom images) show as an arrow; the host clipboard isn't bridged to sessions.
- If the host rejects a dmabuf `create_immed` with a protocol error, the viewer closes instead of falling back to shm.
- Leaving the session image (without a button held) sends nothing, so app hover highlights stay until the pointer comes back.
