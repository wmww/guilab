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

## Builds and worktrees
- `wt` comes from the reasonable-rust-builds plugin (github wmww/agent-skills, enabled in `.claude/settings.json`). If it's not on PATH, stop and ask the user to install it; don't improvise worktrees.
- Work in a task worktree: `wt claim <branch>` prints it (`.worktrees/wtN`, warm `target/` kept between tasks); `wt release` only as your very last step, after your last commit. Never build in the main checkout; main only fast-forwards (`git rebase main` and the checks in the worktree, then `git merge --ff-only <branch>` in main).
- At most 3 worktrees exist at once (disk space). Coordinators: claim a worktree per sub-agent and give it the path; never use agent-tool worktree isolation (cold targets). `wt trim` when the parallel work winds down.
- Build and test only with `cargo test --workspace` (narrow with `--test it <module>::` and name filters) and `cargo clippy --workspace --all-targets`. No `-p`, no other `--features`, no per-command `RUSTFLAGS`/`CARGO_INCREMENTAL`/`CARGO_TARGET_DIR`: each combination builds and keeps another copy of the tree. `--release` only for timing checks.
- Expected costs are in `notes/code-map.md` (Build and test); if yours are 3x those at low load, stop and find out why.

