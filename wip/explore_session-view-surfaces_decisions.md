# Exploration Decisions: session-view-surfaces

## Round 1

- Surface split — the dashboard carries the full content view (a new detail
  tab in the TUI plus a single-session detail mode behind a new flag on the
  existing `--once` invocation); the workflow view carries at most a compact
  additive summary (key names with sizes, counts, anchor, store kind) and
  never key content: the workflow file's contract is a status tree with
  240-char previews and no content slot, and it is written by default under
  the hosting Claude session's directory, where session content does not
  belong. (status: confirmed — grounded in contract.rs/materialize.rs and the
  default-on `workflows.native` gate)
- "Remote" in the requirement is read as the session's store origin
  (`origin.store` kind and base, plus cloud sync presence), not a git remote:
  nothing in the header records a git remote, and inventing one at view time
  would show a fact koto never stored. (status: confirmed — types.rs has no
  such field)
- The `--once` 8-column feed contract stays untouched; session content never
  rides new columns. (status: confirmed — documented contract, scripts depend
  on the first six positions)
- The view's content reads must not silently mutate: avoid the CLI `get`
  path's `context_read` logging for bulk rendering, and treat remote-only
  (unpulled) keys as a presentation state rather than forcing a pull; the
  exact pull policy is the design's call. (status: assumed — evidence clear on
  the side effects, policy open)
- A migrated session is handled by calling `check_not_migrated` up front and
  presenting the target and workspace, instead of relying on per-read
  refusals that `ctx_exists`/`meta` hide as false/None. (status: confirmed)
- Bounded display reuses the existing cut-and-flag precedents
  (`safe_cut_len`, truncated flags, lossy decode, the 240-char preview); the
  excerpt bound, binary criterion and size-unit style are new decisions the
  design must set, since nothing in the repo decides them. (status: confirmed
  as approach; parameters open)
- One research round suffices: remaining open questions (excerpt bound,
  binary rule, pull policy, what Claude Code renders beyond known fields) are
  design decisions or empirically unknowable here, not research gaps.
  (status: confirmed)
