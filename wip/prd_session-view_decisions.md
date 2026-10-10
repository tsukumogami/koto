# /prd Decisions: session-view

| id | artifact | tier | status | question |
|----|----------|------|--------|----------|
| prd-d1 | wip/prd_session-view_scope.md | 2 | confirmed | Phase 2 discovery reuses the exploration round's four research files instead of re-spawning agents: the same questions were investigated against this worktree hours ago, and a duplicate fan-out would add cost without new evidence. |
| prd-d2 | docs/prds/PRD-session-view.md | 2 | confirmed | "Remote" on screen means the session's store origin (origin.store kind and base); no git remote is recorded anywhere in a session, so showing one would invent a fact. Displayed label: "store". |
| prd-d3 | docs/prds/PRD-session-view.md | 2 | confirmed | Binary means content that is not valid UTF-8 (sampled prefix) or contains NUL; binary values render as type, size and hash, never bytes. Text values render as a bounded excerpt with an explicit truncation marker; the exact bound and size-unit style are named constants the DESIGN sets. |
| prd-d4 | docs/prds/PRD-session-view.md | 2 | assumed | The cloud read policy (pull remote-only content, with its local-write side effects, vs metadata-only display) is the DESIGN's decision; the PRD requires only that remote-only keys are shown with size and an explicit "not local" state when content is not fetched. |
