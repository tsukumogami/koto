critical_findings:
  - category: "B"
    description: >
      The design's own corrected text says the opposite of what Issues 4 and 5
      require. In Solution Architecture > Components > src/session/cloud.rs,
      the design states (as an explicit Phase 4 correction): "Same zero-cost
      discriminant, no new accessor needed; only the earlier method-call
      phrasing was inaccurate" -- i.e. callers should pattern-match
      `matches!(backend, Backend::Cloud(_))` directly against the `Backend`
      enum, and no new `Backend::is_cloud()` method should be added. Issue 4's
      first acceptance criterion nonetheless requires: "`Backend`
      (`src/session/mod.rs`) gains a `pub fn is_cloud(&self) -> bool` method
      ... since no such method exists on the enum today," and Issue 5's AC
      says wording "branches on `Backend::is_cloud()`." Both issues build
      exactly the accessor the design explicitly rejected as unnecessary,
      reintroducing the inaccuracy (a method call through the enum rather
      than direct variant matching) that the design's Phase 4/6 correction
      was written to walk back.
    affected_issue_ids: [4, 5]
    correction_hint: ""
  - category: "B"
    description: >
      The design uses two different function names for the same refactor
      target -- `path_resolution.rs`'s per-task resolver -- in two different
      sections, and the plan inherits the ambiguity instead of resolving it.
      Solution Architecture > Components says: "`src/engine/path_resolution.rs`
      ... the per-task resolver that currently computes staleness inline now
      calls `check_template_source_path` and passes the result ..." (the core,
      `Option<&Path>`-accepting function). But Solution Architecture > Key
      Interfaces labels the same function differently via code comment:
      "// Wrapper: used by path_resolution.rs and the three new call sites,
      which have a StateFileHeader in hand" directly above
      `check_template_source_dir(header: &StateFileHeader)` -- i.e. this
      section says path_resolution.rs uses the header-accepting wrapper, not
      the core path-accepting function the Components section names. Issue 1's
      Downstream Dependencies section papers over this by saying Issue 2 needs
      "both `check_template_source_path` ... and `check_template_source_dir`
      ... to exist and be importable from `src/engine/path_resolution.rs` and
      `src/cli/batch.rs`" without assigning either function to either site.
      Issue 2 hedges identically ("the header-accepting wrapper
      `check_template_source_dir` ... which `path_resolution.rs` can use
      if/when a header is in scope at its call site") and its acceptance
      criteria for the `path_resolution.rs` refactor never name which shared
      function must be called -- while Issue 2's own code-level analysis notes
      the resolver actually receives a pre-computed `base_exists: Option<bool>`
      from its caller, which matches neither function's signature
      (`Option<&Path>` nor `&StateFileHeader>`) as-is. The plan should have
      caught and resolved this design-level naming split rather than
      forwarding it as an unresolved "if/when" hedge.
    affected_issue_ids: [1, 2]
    correction_hint: ""
