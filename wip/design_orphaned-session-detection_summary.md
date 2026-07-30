# Design Summary: orphaned-session-detection

## Input Context (Phase 0)
**Source:** /explore handoff
**Problem:** koto's `koto init`/`koto status`/`koto session list` never check
whether a session's recorded `template_source_dir` still exists on disk, so
a torn-down originating working tree leaves the session "live" forever and
a same-named re-init collides with a generic, undiagnosable "already
exists" error (tsukumogami/koto#189). koto already has a working version of
this check (`SchedulerWarning::StaleTemplateSourceDir`, Decision 14 in
DESIGN-batch-child-spawning.md), scoped only to the batch scheduler.
**Constraints:**
- Direction 3 (destructive sweep/gc) explicitly out of scope for this design.
- Must reuse `StaleTemplateSourceDir`'s shape/`machine_id` vocabulary rather
  than invent a parallel signal.
- Must not name anything `--orphaned` (collides with existing
  `koto workflows --orphaned`).
- Must account for the shipped `CloudBackend`/cloud-sync cross-machine
  resume workflow as the primary legitimate false-positive source.

## Security Review (Phase 5)
**Outcome:** Option 2 (document considerations)
**Summary:** No security-relevant risk found that requires changing the
design. The new I/O (`koto init` opening a colliding session's header) and
new output fields (a local path, `machine_id`) stay within koto's existing
single-user, local-filesystem trust boundary and existing disclosure
precedent (`StaleTemplateSourceDir`).

## Current Status
**Phase:** 5 - Security (complete)
**Last Updated:** 2026-07-29
