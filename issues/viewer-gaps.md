# Viewer gaps

- Client-side decorations (hosts without server-side ones, e.g. GNOME): untested on a real CSD-only host; tested against guilab with `GUILAB_VIEWER_CSD=1`. The resize border is 5 pt inside the window (no invisible margin outside it), and there's no window shadow.
- Motion polish left (`image-system-design.md` "Flights and glow"): a flight landing in a composer slot covers its × badge, which pops in at landing; flights clip square, thumbnails have rounded corners (tiny corner pop at landing); with the chat on the right the slot sits right next to the image, so the beam is a thin sliver (the halo carries the link). The open image's × removal and send just cut (no motion).
