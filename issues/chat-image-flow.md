# Screenshot editor ↔ chat integration

Queued feature spec from the user. Do it together with or after `image-system.md`, which defines the image model this relies on.

## Mental model
A screenshot is one physical object. While you edit it, it's **already attached** to your next message. You can delete it, but there's no "queue or discard" step. Past images (sent ones, the agent's) are never edited. Editing one implicitly makes a pending copy, and the copy is what you edit.

## Image model (get this right; no hacks)
- An image is: an unbaked base (pixels exactly as captured), a crop rect (optional), and a list of marks. Renderings (PNG with marks baked in, cropped, shrunk) are outputs only.
- **The editor only ever opens image objects, never rendered files.** Opening any image shows its base with its marks as editable items: text can be edited, items moved, and so on. Invariant: there must be no path inside guilab by which a baked-in annotation becomes part of an editable base. Only out-of-band tools (a system screenshot, an agent running GIMP) could do that.
- Crops carry through. If the agent pulled a cropped version, opening it in the editor shows the same crop of the unbaked base, with its marks.
- Images the agent attaches are always image ids (see `image-system.md`), so there is no path from a rendering back into the editor.

## Composer (bottom chat box)
- Pending images appear as thumbnails **inside the text box, above the text**, the same size and look as in the chat log. This replaces the current "drawing attached" text that shifts the box.
- Each pending thumbnail has an × at the top right. Deleting doesn't ask for confirmation; undo brings it back. Undo covers composer actions as well as mark edits.
- Send (Enter or the button) is possible when there's text **or** at least one pending image (the instructions may be on the image), and impossible when both are empty.
- Sent user messages look the same in the chat log as in the composer: images above the text, matching how agent images appear.

## Opening and closing the editor
- Drawing on the live app with any tool starts a new pending image. It shouldn't feel like a screenshot was taken (no flash, nothing on the main view changes), just like drawing on the app. A new pending slot animates into the composer.
- Switching back to the app-interaction tool closes the editor.
- Clicking any image (pending, your sent ones, the agent's) opens it in the editor. Opening a past image doesn't make it pending. The first edit does, by creating a pending copy (see below).
- Chat thumbnails: on hover (no delay, or a very short one), grow a bit for a better look. Click opens in the editor. This replaces the current delayed big preview.
- The screenshot view is never locked to the window, even when "lock size to window" is on for the app view. Pan and zoom always work while editing.

## Animations ("whoosh")
Use whooshes consistently, everywhere an image moves between the editor and a chat slot. Snappy and unobtrusive.
- Closing the editor: the image whooshes from the main view into its exact slot.
- Opening an image: it whooshes out of its slot into the main view.
- Switching from one pending image to another: the current one whooshes back to its slot and the new one whooshes out.
- While an image is open, its slot shows an **empty slot**, not a second copy (it's the same object). Light rays or a glow lead from the empty slot toward the editor, ideally tracking the image as you pan/zoom, to sell that the two are linked.
- Opening a past image (sent or the agent's): same whoosh out, leaving an empty slot in the chat log. On its first edit there's no whoosh back. The chat-log slot quietly fades back to the original, and the normal new-pending-slot animation plays in the composer. Nothing else signals that a copy was made; the edit just takes effect.
- The open image's pending slot (the empty one) gets a visual indicator such as an outline, so it's clear which one is in the editor.

## Notes
- Removes the Queue/Discard bar from the current editor.
- Animation timing and easing belong in the theme module (see design.md's theming rule).
