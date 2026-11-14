# Human messages lack the context they refer to

Short messages ("also that black color", "still no black??") were misread by an agent twice: it didn't know what the human was looking at. The poll output already includes the human input before the message (where they clicked, where the pointer rests), and the skill doc says to read it and take a screenshot.

Idea: attach lightweight context to each message from the viewer: the selected tool, the last hovered window/region, the current tab, maybe a thumbnail of what the session showed when they started typing. Do it together with or after `image-system.md` (a screenshot attached to every message would be an image id there).
