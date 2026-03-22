# Critique: shirabe work-on koto template design
# Role: koto template author
# Focus: Can this template actually be written, compiled, and maintained?

---

## 1. Mutual exclusivity at setup: will the compiler accept mode-based routing?

The design requires `setup` to route to `staleness_check` (when `mode: issue-backed`) or
`analysis` (when `mode: free-form`). The agent re-submits `mode` in `setup`'s evidence epoch.

**Tracing compile.rs validation logic:**

The compiler calls `template.validate()` after building compiled states, which calls
`validate_evidence_routing` for every state. That function runs five rules on conditional
transitions:

1. `when` blocks must not be empty.
2. `when` values for enum fields must appear in the `values` list.
3. `when` fields must be declared in the state's `accepts` block.
4. Pairwise mutual exclusivity: for every pair of conditional transitions, at least one
   shared field must have disjoint values.
5. A `when` condition requires an `accepts` block on the state.

For `setup`, the template would declare:

```yaml
accepts:
  mode:
    type: enum
    values: [issue-backed, free-form]
    required: true
  branch_created: ...
  branch_name: ...
  baseline_outcome: ...
transitions:
  - target: staleness_check
    when:
      mode: issue-backed
  - target: analysis
    when:
      mode: free-form
```

The pairwise check (Rule 4) looks for a shared field with disjoint values. Both transitions
use `mode` as their shared field, and `issue-backed != free-form`, so the check finds
`has_shared_field = true` and `has_disjoint_value = true`. The condition
`!has_shared_field || !has_disjoint_value` is false, so no error is returned.

**Conclusion: this compiles correctly.** The mutual exclusivity check is not O(n^2) per
state in any harmful sense -- it's O(T^2 * F) where T is the number of conditional
transitions from a single state and F is the number of when-fields per transition. For
`setup` with two transitions and one when-field each, it runs one comparison. There is no
issue here.

One subtlety: the `when` value `"issue-backed"` contains a hyphen. The compiler uses
`serde_json::Value` comparison (JSON equality), so `"issue-backed"` the YAML string
deserializes to the JSON string `"issue-backed"`, and the enum value in the `accepts` block
is declared as the string `"issue-backed"`. These compare equal. The hyphen causes no
problem in the YAML or JSON layer.

---

## 2. Self-loop feasibility: does the engine accept or reject them?

The design uses three self-loops: `analysis` (`scope_changed`), `implementation`
(`partial_tests_failing`), and `pr_creation` (`creation_failed`).

**Compiler check:** `compile.rs` validates that transition targets exist in the compiled
states map (line ~185-195). A self-loop target `"analysis"` references the state being
compiled, which is already in `compiled_states` at that point (the loop builds all states
first, then validates). `validate_evidence_routing` does not reject self-loops -- it only
checks field names, values, and pairwise mutual exclusivity. **The compiler accepts
self-loops.**

**Engine check:** `advance_until_stop` in `advance.rs` maintains a `visited` HashSet
tracking states auto-advanced THROUGH during a single invocation. The comment at line 151
explains: "The starting state is NOT added to visited." After a transition to target T,
`visited.insert(target.clone())` is called (line 263). Cycle detection fires when the
resolved target is already in `visited` (line 246).

For a self-loop at an evidence-gated state (e.g., `analysis` with
`plan_outcome: scope_changed`): the agent submits evidence, the engine resolves the
transition target to `"analysis"` (same state), appends a `Transitioned` event, inserts
`"analysis"` into `visited`, and then loops. On the next iteration, `"analysis"` is the
current state and is terminal=false and has gates, so it re-evaluates. If another transition
is resolved that targets a state already in `visited`, cycle detection would fire -- but for
a simple self-loop at an evidence-gated state, the next `koto next` call re-enters with
fresh evidence (epoch cleared), so the transition won't re-resolve to itself without new
evidence. The self-loop does not trigger `CycleDetected` during a single invocation because
the agent must call `koto next --with-data` again with new evidence to re-enter.

**However, there is a subtle risk with auto-advancing self-loops.** If a state has an
unconditional self-loop (no `when` condition, target = self), `resolve_transition` returns
`Resolved(self)`, `visited` inserts self, loops again, and `visited` detects the cycle on
the second pass. The design's self-loops are all conditional (`when: {plan_outcome:
scope_changed}`), not unconditional, so this doesn't apply -- but the template author must
be careful never to make a self-loop unconditional.

**Conclusion: self-loops compile and work correctly as designed, with the constraint that
they must be conditional (gated on specific evidence values).**

---

## 3. Gate command realism

The design specifies these gate commands. Evaluating each against real project directory
behavior:

**`gh issue view {{ISSUE_NUMBER}} --json number --jq .number`** (`context_injection`)

This is correct. `gh issue view 71 --json number --jq .number` outputs `71` on stdout and
exits 0 if accessible, exits non-zero if not found or no auth. The gate checks exit code
only. Works as written once `--var` substitution is implemented. Without `--var`,
`{{ISSUE_NUMBER}}` is a literal string passed to `gh`, which returns a non-zero exit code
(not found), so the gate fails unconditionally -- acceptable since evidence fallback handles
it per the design.

**`test -f wip/issue_{{ISSUE_NUMBER}}_introspection.md`** (`introspection`)

Correct. `test -f` exits 0 if file exists, 1 if not. Works as written, no edge cases.
Requires `--var ISSUE_NUMBER=71` at init time for precise matching; without it, always
fails (evidence fallback applies).

**`test -f wip/issue_{{ISSUE_NUMBER}}_plan.md` and `test -f wip/task_*_plan.md`**
(`analysis`)

The `wip/task_*_plan.md` variant uses shell globbing in a `test -f` command. **This is
wrong.** `test -f` does not expand globs -- it passes the literal string `wip/task_*_plan.md`
to the kernel's `stat()` syscall, which looks for a file named literally `*`. This will
always return exit code 1, making the gate always fail. The design acknowledges this
limitation ("glob-based gates can match artifacts from prior workflows") but the deeper
problem is that `test -f` doesn't support globs at all. The correct form is
`ls wip/task_*_plan.md 2>/dev/null | grep -q .` or `[ $(ls wip/task_*_plan.md 2>/dev/null
| wc -l) -gt 0 ]`. This is a real bug that would cause the free-form analysis gate to
always fail, pushing every free-form run through the evidence fallback path even when the
plan file exists.

**`git rev-parse --abbrev-ref HEAD | grep -qv 'main\|master'`** (branch check in `setup`)

Not shown explicitly in the design, but implied by "gate: branch is not main/master". The
pattern `grep -qv 'main\|master'` is inverted-match grep with `\|` for alternation in
basic regex. In POSIX BRE (the default for grep), `\|` is not standard -- this works on
GNU grep (Linux) but not BSD grep (macOS). The portable form is
`git rev-parse --abbrev-ref HEAD | grep -qvE 'main|master'` (ERE) or two separate greps.
On macOS this gate would fail unexpectedly.

**`gh pr checks $(gh pr list ...) --json state --jq '...'`** (`ci_monitor`)

The full command is:
```
gh pr checks $(gh pr list --head $(git rev-parse --abbrev-ref HEAD) --json number --jq '.[0].number') --json state --jq '[.[] | select(.state != "SUCCESS")] | length == 0' | grep -q true
```

Three issues:
1. The inner `$(gh pr list ...)` returns an integer (e.g., `71`). `gh pr checks 71` takes
   the PR number as a positional argument -- this is correct syntax.
2. The `jq` filter `'[.[] | select(.state != "SUCCESS")] | length == 0'` outputs the string
   `"true"` or `"false"` (JSON boolean from `length == 0`). `grep -q true` matches the
   string. This works.
3. **Race condition**: If `gh pr list` returns no results (empty array), `.[0].number` in jq
   outputs `null`, and `gh pr checks null` is invalid. The design notes the "brief window
   after pr_creation" issue -- this is real, but the fix would need a null check in the jq
   filter: `.[0].number // empty`. The current command would produce `gh pr checks null`
   which exits non-zero, falling through to evidence fallback -- so it fails safely, but
   with a confusing error message rather than a clean "not ready yet" signal.

**`go test ./...`** (tests gate in `implementation` and `finalization`)

Correct for Go projects. Exits 0 on pass, non-zero on failure. The design notes this is
language-specific. The `TEST_COMMAND` variable suggestion in Consequences is the right
mitigation.

---

## 4. 15-state complexity vs hello-koto

**hello-koto** has 2 states, ~30 lines of template source. The design has 15 states.

**No template size limit exists in the engine.** The compiler reads the entire file into
memory, parses it, and validates it. There is no `MAX_STATES` constant or size check in
`compile.rs` or `types.rs`.

**Compiler performance:** The mutual exclusivity check is O(T^2) per state where T is the
number of conditional transitions from that state. For this template, most states have 2-3
conditional transitions. The worst case is `ci_monitor` with transitions for
`passing`, `failing_fixed` (both go to `done`? -- actually the design routes all to `done`
except `failing_unresolvable` to `done_blocked`), giving at most 3 transitions, or 3
pairings. With 15 states and average 2 conditional transitions each, the total comparisons
are 15 * (2 choose 2) = 15 comparisons. This is negligible.

**validate() iterates all states once** with O(T^2) inner loop per state. For 15 states
total, this is completely fine. The `extract_directives` function is O(lines) for the
full markdown body -- one pass through the file.

**Maintainability concern:** The real complexity problem is not performance but authoring.
15 states means 15 `## state-name` sections in the markdown body, all of which must exactly
match the YAML state names (case-sensitive string comparison in `extract_directives`). The
compiler will error with "state X has no directive section" if a heading is missing or
misspelled. With 15 states, the risk of a name mismatch during authoring is real.

The `hello-koto` template has only 2 states and was presumably trivial to write. A 15-state
template with conditional routing, gates, and accepts blocks on most states is qualitatively
different -- it's closer to writing a small program in YAML than filling out a form. There
are no `koto template lint` tooling aids, no syntax highlighting for `.md` templates, and
no partial-compile feedback. The author will need to compile repeatedly to catch validation
errors one at a time (the compiler returns on the first error, not all errors).

---

## 5. Epoch-scoped evidence and the mode re-submission design

**Why `mode` must be re-submitted:** koto's evidence is epoch-scoped. The `advance_until_stop`
function clears evidence after each auto-transition (`current_evidence = BTreeMap::new()`
at line 267). Evidence submitted at `entry` is only visible during `entry`'s epoch. When
the engine transitions to `context_injection` or `task_validation`, evidence is cleared.
`setup` is reached several states later with a fresh epoch, so `mode` from `entry` is
unavailable. This is not a design flaw -- it's a deliberate engine property that prevents
evidence leakage across state boundaries. The re-submission requirement is correct and
unavoidable given the current engine.

**Alternative topology: split `setup` into `setup_issue_backed` and `setup_free_form`**

Instead of:
```
entry (mode evidence) → context_injection OR task_validation → ... → setup* (mode re-submitted)
  → staleness_check OR analysis
```

The split approach:
```
entry (mode evidence) → context_injection → setup_issue_backed → staleness_check
entry (mode evidence) → task_validation → setup_free_form → analysis
```

Trade-offs:

**Split approach advantages:**
- No re-submission ceremony. `mode` is implicit in which state you're in. The template is
  more explicit -- the path you're on is visible from the state name alone.
- `setup_issue_backed` and `setup_free_form` can have different accepts schemas. The
  issue-backed setup might capture `baseline_outcome` while free-form setup captures
  different fields. No need to declare a superset schema in a single `setup` state.
- Compiler validation is simpler: each `setup_*` state has straightforward unconditional
  (or simple conditional) transitions, no multi-field routing needed.

**Split approach disadvantages:**
- Two `## setup` sections become `## setup_issue_backed` and `## setup_free_form`. The
  directives are nearly identical (both create a branch and baseline file), so you'd
  duplicate the directive text. The design explicitly rejects two-template duplication;
  state-level duplication has the same problem at smaller scale.
- The template has 16 states instead of 15. Minor but worsens the maintainability concern
  in Q4.
- The converge point at `analysis` is unchanged -- `analysis` still needs to handle both
  modes without `mode` evidence, which is fine since both setup states have unconditional
  transitions to `analysis` (or `staleness_check` for issue-backed).

**Verdict:** The split topology eliminates the re-submission requirement and makes the
template's routing structure more legible. The directive duplication is manageable -- both
setup states can share the same markdown section by using a minimal one-line instruction
in the non-authoritative section and the full text in the primary one, or the directives
can be kept brief ("see branch setup instructions above"). For a template that will be
read and maintained by other authors, the split is likely more maintainable than a
single `setup` state with a `mode` re-submission that requires understanding epoch-scoped
evidence semantics. The re-submission design is correct but demands non-obvious authoring
knowledge.

---

## Summary of Findings

**Finding 1: The `setup` mutual exclusivity check compiles correctly.** The pairwise check
finds `mode` as a shared field with disjoint values `issue-backed` vs `free-form`. No
compile error. The O(T^2) complexity is irrelevant at 2 transitions per state.

**Finding 2: Self-loops are safe only as conditional transitions.** The compiler accepts
them. The engine's cycle detection does not fire for evidence-gated self-loops within a
single `koto next` invocation. An unconditional self-loop would cycle and be caught by
`CycleDetected` -- but the design uses conditional self-loops only, so this is not a risk
in practice. The constraint should be documented explicitly in the template's header comment.

**Finding 3: Two gate commands are wrong.** `test -f wip/task_*_plan.md` does not expand
the glob -- `test -f` passes the literal `*` character to `stat()`, so this gate always
fails for free-form analysis. The branch check `grep -qv 'main\|master'` uses BRE `\|`
alternation which fails silently on macOS/BSD. Both bugs require fixes before the template
is functional in non-GNU environments.

**Finding 4: The 15-state template is authoring-heavy with no tooling support.** There is
no size limit, no performance issue, and the compiler is correct. The risk is human error:
state name mismatches between YAML and markdown headings, missing directive sections, and
the one-error-at-a-time compile cycle. The template author will benefit from writing the
YAML front-matter and markdown headings in lockstep, verifying each state compiles before
adding the next.

**Finding 5: The mode re-submission at `setup` is a correctness requirement with a cleaner
alternative.** Epoch-scoped evidence is non-negotiable in the current engine; the design
handles it correctly. The split topology (`setup_issue_backed` / `setup_free_form`) would
eliminate the re-submission pattern entirely and make the template's routing structure
self-documenting. The directive duplication in two setup states is a real cost but smaller
than the cognitive cost of explaining epoch-scoped evidence to every future template author
who reads this template as an example.
