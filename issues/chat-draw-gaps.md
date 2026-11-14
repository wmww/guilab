# Chat and drawing gaps

Known limits of the step-7 chat and drawing (see design.md "Viewer"):
- The feed shows the human's sent drawings as stored for the agent, i.e. without their text notes (only the queue thumbnail and the open drawing show text). The viewer could keep the `Annotated::preview`s of what it sent and show those instead.
- Hiding the viewer drops unsent state: the draft, queued drawings and an open drawing (the feed comes back from the log).
- Marks can't be selected, moved or edited after drawing; only undo (last mark) and discard.
- No way to draw on an older screenshot or on a `say` image from the agent (e.g. "this one, not that one").
- A session resize between freezing and drawing isn't handled specially (marks stay in pixels of the frozen image, which is what's sent).
