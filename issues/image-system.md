# Image system: open questions

The image system is built (phases 1–7; the model, text format, agent CLI, viewer glue and motion are in `notes/image-system-design.md`, the tested agent doc in the skill doc's "Images").

## Open questions for the user
Both are built as these defaults; change them if the user decides otherwise.
- **Deleting an element in the editor** (the spec says pictures can be "deleted as a unit", but there's no UI for it): Delete/Backspace removes the item under the pointer while the Move tool is selected.
- **Cropping in the editor** (the spec says crops are non-destructive and can be undone, but there's no UI for it): a Crop tool. Drag a rectangle (or its edges/corners, or move it); while the tool is selected, the whole canvas shows dimmed outside the crop, with a "Show all" button (in a bar over the canvas) to uncrop.
