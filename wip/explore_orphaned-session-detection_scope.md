# Explore Scope: orphaned-session-detection

## Visibility

Public

## Core Question

Koto session state already records the `template_source_dir` a session was
initialized from, but nothing reads it back to check whether that directory
still exists. When the originating working tree is torn down (ephemeral
sandbox reap, `git worktree remove`, container teardown), the session stays
"live" forever and later collides with a same-named `koto init` from an
unrelated environment, producing a generic "already exists" error
indistinguishable from a real concurrent session. What's the right shape of
fix -- and what artifact should capture it?

## Context

- Filed as tsukumogami/koto#189, labeled `enhancement`, not `needs-triage`.
- Real-world repro: a multi-week sweep workflow in a downstream consumer
  (`dangazineu/commuter`) hit this when its niwa instance was reaped between
  sessions; see dangazineu/commuter#49 for the consumer-side account.
- The issue itself proposes three candidate directions (not a decision):
  1. `koto status <name>` / the "already exists" init error check
     `template_source_dir` existence and say so explicitly.
  2. `koto session list` gains a staleness column or `--orphaned` flag for
     proactive discovery.
  3. A `koto session cleanup --orphaned` / `koto gc` sweep that finds and
     removes/reports orphaned sessions.
- Open questions the issue filer flagged but didn't resolve: is an
  existence check at read-time sufficient, or does the path itself need
  staleness handling for remount/rename (not just deletion)? Should this be
  opt-in/flagged, since some workflows may legitimately outlive their
  originating directory? How does this interact with any future
  non-local/remote session state direction?
- This is a background/dispatched exploration session (no live back-and-forth
  scoping conversation available); proceeding in effectively auto mode,
  following the research-first decision protocol at decision points and
  documenting choices rather than blocking on interactive confirmation.
- Label signals: no `bug` or `needs-prd` label present, so per the auto-mode
  default the adversarial demand-validation lead is not fired. Demand is
  independently well-evidenced by the linked consumer-repo incident, so this
  default holds.

## In Scope

- How koto's CLI and state-file layer currently expose `template_source_dir`
  and where existence-checking logic would plug in.
- Evaluating the three candidate directions (and any alternative shape) against
  the actual codebase structure -- which one fits with least surface-area
  change vs. which best serves the "proactive sweep" use case.
- Whether existence-checking needs to be broader than "does the path still
  resolve" (renames, remounts, symlink edges).
- Whether the behavior should be opt-in/flagged given legitimate long-lived
  sessions.
- Recommending the right artifact type (design doc, plan, or something smaller
  like a directly-scoped issue) for the chosen direction.

## Out of Scope

- Implementing the actual code fix -- that's later dispatched work once
  direction is settled.
- Re-litigating whether this is worth fixing at all (already triaged/filed).
- Remote/non-local session state as a design target -- only considered insofar
  as the chosen fix shouldn't foreclose it.

## Research Leads

1. **How is `template_source_dir` recorded and read today, and what CLI paths
   touch session state (`status`, `init`, `session list`)?**
   Need the concrete code shape before judging which candidate direction is
   cheapest/most natural to implement.

2. **Which of the three candidate directions (or a combination) best fits
   koto's existing CLI/session-state architecture, and what's the actual
   diff size and risk for each?**
   The issue's three options aren't mutually exclusive; need to know if one
   subsumes the others or if a combination is warranted, and how big each is.

3. **Is a path-existence check at read-time sufficient, or does staleness
   detection need to handle remounted/renamed directories (not just
   deletions)?**
   The brief calls this out explicitly as an open question; determines
   whether the fix is a simple `Path::exists()` check or needs something
   sturdier (e.g. recording a fingerprint alongside the path).

4. **Should orphan-flagging be opt-in/flagged, and are there legitimate
   workflows that intentionally outlive their originating directory?**
   Determines whether this is a default-on safety check or needs a flag to
   avoid false positives for long-lived/relocated sessions.

5. **Is there existing prior art in koto's own docs/design history for
   session lifecycle, staleness, or remote/non-local state that this fix
   should stay consistent with?**
   Avoids picking a shape that conflicts with a direction the project has
   already committed to elsewhere.

6. **What do comparable tools (docker, systemd, terraform, direnv) do to
   detect and report stale/orphaned local state tied to a since-removed
   directory?**
   Useful prior-art check for whether "read-time check" vs "proactive sweep"
   vs "both" is the common converged pattern, and how they phrase the
   resulting error/report.
