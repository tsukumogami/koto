<!-- decision:start id="forward-compat-diagnosability" status="assumed" -->
### Decision: Forward-Compat Diagnosability for Batch Hook Templates

**Context**

The batch-child-spawning feature adds a new optional field on `TemplateState`
(the exact name is resolved in Decision 1 — `batch` or `materialize_children`).
Today, `CompiledTemplate` and its nested structs do not set
`#[serde(deny_unknown_fields)]`, and the `SourceFrontmatter` parsed from
markdown templates has no `format_version` field at all. When an existing
v0.7.0 binary encounters a template that declares the new field, serde silently
drops it. The batch never materializes, no error is raised, and the user sees
a workflow that appears to "do nothing" with no diagnostic signal that their
binary is too old.

The prior two template-format expansions handled forward-compat by exploiting
an existing choke point rather than a version bump. v0.6.0 added structured
gate output and the gate-contract validation path, but routed migration through
a `strict`/permissive compile mode and a transitory `--allow-legacy-gates`
flag; `format_version` stayed at 1. v0.7.0 added the `children-complete` gate
type and hierarchical workflows; again `format_version` stayed at 1. v0.7.0
got forward-compat for free because new gate types land in `compile_gate`'s
match on `gate_type`, which has an explicit `other =>` arm that errors with
`unsupported gate type {:?}`. An old binary seeing `type: children-complete`
fails cleanly at compile time with a localized, named error.

The batch hook is different: it is (per Decision 1's framing) a new field on
`TemplateState`, not a new gate type. It does not flow through any existing
`other =>` rejection path. The default serde behavior — silently drop
unknown fields — is the failure mode we need to disrupt.

**Assumptions**

- Decision 1 lands the batch declaration as a new field on `TemplateState`
  (not as a new gate type). If Decision 1 reframes the hook as a new gate
  type, this decision collapses: the existing `compile_gate` `other =>` arm
  already produces a clean error on old binaries and no additional work is
  needed.
- An audit of existing `.md` template fixtures and downstream consumers
  (`test/functional/fixtures/templates/`, `plugins/koto-skills/**/templates/`,
  shirabe templates, tsuku templates) will find no templates that currently
  rely on serde's silent tolerance of unknown fields as free-form
  annotations. This is the audit that must happen before the PR that
  introduces the batch field.
- `format_version` is not actively checked against `SourceFrontmatter` today;
  the field exists only on compiled JSON. Any option that wants to reject
  a template at `.md` parse time based on version requires also adding the
  field to `SourceFrontmatter` and validating it there.
- Decision made in `--auto` mode. Status is `assumed` until the audit in
  Step 1 of the rollout runs and confirms no pre-existing unknown-field
  templates exist.

**Chosen: Narrow `deny_unknown_fields` on `TemplateState` (Option 2, scoped)**

Add `#[serde(deny_unknown_fields)]` to the `TemplateState` struct in
`src/template/types.rs` (line ~47) and to the corresponding `SourceState`
struct in `src/template/compile.rs` (line ~41). Do not add it to
`CompiledTemplate`, `Gate`, `VariableDecl`, `Transition`, or other structs
— the scope is precisely the struct that is gaining the new field.

The attribute lands in the SAME PR that introduces the batch field. Old
binaries (v0.7.x) parsing a new-style batch template will reject it at
compile time with a precise, serde-generated error: `unknown field
`materialize_children`, expected one of `transitions`, `terminal`, `gates`,
`accepts`, `integration`, `default_action``. The error names the offending
field and lists the fields the old binary actually supports, which is a
strong implicit signal that the binary is out of date. The error is
localized to the specific state block, not buried under a generic validation
failure.

Concretely, the PR that ships the batch feature must:

1. **Audit first (pre-merge, separate prep commit).** Grep all `.md`
   template fixtures in `test/functional/fixtures/templates/`,
   `plugins/koto-skills/skills/*/templates/`, `docs/examples/`, and any
   template shipped in `shirabe/` and `tsuku/` under this workspace. Check
   for any YAML state entries that contain fields outside the current
   `SourceState` whitelist (`transitions`, `terminal`, `gates`, `accepts`,
   `integration`, `default_action`). If any exist, either remove them or
   document why the strictness should not apply and fall back to Option 6
   for those cases.
2. **Add the attribute.** `#[serde(deny_unknown_fields)]` on `SourceState`
   and `TemplateState`. Same commit adds the new `batch` /
   `materialize_children` field so that the new templates parse cleanly
   on the new binary.
3. **Add a regression test.** A fixture template that contains
   `made_up_field: 1` under a state block must fail compile with the
   serde error. This pins the behavior and prevents accidental regression
   if someone later strips `deny_unknown_fields` under unrelated refactoring.
4. **Document the guarantee in `template-format.md`.** One paragraph in the
   koto-author skill reference explaining that state entries reject
   unknown fields and that this is intentional — both so template authors
   catch typos and so old binaries produce clean errors when run against
   templates that use fields from a newer koto version.

**Rationale**

Five drivers pushed toward this option:

1. **Old-binary UX is precise, not coarse.** The serde error names the
   unknown field (`batch` or `materialize_children`) and the state it sits
   in. A user who sees `unknown field `materialize_children`, expected
   one of ...` and whose template clearly contains that field will
   correctly diagnose "my binary is too old" without needing to consult
   docs. Option 1 (format_version bump) produces a coarser error
   (`unsupported format version: 2`) that requires the user to know that
   format versions correlate with koto releases.

2. **Matches koto's existing precedent.** Both v0.6.0 and v0.7.0 added
   new template surface without a format_version bump. v0.7.0 in
   particular gets forward-compat for free via `compile_gate`'s `other =>`
   match arm. Option 2 extends the same principle (fail fast, name the
   unknown surface) to a struct field rather than a gate type. Option 1
   would be the first version bump in koto's history and would set a
   precedent that every future template-format expansion must bump
   format_version, which conflicts with the way v0.6.0 and v0.7.0 actually
   shipped.

3. **No cascading serial bumps.** If format_version becomes the mechanism,
   every future feature (child-template path resolution, retry semantics,
   observability surface, any field added to any struct later) has to
   either bump format_version or tolerate the same silent no-op problem.
   That forces a governance question the project has not yet engaged
   with: when do we bump? On every field addition? Only on breaking
   changes? Option 2 sidesteps this entirely by making ANY unknown field
   a hard error automatically.

4. **Option 1 requires non-trivial plumbing to actually work for .md
   templates.** Today `format_version` is only validated in
   `CompiledTemplate::validate` (types.rs:310), which is called on
   pre-compiled JSON. Markdown templates do not carry format_version in
   frontmatter — `SourceFrontmatter` has no such field. A working
   Option 1 requires (a) adding `format_version` to `SourceFrontmatter`,
   (b) validating it at parse time, (c) updating every example template
   to declare it, (d) updating `template-format.md` in the koto-author
   skill, and (e) threading the version through `compile_cached` and
   the cache key. This is a multi-file change with a migration story of
   its own — all to catch one kind of forward-compat bug. Option 2 is
   two attributes, an audit, and a regression test.

5. **Reversible.** If the audit misses an edge case and some downstream
   template fails to parse after the PR lands, dropping
   `deny_unknown_fields` is a one-line patch release. In contrast,
   retracting a format_version bump is harder because any template that
   declared `format_version: 2` now has to be rewritten.

The audit in Step 1 is the risk this decision carries. If the audit is
skipped or sloppy, a real template with an annotation field could break
on upgrade. The audit must produce a concrete written list of grepped
paths and results in the PR description, not a verbal "I checked." This
is why the status is `assumed` rather than `confirmed` — the evidence
base favors this option clearly, but the decision is conditional on the
audit coming back clean.

**Alternatives Considered**

- **Option 1 — Bump `format_version` to 2.** Rejected primarily because
  it requires significant new plumbing (add `format_version` to
  `SourceFrontmatter`, validate at parse time, update examples, update
  skill docs, thread through cache key) to work on markdown templates at
  all. The current `format_version` check only runs on pre-compiled JSON,
  which is not the common runtime path. Also rejected because it sets a
  precedent of serial bumps for every future field addition, conflicting
  with how v0.6.0 and v0.7.0 shipped. The error message is coarser than
  Option 2's. Worth revisiting ONLY if we later decide we want an
  explicit capability-version concept independent of serde strictness.

- **Option 3 — Compile-time warning without version bump.** Rejected
  because a warning emitted by the NEW binary does not help users on OLD
  binaries, which is the exact population this decision is meant to help.
  The new binary already "knows" the feature exists and does the right
  thing; the problem is the old binary.

- **Option 4 — Capability manifest (`requires: [batch_spawn]`).**
  Rejected as overengineered for the immediate need. Interesting
  long-term direction for multi-feature forward-compat, but would
  require both the old binary to understand the `requires` field (it
  won't — same serde silent-drop problem) AND a capability registry.
  Strictly worse than Option 2 for the old-binary signal, because
  without `deny_unknown_fields` the old binary would also silently drop
  `requires:`. Could be added LATER on top of Option 2 if we find we
  need more granular capability declarations.

- **Option 5 — Runtime feature detection.** Same rejection as Option 3:
  helps only the new binary, not the old one.

- **Option 6 — Nothing + docs.** Rejected because it accepts the known
  failure mode (silent no-op on old binaries) despite having a cheap
  fix available. Documentation alone does not reach a user who does not
  know they need to read it; the whole point is that the current failure
  is silent.

**Consequences**

What changes:

- `SourceState` and `TemplateState` reject any field outside their
  declared schema. Any typo in a state block (e.g., `transtitions:`
  instead of `transitions:`) now fails compile with a precise error —
  a usability win independent of the batch feature.
- Old binaries (v0.7.x and earlier) parsing post-batch templates fail
  at compile time with `unknown field` errors. This is the intended
  signal. Users upgrade and re-run.
- The koto-author skill's `template-format.md` reference gains one
  paragraph about struct-level strictness on state entries.

What becomes easier:

- Future fields added to `TemplateState` automatically get forward-
  compat enforcement for free. Every new field adds to the set of names
  the old binary does not recognize, and every old binary produces a
  clean error. No additional work per feature.
- Template authors get faster feedback on typos.
- The design doc no longer has to carry a "koto >= X.Y.Z required"
  docs note for the batch feature — the error message carries that
  signal implicitly.

What becomes harder:

- Any future use case where a template needs free-form annotations on
  state entries (e.g., a comment field, a linter-specific metadata
  block) must go through an explicitly-declared optional field on
  `TemplateState` rather than being silently tolerated. This is a
  minor constraint and arguably a feature — it keeps the schema
  auditable.
- The audit in Step 1 of the rollout is mandatory. If it is skipped
  and a downstream template has an unknown field, the PR ships a
  regression. Mitigation: run the audit as a grep command in CI on a
  prep branch before the batch-feature PR opens, commit the result
  to the PR description.

**Forward-compat gaps to fix in the same PR** (since Option 2 was
chosen, this applies only if the audit surfaces specific gaps — but for
completeness, the decision records what a format_version bump WOULD
have covered so the alternative path is documented):

If Option 1 had been chosen, the same version bump should have absorbed
any other forward-compat gap that could silently no-op on old binaries.
Candidates observed in the source scan:

- Any new field added to `Gate` (e.g., a future `retry_on_fail` field
  would silently drop on old binaries today).
- Any new field added to `VariableDecl`, `Transition`, or
  `ActionDecl` for the same reason.
- Any new gate type — though this is already covered by `compile_gate`'s
  `other =>` arm and would error cleanly on old binaries.

With Option 2 chosen, the correct follow-up is NOT to extend
`deny_unknown_fields` to every struct in the same PR — that blast
radius is too large to audit in one pass. Instead, add
`deny_unknown_fields` incrementally as each struct gains a new field,
with a per-struct audit each time. This keeps the risk bounded and
lets the pattern spread without a big-bang migration. A tracking
issue should capture the list of structs that still tolerate unknown
fields so future feature PRs know the checklist.

<!-- decision:end -->

---

```yaml
decision_result:
  status: "COMPLETE"
  chosen: "Narrow deny_unknown_fields on TemplateState (Option 2, scoped)"
  confidence: "high"
  rationale: >-
    Old binaries produce a precise, serde-generated error naming the unknown
    field and the state it sits in, matching koto's existing precedent of
    failing fast on unknown template surface (v0.7.0 did the same for new
    gate types via compile_gate's `other =>` arm). Bumping format_version
    would require plumbing the field through SourceFrontmatter and the cache
    key just to catch one bug class, and would set a precedent of serial
    bumps for every future field addition. Scope is narrowly limited to
    SourceState and TemplateState in the same PR as the new batch field.
  assumptions:
    - >-
      Decision 1 lands the batch hook as a field on TemplateState, not as a
      new gate type. If it lands as a new gate type, this decision collapses
      because compile_gate's `other =>` arm already handles forward-compat.
    - >-
      A pre-merge audit of .md template fixtures (test/functional/fixtures,
      plugins/koto-skills templates, shirabe templates, tsuku templates)
      finds no existing templates that rely on serde silently tolerating
      unknown fields as annotations. The audit must be committed to the PR
      description, not left verbal.
    - >-
      Decision was made in --auto mode without user confirmation. Status is
      assumed until the audit runs clean.
  rejected:
    - name: "Option 1: Bump format_version to 2"
      reason: >-
        Requires non-trivial new plumbing (add format_version to
        SourceFrontmatter, validate at parse time, thread through cache key,
        update all examples and docs) to actually catch .md templates at
        parse time — the current format_version check only runs on
        pre-compiled JSON. Sets a precedent of serial bumps for every
        future template-format expansion, conflicting with how v0.6.0 and
        v0.7.0 shipped. Error message is coarser than Option 2's
        serde-generated `unknown field` error.
    - name: "Option 3: Compile-time warning without version bump"
      reason: >-
        A warning from the NEW binary does not help users on OLD binaries,
        which is the exact population the decision is meant to help.
    - name: "Option 4: Capability manifest (`requires: [batch_spawn]`)"
      reason: >-
        Overengineered for the immediate need. Without deny_unknown_fields,
        old binaries would also silently drop the `requires` field, so
        Option 4 requires Option 2 underneath anyway. Strictly worse than
        Option 2 alone for the old-binary signal. Can be layered on top
        later if multi-feature capability declarations become necessary.
    - name: "Option 5: Runtime feature detection"
      reason: >-
        Same failure as Option 3 — helps only new binaries, not old ones.
    - name: "Option 6: Nothing + docs"
      reason: >-
        Accepts the known silent-no-op failure mode despite a cheap fix
        being available. Docs do not reach users who do not know to read
        them.
  report_file: "wip/design_batch-child-spawning_decision_3_report.md"
```
