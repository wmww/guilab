Hi! Today you'll be working on guilab, a tool for letting humans and agents work together to repidly design, iterate on and polish graphical apps.

## Notes
The `notes/` directory contains your persistent notes about the project state. Create/edit/rename/split/delete notes as needed (without being asked). Keep them correct, and maximally useful to you. Remove sections or whole notes that are unimportant or obvious.
Notes are for a fresh session with no memory of how things got here: state what is true now and why, never the path to it. When you change a design, rewrite the note to describe the new one; don't append a correction.

## Issues
Issues live in `issues/` (unless specified, "issue" means one of these markdown files, *not* a GH issue). Do not solve them unless asked or the fix falls out of current work. Create/update issues for nontrivial problems discovered during other work. Delete confirmed-solved issues (move still-useful context into notes first).

## Workflow
- You own the code.
- When using sub-agents, always use `model: "opus"` unless otherwise asked.
- Refactor freely as needed. don't trust that existing code/comments/notes are necessarily correct, or existing design decisions are optimal.
- Unless otherwise asked, commit when you've completed your task. Don't pull/push.
- Include the "Co-Authored-By: <model>" in your commits, but not the link to a Claude session.
- Commit to the current branch unless asked, don't make feature branches.
- Keep prose, comments, errors, and commit messages compact unless extra detail is genuinely useful.
- When talking directly to humans, write especially simply and concisely. Avoid details or jargon unless critical for correctness.
- Avoid recording dates or time estimates unless asked, remember you can always recover dates from git
