# Category D: Sequencing / Priority Integrity

## Verdict
findings

## Critical findings

```yaml
critical_findings:
  - category: "D"
    description: "Issue 2's byte-identity AC ('responses are byte-identical to the
      pre-change binary's... The baseline is captured before this issue lands.')
      depends on a prerequisite artifact -- a frozen fixture of pre-change-binary
      responses -- that has no producer anywhere in the dependency graph. The only
      instruction to capture it lives in end-of-document prose (Implementation
      Sequence) that an issue-by-issue implementer is not guaranteed to read before
      starting issue 2's code changes, and inside issue 2 itself it is phrased as
      an already-satisfied fact ('is captured'), not as an actionable first step.
      If an implementer starts issue 2's response-construction changes before
      capturing the baseline, the pre-change binary's output is no longer
      reproducible without extra recovery (stash/checkout/rebuild) the plan never
      names, and the AC becomes unverifiable exactly as the plan's own prose warns
      ('the criterion cannot be evaluated afterwards')."
    affected_issue_ids: [1, 2]
    correction_hint: ""
```

- Dependency-ordering-error subtype -> `loop_target: 5`.

## Reasoning

**1. Chain edges 1->2->3->4->5.** 1->2 is real: issue 2's combinator and both
call sites consume the event variant and predicate issue 1 adds. 3->4 is real:
the pointer names the retrieval issue 3 creates. 4->5 is real in the sense that
issue 5 documents the shipped pointer/retrieval text, though nothing prevents
drafting docs earlier -- harmless over-serialization, not a defect worth flagging.

The 2->3 edge is the one edge that is explicitly *not* a real dependency: the
plan's own decomposition notes (`wip/plan_inline-phase-details_decomposition.md`,
Phase 5) state outright that "ISSUE:3 does not strictly need ISSUE:2, but lands
after it so its output shape matches what `next` produces once the rule is
settled." The PLAN document itself hides this nuance behind an unqualified
"Blocked by <<ISSUE:2>>." This is over-serialization (issue 3 could be built off
issue 1 alone), not under-serialization, so it creates no risk of unverifiable
work or missing coverage -- it just means the plan's "no parallelization worth
naming" claim is not literally true. Not elevated to a finding since it isn't
risk-creating, but worth naming since it was explicitly asked about.

**2. The baseline capture.** See the critical finding above. This is the one
sequencing defect with teeth: a procedural prerequisite (capture a frozen
fixture from the pre-change binary) sits outside the dependency graph entirely,
is asserted rather than instructed inside issue 2's checklist, and is otherwise
recoverable only from prose an issue-by-issue reader can skip.

**3. Issue 5's fmt/clippy/tests/template-compile/wip-hygiene bundle.** This is
not a structural deferral. `validate.yml` already runs `cargo fmt --check`,
`cargo clippy -D warnings`, the full unit-test suite, `koto-stability-tests`, and
the wip-hygiene check (`check-artifacts`) on every push to the PR, not only on
the final commit -- so issues 1-4's commits get this feedback continuously
regardless of what issue 5's AC list restates. Template compilation
(`validate-plugins.yml`) and eval coverage (`eval-plugins.yml`) are path-filtered
to `plugins/**`, and only issue 5 touches that path, so those two checks
correctly first become relevant exactly when issue 5 lands -- that is proper
gating by path, not deferral of something that could have run earlier. Issue 5
restating these gates as explicit ACs is redundant but not a sequencing problem.

**4. CI merge-gate coverage.** `validate.yml`'s `validate` job gates on
`check-artifacts`, `unit-tests`, `stability-tests`, `fmt`, `clippy`, `audit`,
`tsuku-distributed-install`, and `cloud-integration`. All but `audit`,
`tsuku-distributed-install`, and `cloud-integration` are addressed by an AC
somewhere in the plan. Those three are untouched by this feature (no dependency
changes, no recipe changes, cloud-integration tests are feature-gated and
unrelated to the CLI/engine surface this design touches), so they pass
trivially without needing an explicit AC -- their absence from the plan is a
completeness question at most, not a sequencing one, so it is out of Category D
scope.

**5. Issue 2's AC list vs. issue 3+.** Walked all sixteen bullets in issue 2.
None reference `koto status`, `directive`/`details`/`expects` retrieval, the
pointer, or documentation -- nothing in issue 2 is actually blocked on issue 3
or later. The one AC that reaches outside issue 2's own scope is the baseline
bullet (finding above), and that reaches backward/outward, not forward into a
later issue.

**6. "No parallelization worth naming."** Technically inaccurate per point 1
(issue 3 vs. issue 2), but the inaccuracy runs in the safe direction -- the plan
over-serializes rather than under-serializes -- so it does not meet the finding
criteria for dangerous parallel execution or QA deprioritization.
