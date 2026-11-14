# Human messages lack some of the context they refer to

Short messages ("also that black color") need to know what the human was looking at. Built: a message without images carries `screen` (the session with the pointer drawn in, captured at send), and any message names a past image open in their editor (`viewing`); `poll` points the agent at both.

Still open, if agents keep misreading short messages:
- Capture when the human starts typing rather than at send (the app may change meanwhile, e.g. the agent's own restart).
- Other viewer context: the selected tool, the current tab.
