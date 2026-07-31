```yaml
critical_findings:
  - category: "C"
    description: >
      Issue 1 (template_source_status module) is happy-path-only: scanning
      the entire Acceptance Criteria section finds zero occurrences of any
      failure/edge-case vocabulary ("fail", "error", "invalid", "edge",
      "empty", "missing", "not found", "reject", "unauthorized", "timeout",
      "concurrent", "conflict", "duplicate"). All eight ACs describe
      construction, wiring, and three success-path unit-test cases
      (path exists, path does not exist, path absent/None) but none
      exercises a boundary or invalid-input condition beyond those three
      plain cases -- e.g. a `template_source_dir` that points at a file
      instead of a directory, a broken symlink, or a non-UTF8/malformed
      path -- so a wrong implementation that mishandles any such input
      (e.g. panics, or silently treats a file path the same as a directory)
      would still pass every listed AC.
    affected_issue_ids: [1]
    correction_hint: >
      Add at least one boundary-condition unit test to
      check_template_source_path/check_template_source_dir beyond the
      plain exists/does-not-exist/absent cases -- for example, a
      template_source_dir that resolves to a regular file rather than a
      directory, or a dangling symlink -- and assert the function's
      documented behavior (e.g. exists reflects Path::exists() semantics
      regardless of file type) rather than leaving that case unspecified.
  - category: "C"
    description: >
      Issue 4's AC 1 requires adding a brand-new `pub fn is_cloud(&self) ->
      bool` method on the `Backend` enum in src/session/mod.rs, and Issue
      5's AC 6 references that same method as the mechanism for gating
      collision-message wording. But the upstream design doc
      (DESIGN-orphaned-session-detection.md, Solution Architecture,
      src/session/cloud.rs component, the "Correction from Phase 4 plan
      generation" parenthetical) explicitly states the opposite as its
      final, corrected position: "Same zero-cost discriminant, no new
      accessor needed; only the earlier method-call phrasing was
      inaccurate" -- directing callers to inline-match
      `matches!(backend, Backend::Cloud(_))` instead of calling a method
      through the enum. An implementer following the design doc's explicit
      correction verbatim (no new accessor, inline match only) would not
      create `Backend::is_cloud()` and would therefore fail Issue 4's AC 1,
      which requires that exact method to exist.
    affected_issue_ids: [4, 5]
    correction_hint: >
      Reconcile the two artifacts before regeneration: either drop the
      "Backend gains a pub fn is_cloud(&self) -> bool method" requirement
      from Issue 4's AC 1 (and Issue 5's AC 6) and instead specify the
      inline `matches!(backend, Backend::Cloud(_))` pattern the design doc
      calls for at each of the three call sites, or -- if the new accessor
      is genuinely preferred for readability/reuse across handle_status,
      handle_list, and koto init's collision paths -- amend the design
      doc's corrected parenthetical to say an accessor is warranted after
      all, so the two documents agree on one mechanism.
```
