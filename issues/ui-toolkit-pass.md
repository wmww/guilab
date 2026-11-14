# UI code pass

Queued for after the dogfood session and the queued viewer features (move tool, cursors, sizing, agent status).

Go through the viewer and markup UI code and get it into shape per design.md's "UI quality" rule:
- Find hand-rolled widgets (menu bar from plain buttons, toolbars, popups, panels) and fix what feels off compared to a normal desktop app: hover/pressed/disabled states, keyboard navigation and shortcuts, menus closing on outside click/Escape, focus handling, spacing, cursors, tooltips.
- Pull repeated patterns into a small set of reusable widgets with the right abstractions. Keep `crates/markup` free of guilab types.
- Check if a well-built egui crate covers something better than our code; adopt only if it's genuinely good.
- Verify by using the viewer through guilab itself (screenshots + input), not just by reading code.
