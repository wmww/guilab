# Editor: hover is the only "selection"; decide before adding element actions

Low priority, a design decision for the user. Raise it before adding any action on an existing element (recolor, nudge, copy/paste, z-order, delete from a menu).

The move tool has no selection. `Delete`/`Backspace` removes whatever is under the pointer (`delete_hovered`, editor/lib.rs:282; canvas.rs:257), and `Hit`/`hovered()` is the public notion of "the current element". That matches `move-tool.md`, but every element action that will come up needs a target that doesn't move with the pointer:
- Picking a color recolors only future marks (`pick_color`, lib.rs:297), even while you edit a text (`Typing.color` is fixed at start, text.rs:23). In desktop editors, choosing a color with an item selected recolors it.
- Keyboard nudges, menu commands (Edit → Delete) and copy/paste have no target while the pointer is over the menu.
- Delete depends on the pointer resting exactly on a thin line.

Option: `selection: Option<ElemKey>` (set by a Move press on an item, kept after release, cleared by a press on empty space, Esc, or the element going away). It's drawn with the same glow plus key points. Delete, color picks (one `Replace`), and future actions act on it, falling back to hovered. Hover feedback stays as specified. It would replace `delete_hovered` with `delete_selected` before phase 6 wires keys and menus to it, so decide (or explicitly defer) before then.
