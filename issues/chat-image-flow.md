# Screenshot editor ↔ chat integration: animations

Spec from the user. Everything but the animations is built (image-system phase 6; how it works: `notes/image-system-design.md` "Viewer glue", `notes/design.md` "Drawing flow"). The animations are phase 7, designed in the same note's "Flights and glow"; the viewer already keeps the hooks (slot rects, the open image's rect, `Canvas.visible`, empty and outlined slots, `cross_fade` on copy-on-edit).

## Animations ("whoosh")
Use whooshes consistently, everywhere an image moves between the editor and a chat slot. Snappy and unobtrusive.
- Closing the editor: the image whooshes from the main view into its exact slot.
- Opening an image: it whooshes out of its slot into the main view.
- Switching from one pending image to another: the current one whooshes back to its slot and the new one whooshes out.
- While an image is open, its slot shows an **empty slot**, not a second copy (it's the same object). Light rays or a glow lead from the empty slot toward the editor, ideally tracking the image as you pan/zoom, to sell that the two are linked.
- Opening a past image (sent or the agent's): same whoosh out, leaving an empty slot in the chat log. On its first edit there's no whoosh back. The chat-log slot quietly fades back to the original, and the normal new-pending-slot animation plays in the composer. Nothing else signals that a copy was made; the edit just takes effect.
- The open image's pending slot (the empty one) gets a visual indicator such as an outline, so it's clear which one is in the editor.

## Notes
- Animation timing and easing belong in the theme module (see design.md's theming rule).
