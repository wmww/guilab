# Move tool behavior

Spec from the user. The dogfood session started a move tool; finish it to match this.

Drag with the move tool, by what's under the pointer (first match wins):
1. **Key point** → reshape the item.
2. **Item, or near it** (a tolerance of a few screen pixels, since thin lines are hard to hit) → move the whole item.
3. **Anything else** (outside the session, the live session with no screenshot open, or empty parts of a screenshot) → pan the view.

Key points:
- Box, ellipse, text, pen stroke: the four corners of the bounding box. Line and arrow: start and end.
- Dragging a corner resizes. Text scales its font size; pen strokes scale their points. Dragging a line/arrow endpoint moves that end.
- Drawn as small circles, only while the move tool is selected. A circle lights up when hovered. The hit area can be smaller than the circle.

Hover feedback: the item a drag would move lights up too (e.g. an outline or glow), so a drag never surprises. With overlapping items, the topmost (most recently drawn) wins, for both hover and drag.

Undo must cover moves and reshapes, not just new marks, so it becomes an edit history.
