# Remove Share entirely

User decision: drop "share with agent" (human input while an agent holds the lock). It can only confuse the human and the agent. Remove it everywhere: the viewer toasts (Share / Stop sharing / Take over), server share state and `Server::share`, `guilab debug share`, the `share` event kind, docs (design.md Locking, skill/SKILL.md, viewer.md, code-map.md) and tests. While an agent holds the lock, the human's only option is Stop (revoke).
