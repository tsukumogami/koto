<!-- decision:start id="context-artifact-naming" status="assumed" -->
### Decision: Context Artifact Naming Convention

**Context**

The `context_injection` state in the work-on koto template runs `extract-context.sh`
and then gates on the output artifact's existence. The current design uses
`wip/IMPLEMENTATION_CONTEXT.md` — a fixed all-caps path that matches the existing
`extract-context.sh` output. All other shirabe workflow artifacts follow
`wip/issue_<N>_<artifact>.md` (baseline, introspection, plan, summary). The context
file is the sole exception: all-caps, unnumbered, reads visually like a permanent
project configuration file rather than a per-workflow artifact.

Research confirmed the path appears hardcoded in 5 shirabe files: `extract-context.sh`
(write call + JSON summary output field), `SKILL.md`, `phase-0-context-injection.md`,
`phase-3-analysis.md`, and `phase-4-implementation.md`. All five require coordinated
updates in Phase 3 (shirabe integration) regardless of which convention is chosen,
since the script currently takes no issue-number argument.

**Assumptions**

- The gate degradation to evidence-only mode (when `{{ISSUE_NUMBER}}` substitution
  is unavailable) is acceptable per the design's existing handling of the introspection
  gate. If this assumption is wrong (i.e., the context_injection gate must auto-advance
  today), only option (a) satisfies the constraint.
- Phase 3 will update `extract-context.sh` to accept an issue number argument. This
  is required regardless — the script currently writes to a hardcoded path.

**Chosen: `wip/issue_<N>_context.md`**

The context artifact for issue-backed workflows is named `wip/issue_<N>_context.md`
where `<N>` is the issue number. The `context_injection` gate becomes
`test -f wip/issue_{{ISSUE_NUMBER}}_context.md`, which requires `--var ISSUE_NUMBER=<N>`.
Until `--var` ships, the gate fails unconditionally and `context_injection` operates as
an evidence-gated state (same behavior as introspection). Once `--var` ships, the gate
auto-advances when the artifact exists. `extract-context.sh` is updated in Phase 3 to
accept `--issue <N>` and write to the numbered path.

**Rationale**

The numbered convention is the established pattern across every other shirabe artifact.
Maintaining a permanently-inconsistent name creates ongoing friction for template
maintainers who must understand why this one artifact differs. The all-caps name in
particular signals "shared configuration" rather than "per-workflow artifact" to anyone
reading the wip/ directory. The gate degradation cost is temporary and accepted: the
design already uses evidence-only fallback for the introspection gate, and the
gate-with-evidence-fallback pattern is explicitly designed to accommodate `--var`'s
absence. The concurrency risk of fixed paths is permanent; option (b) eliminates it
permanently. Option (c) (`wip/context.md`) requires the same rename cost as option (b)
without gaining the numbered path's correctness benefits.

**Alternatives Considered**

- **`wip/IMPLEMENTATION_CONTEXT.md` (current)**: Fixed all-caps path. Gate works today
  without `--var`. Rejected because the naming is a permanent outlier in the artifact
  namespace — all-caps signals a project-level file, not a per-workflow artifact.
  Concurrency risk (two concurrent work-on sessions overwriting each other's context)
  is permanently unmitigated. The "gate works today" benefit disappears once `--var`
  ships, leaving only the naming inconsistency as a lasting cost.

- **`wip/context.md` (lowercase fixed path)**: Lowercase is better than all-caps but
  still unnumbered. Requires the same 5-file rename as option (b). Still not
  concurrency-safe. Does not follow any existing pattern in shirabe's artifact namespace.
  Rejected because it pays the rename cost without gaining the primary benefits of
  the numbered convention.

**Consequences**

What changes: the context artifact path in `context_injection` gate, directive text,
and Data Flow section of the design doc. `extract-context.sh` gains an `--issue <N>`
argument in Phase 3.

What becomes easier: template maintainers see a consistent `wip/issue_<N>_*` pattern
across all artifacts; concurrent work-on sessions in the same repo are safe.

What becomes harder: until `--var` ships, `context_injection` cannot auto-advance
(same trade-off already accepted for introspection). The Phase 3 `extract-context.sh`
update must land before the numbered path behavior is fully wired.
<!-- decision:end -->
