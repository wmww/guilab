# Viewer: session sizing

Spec from the user. Applies per session tab.

## View menu
- **Resize to window** (button): sets the session's *logical* size to the viewer's session area (in host logical pixels), then resets zoom/pan. The output's pixel size is that times the session scale, and the viewer scales it to fit. So the session scale doesn't change how big things look; it only changes sharpness (session scale above the host's: downscaled for display; below: upscaled and blurry).
- **Lock size to window** (checkbox): the same, but kept up to date as the viewer window resizes. While it's on, pan and zoom are disabled.

- **Session size and scale controls**: set the output size (WxH) and scale, including fractional scales (presets 1, 1.25, 1.5, 1.75, 2, plus custom). Agents already have this via `guilab output -s WxH --scale S`. With "lock size to window" on, changing the scale keeps the logical size and changes the pixel size.

## With an agent's lock
- Taking the lock freezes the output at its current size; viewer resizes stop changing it.
- The agent can still resize with `guilab output -s`, even with "lock size to window" on. Since pan/zoom are disabled, the viewer just scales the session to fit.
- When the agent unlocks (or the human revokes), the output fits the window again if "lock size to window" is on.
- The agent learns about every size change through the normal passive `output` event. It doesn't need to know why. Log the source (human/viewer vs agent).

## Resize handles
- Edge and corner handles just outside the session image, visible when there's room (e.g. zoomed out). Dragging one resizes the output, in session pixels, with a size readout while dragging.
- Resize live, throttled (~10/s) so apps aren't flooded. Not available while input is blocked by the agent's lock, or while "lock size to window" is on.

## Fractional scaling
wp_fractional_scale + viewporter are implemented. Checked GTK4 (gtk4-demo) at 1.25 and 1.5, including a live change: crisp, correct layout. When building this, also check Qt6 and a browser at fractional scales, and that the viewer's scale-to-fit doesn't add blur at matching scales.
