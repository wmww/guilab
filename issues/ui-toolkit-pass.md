# UI code pass: what's left

The toolkit and theming parts are done (`guilab-kit`: theme tokens light/dark following the XDG portal, View > Appearance, menus with keyboard navigation, shared widgets; the viewer, editor and chat use it). Left:
- Go over the new editor and chat UI per design.md's "UI quality" rule (states, keyboard, focus, spacing, cursors, tooltips); build them on kit widgets and tokens only.
- Keyboard access to the menu bar itself (e.g. F10): keys go to the session app unless egui has focus, so there's no free key yet. Decide whether to reserve one.
- Tooltips are egui's (placed below the pointer). Restyle if they look off next to the new UI.
- Verify by using the viewer through guilab itself (screenshots + input), both themes.
- Flat look (design.md): remove every thin border and separator line (kit widgets, egui's default widget/panel strokes in the theme, viewer, editor, chat); keep outlines only for state. Check by screenshot in both themes.
