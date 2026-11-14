# Event log gaps

Known simplifications in what the log records (see design.md "Event log"):
- Human keys are logged as keysym name + the keysym's character. Dead keys, compose and IME input aren't turned into the text that actually got typed.
- Touch logs only finger down/up (with the last position), and gestures only their start; touch motion and gesture updates aren't logged.
- "window N closed" can't say who closed it (app, human, crash); agents have asked. The app exit event next to it usually tells.
- Token-less clients of one session share one delivery cursor, so two such agents split passive events between them (each wake event still reaches every poller).
