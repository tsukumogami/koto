# Security Review: request-store-converge

Executed INLINE (subagents cannot spawn subagents).

## Dimension Analysis

### External Artifact Handling
**Applies:** No (no network/download), but local input-validation applies.
The feature downloads and executes nothing. It reads and writes only local
NDJSON session logs and the local terminal index under `<koto_root>`. The one
input-handling concern is internal: the result summary and `payload` originate
from agent-submitted terminal evidence (`EvidenceSubmitted.fields`). These are
already trusted-tenant inputs in koto's model (the agent drives the workflow),
but the result is later read back and inlined into the parent's directive, so
the envelope must be bound-checked and parsed defensively. Mitigation: bound the
`summary` length; treat `payload` as opaque `serde_json::Value` (never
eval/exec); parse the `request_store.result` event through the same
skip-and-continue discipline the index reader uses, so a malformed result event
degrades gracefully rather than aborting a converge.

### Permission Scope
**Applies:** No escalation. The feature uses the same filesystem permissions and
the same write sites koto already holds: append to a session event log, append
to the terminal index, append a parent event. No new files, no new directories,
no new process spawning, no network. The terminal-index append keeps its
`O_APPEND` + fsync discipline; the new `has_result` field does not change the
open mode or introduce a seek/`write_at`. The compaction lease (mode 0600) is
untouched. There is no privilege boundary crossed that did not already exist.

### Supply Chain or Dependency Trust
**Applies:** No. No new dependencies are introduced. The change is confined to
existing crates already in koto's tree (`serde`, `serde_json`, `anyhow`). The
new event variant and struct fields use the same derive macros already in use.
No build-script, no proc-macro, no external code path is added.

### Data Exposure
**Applies:** Yes — low severity, worth documenting. The result envelope persists
agent-produced content (summary + optional payload) in THREE places: the child
session log, the parent's `ChildCompleted` event, and (indirectly, as a
done-bit) the index. Two implications:
1. **Duplication of potentially sensitive content.** If an agent puts sensitive
   data in a result summary/payload, that data now lives in the parent's log as
   well as the child's. A reader with access to the parent log sees child
   results without opening the child — which is the feature's *point*, but it
   means parent-log access is now sufficient to read child outcomes. This is
   intended (convergence is a read), and koto's existing model already places
   parent and child logs under the same `<koto_root>` ownership, so no new trust
   boundary is crossed. Mitigation: document that result content inherits the
   same local-filesystem trust model as all session logs; agents must not place
   secrets in results any more than in evidence today.
2. **The index `has_result` flag leaks only a boolean** — that a session
   produced a result — never the content. This is the minimum disclosure
   consistent with the lean-scan-path requirement and is the safest possible
   index footprint.

### Concurrency / Integrity (koto domain-specific dimension)
**Applies:** Yes — the design's central integrity invariant. Multiple children
completing concurrently each append their own `request_store.result` to their
OWN child log (no shared file) and their result rides their OWN parent
`ChildCompleted` append. The only shared hot file is the terminal index, and the
design adds only a bounded `bool` to its line, explicitly preserving the
`MAX_INDEX_LINE_BYTES` (PIPE_BUF) atomic-append guarantee — the writer still
hard-errors on overlength lines. So N concurrent completions cannot corrupt the
index or one another's results (PRD R11 / AC11). The parent's converge read is a
point-in-time read of completed results; it never partially reads a single
child's result because each result is one atomic event append. Mitigation /
release-time enforcement: keep the index append on `O_APPEND` (no seek), keep
the result envelope a single event (not a multi-line write), and bound the
summary so the parent `ChildCompleted` line and any index interaction stay safe.

## Recommended Outcome

**OPTION 2 - Document considerations.** No design changes needed. The design is
confined to local append-only writes with no new dependencies, no privilege
change, and no network/external-artifact surface. The two dimensions that apply
(local input validation of the result envelope; intentional data duplication
across child and parent logs) are inherent to the feature and are addressed by
bounding the summary, treating payload as opaque, parsing result events
defensively, and documenting that results inherit koto's existing
local-filesystem trust model. The concurrency invariant is preserved by keeping
the index line bounded and each result a single atomic event.

## Summary
Limited attack surface: the feature adds local, append-only writes and a typed
envelope with no new dependencies, permissions, or network exposure. The
load-bearing concerns are defensive parsing/bounding of the agent-produced result
envelope and the intentional (in-trust-boundary) duplication of result content
into the parent log; both are documented, and the concurrency integrity of the
hot index path is preserved by keeping only a bounded boolean in the index line.
