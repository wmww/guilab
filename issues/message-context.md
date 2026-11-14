# Human messages lack some of the context they refer to

Short messages ("also that black color") need to know what the human was looking at. Built: a message without images carries `screen`, an image of the session with the pointer drawn in, captured when it's sent, and `poll` points the agent at it (two fresh agents resolved "this gap is way too big" correctly from it plus the pointer position).

Still open, if agents keep misreading short messages:
- Capture when the human starts typing rather than at send (the app may change meanwhile, e.g. the agent's own restart).
- Viewer context: the selected tool, the current tab, an open drawing they were looking at but didn't attach.
