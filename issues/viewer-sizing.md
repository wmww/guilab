# Viewer: session sizing

Spec from the user. Applies per session tab.

## View menu
- **Resize to window** (button): sets the session's output size so it fills the viewer's session area at 100% zoom (one session pixel per screen pixel), then resets zoom/pan.
- **Lock size to window** (checkbox): the same, but kept up to date as the viewer window resizes. While it's on, pan and zoom are disabled.

## With an agent's lock
- Taking the lock freezes the output at its current size; viewer resizes stop changing it.
- The agent can still resize with `guilab output -s`, even with "lock size to window" on. Since pan/zoom are disabled, the viewer just scales the session to fit.
- When the agent unlocks (or the human revokes), the output fits the window again if "lock size to window" is on.
- The agent learns about every size change through the normal passive `output` event. It doesn't need to know why. Log the source (human/viewer vs agent).

## Resize handles
- Edge and corner handles just outside the session image, visible when there's room (e.g. zoomed out). Dragging one resizes the output, in session pixels, with a size readout while dragging.
- Resize live, throttled (~10/s) so apps aren't flooded. Not available while input is blocked by the agent's lock, or while "lock size to window" is on.
