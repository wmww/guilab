# Editor: element actions should target the selection

The move tool has a selection (`Editor::selection`, see `notes/image-system-design.md` "Move tool"); Delete and moves use it. Not yet:
- Picking a color recolors only future marks (`pick_color` switches to the pen, which clears the selection). Desktop editors recolor the selection (one `Replace` per element).
- Keyboard nudges, Edit-menu commands (Delete, select all) and copy/paste.
- Rubber-band (drag on empty space) selection: empty-space drags pan today.
