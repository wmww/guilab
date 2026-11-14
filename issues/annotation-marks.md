# Annotation format: send marks as data, bake text into the image

Queued; not started. Touches `crates/markup` (compositing, message output) and the server's human message/poll formatting. Update design.md's drawing section when done.

- **Bake text into the image**, like every other mark. Reverses the earlier "text not in image" decision. That test's only miss came from one sample where the text box covered the label the arrow pointed at. In real use the agent knows the app (earlier screenshots, the code), and users don't cover what matters. Keeping text out of the image breaks what the human expects and is a special case. Consider a semi-transparent background behind text so things underneath stay faintly visible.
- **Also send every mark as data**, text included, in one list per image, in drawing order. Example:
  ```
  image 1 marks:
    red arrow 412,300 → 640,410 (tip)
    red box 600,380 – 700,440
    blue ellipse 90,120 – 260,180
    green pen stroke, bounds 300,500 – 420,560
    red text at 640,410: "too big"
  ```
  Coordinates are screenshot pixels. Pen strokes give only their bounds. This replaces the separate text-annotation lines and the "text isn't in the image" hint in `poll`.
- Check with a few fresh sub-agents (with the list vs. image only) that the list helps and doesn't distract.
