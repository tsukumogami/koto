<!-- decision:start id="path-resolution-contradictions" status="assumed" -->
### Decision: Path-resolution contradictions (CD14)

**Context**

Round 1 walkthrough pair 3c surfaced six findings against Decision 4's
template path resolution and the `BatchError` enum. Five are
contradictions or undefined corners the design must close; one
(resolution winner surfaced in scheduler output) is a separable UX
improvement deferred out of CD14.

Decision 4's core mechanism stands: `template_source_dir` captured at
`koto init` on the state file header, `submitter_cwd` captured at
`koto next --with-data` on the `EvidenceSubmitted` event, resolution
order (absolute → template_source_dir → submitter_cwd → error). CD14
resolves five gaps within that mechanism:

1. The design contradicts itself on whether a single bad child template
   fails the whole submission (R1 + Data Flow at lines 1862-1868) or
   just that task (`BatchError::TemplateResolveFailed { task, ... }`
   variant shape at line 1603).
2. Decision 4's fallback text says "on ENOENT" -- but a `template_source_dir`
   that is absent entirely (pre-D4 state file) never reaches the ENOENT
   check.
3. `TemplateResolveFailed` conflates "file not found" with "file found
   but failed to compile." `paths_tried` is meaningless for the compile
   case.
4. Phase 3's "DAG depth of 50" has three plausible definitions.
5. Security Considerations mentions `template_source_dir` exposure but
   doesn't surface the cross-machine portability limitation.

**Assumptions**

- CD11 will produce an error envelope that accommodates (a) per-task
  scheduler errors so `TemplateNotFound` / `TemplateCompileFailed` on a
  single task don't abort siblings, and (b) a scheduler warnings
  vector so the absent-source-dir and stale-source-dir warnings have
  somewhere to land. If CD11's envelope omits warnings, the warnings
  degrade to log-only emission; the scheduler behavior stands.
- Agents reading per-task errors understand that a failing task does
  not abort siblings.
- The repo-layout assumption from Decision 4 ("repo paths are stable
  across machines") holds for the common same-user same-layout case
  and breaks on cross-home-layout migrations (Linux ↔ macOS, different
  usernames, container paths).
- `std::path::Path::exists()` on `template_source_dir` is a cheap
  probe the scheduler can call once per tick to detect cross-machine
  staleness.

**Chosen: Per-task template failures, absent-source-dir skips cleanly, split variant, node-count depth, documented portability with runtime warning**

Five coordinated resolutions, one per sub-question.

**1. R1 is per-task, not whole-submission.**

Child-template compile and resolve failures are per-task. A single task
entry with a missing or broken template produces a per-task error in
`SchedulerOutcome` and does **not** abort sibling spawns. Whole-submission
failures are restricted to properties of the task graph: R3 (cycles),
R4 (dangling refs), R5 (duplicate names). These produce
`BatchError::InvalidBatchDefinition` and reject the submission pre-append.

Update the R1-R7 table in the design so R1 reads "Child template
resolvable and compilable (per-task; failures are reported in
`SchedulerOutcome.errored`, siblings continue)." Correct the
ambiguous sentence in Data Flow step 4 at lines 1862-1868 to
enumerate only R3/R4/R5 as whole-submission failures.

**2. Absent `template_source_dir` skips step (b), falls through to
`submitter_cwd`, and emits a warning.**

Extend Decision 4's resolution order to handle the absent case
explicitly:

- (a) Absolute paths pass through.
- (b) If `template_source_dir` is `Some(dir)`, join the relative path
  against it. If the file exists, use it. If ENOENT, fall through to (c).
- **(b') If `template_source_dir` is `None`, skip (b) entirely. Emit
  `SchedulerWarning::MissingTemplateSourceDir` once per scheduler tick.
  Fall through to (c).**
- (c) Join against `submitter_cwd`. If the file exists, use it. If
  ENOENT, fall through to (d).
- (d) Return `BatchError::TemplateNotFound { task, paths_tried }`
  listing every attempted path.

Absent and ENOENT are treated identically for fallback purposes. The
warning distinguishes them so agents see why resolution might feel
surprising on pre-D4 workflows.

**3. Split `BatchError::TemplateResolveFailed` into two variants.**

```rust
pub enum BatchError {
    // ... unchanged variants ...

    /// The resolver tried every configured base and none contained the
    /// template file. paths_tried lists every path attempted, in order.
    TemplateNotFound {
        task: String,
        paths_tried: Vec<String>,
    },

    /// A path was found and read succeeded, but template compilation
    /// (frontmatter parsing, state-graph validation, etc.) failed.
    TemplateCompileFailed {
        task: String,
        path: String,
        compile_error: String,
    },
}
```

Remove `TemplateResolveFailed`. Every call site that previously returned
`TemplateResolveFailed` now returns one of the two new variants based
on whether the failure was path resolution or template compilation. CD11
will map these to the error envelope wire shape; CD14 commits only to
the variant names and fields.

**4. DAG depth = longest root-to-leaf path, counted in nodes.**

Define depth precisely: a **root** is a task with empty `waits_on`; a
**leaf** is a task with no sibling's `waits_on` referencing it. Depth
is the number of nodes in the longest path from any root to any leaf.
A linear chain of 51 tasks has depth 51; this exceeds the limit.

Update Phase 3's limit block (line 2128) and Security Considerations'
resource-bounds block (line 2289) to spell out "longest dependency
chain, counted in tasks." Update `BatchError::LimitExceeded` messages
for `which == "dag_depth"` to say: `"Longest dependency chain has N
tasks; limit is 50."`

The "longest any-to-any" confusion is foreclosed: in a DAG, any path
extends to a root and leaf, so root-to-leaf is the natural maximum.
Nodes (not edges) matches user intuition ("I wrote 51 tasks").

**5. Document cloud-sync portability limitation and warn at resolution
time on stale `template_source_dir`.**

Add a paragraph to Security Considerations' path-resolution section
(after line 2312):

> **Cross-machine portability.** `template_source_dir` on the state
> file header and `submitter_cwd` on `EvidenceSubmitted` events capture
> absolute filesystem paths at init and submission time. These paths
> are stable across machines only when the user runs koto under the
> same home-directory layout on every machine (e.g., Linux home-dir
> sync). Cross-layout migrations -- Linux to macOS, different
> usernames, containerized checkouts with different mount paths -- can
> leave both captured bases pointing at locations that don't exist on
> the current machine. When the scheduler resolves a template against
> a `template_source_dir` that does not exist on the current machine,
> it emits `SchedulerWarning::StaleTemplateSourceDir` and falls through
> to the `submitter_cwd` base (which may also be stale). If both bases
> fail, `BatchError::TemplateNotFound` lists every attempted path so
> the user can diagnose which leg of the resolution is stale. A future
> `koto session retarget` subcommand may provide a rewrite path for
> header fields; out of scope for v1.

The runtime warning is emitted when `Path::new(template_source_dir).exists()`
is false at scheduler start. One probe per tick. Deduplicated per
`template_source_dir` value per tick (if the header value doesn't change
within a tick, one warning).

Defer `koto session retarget` (a retargeting subcommand) and storing a
repo-relative form of `template_source_dir` as future extensions. Both
are real portability improvements; neither is required for CD14 to
close the round-1 gaps.

**Rationale**

Each sub-question admits one clearly-better answer once the constraints
and variant shapes are considered:

- **Per-task (1)** matches the existing variant's `task: String` field,
  preserves the partial-success property that makes dynamic additions
  viable, and aligns the error granularity with the per-task R7
  (sibling collision) that already exists.
- **Skip-on-absent (2)** is the only option that preserves backward
  compat for pre-D4 state files. Erroring would break the explicit
  constraint; silent fallback would hide diagnostic signal. Warning-
  plus-fallback lands in the middle.
- **Split variant (3)** is required by the new constraint ("distinguish
  not-found from compile-failed programmatically"). 3A is the clean
  form; a single variant with a `kind` discriminator (3B) carries dead
  fields for every call and needlessly mixes two failure modes.
- **Nodes-on-path (4)** is the user's mental model and the least
  surprising boundary. Edge-count depth leaves users confused at the
  50/51 boundary ("I wrote 51 tasks and koto says depth 50 is the
  limit -- which is it?"). "Any-to-any" doesn't exist as a distinct
  option in DAGs.
- **Doc + warning (5)** surfaces the limitation at the right moments
  (doc for expectation-setting; warning when it's about to bite)
  without committing to a mechanism fix that belongs in a separate
  design.

None of the five answers interact with each other destructively, so
they ship as one coordinated update to Decision 4, the `BatchError`
enum, R1-R7's placement, Phase 3's limit block, and Security
Considerations.

**Alternatives Considered**

Per sub-question; each winner is drawn from a small set.

- **Sub-question 1 alternatives.** *Whole-submission halt* (1B):
  inconsistent with the variant's `task: String` field and kills
  partial-success. *Hybrid: per-task for compile, whole for not-found*
  (1C): arbitrary; not-found is no more a graph property than
  compile-failed.
- **Sub-question 2 alternatives.** *Error at submission* (2B): breaks
  the pre-D4 backward-compat constraint. *Silent fallback* (2C):
  hides the cause of later surprise.
- **Sub-question 3 alternatives.** *Keep one variant, add `kind`
  field* (3B): bloats the variant with fields meaningful to only one
  kind; still forces agents to pattern-match a nested discriminator.
  *Status quo* (3C): violates the new distinguishability constraint.
- **Sub-question 4 alternatives.** *Edges on longest path* (4B):
  matches CS convention but produces off-by-one surprises at the
  50/51 boundary. *Any-to-any* (4C): collapses to root-to-leaf in a
  DAG; not a real alternative.
- **Sub-question 5 alternatives.** *Doc only* (5B): leaves agents with
  no runtime signal when resolution is about to fail cross-machine.
  *Retarget subcommand* (5C): real fix, out of CD14's scope. *Repo-
  relative path in header* (5D): real fix, out of CD14's scope;
  requires git-root detection and new schema field.

**Consequences**

Easier:

- Per-task errors for `TemplateNotFound` and `TemplateCompileFailed`
  let dynamic additions keep their partial-success guarantee. An agent
  that mistypes one task's template sees that task fail without
  reverting the rest of the batch.
- Pre-D4 state files keep working. Agents upgrading koto see a
  clear warning on first batch submission rather than an opaque error.
- Agents can programmatically distinguish "my path is wrong" from "my
  template file is broken," enabling better recovery suggestions in
  higher-level skills.
- DAG-depth errors cite a concrete number (tasks in chain) the user can
  act on.
- Cross-machine failure modes produce a targeted warning pointing at
  the stale base, not a generic "file not found" trace.

Harder:

- `BatchError` gains two variants; every call site that matched on
  `TemplateResolveFailed` migrates to the two replacements. Small, but
  touches the scheduler error-handling path.
- `SchedulerOutcome` gains a warnings vector (assuming CD11
  accommodates this). Serialization and tests must cover the new
  field.
- R1's table entry is more complex (per-task notation vs the simple
  "runtime check" it used to say). One docs-only change, but it
  affects readers' mental model of the R-rule taxonomy.
- The Security Considerations paragraph is new prose that must age
  well as cloud-sync support matures. If a future `koto session
  retarget` lands, this paragraph should get updated to reference it.
- `SchedulerWarning::StaleTemplateSourceDir` requires a `Path::exists()`
  probe per tick. Cost is trivial (one stat call) but technically
  a new I/O per tick.

**Migration note**

- Pre-D4 state files (no `template_source_dir` on header) continue to
  deserialize cleanly (`#[serde(default)]` on the field remains). On
  first batch submission after upgrade, agents see
  `SchedulerWarning::MissingTemplateSourceDir` and resolution falls
  through to `submitter_cwd`. Users who want the richer resolution
  path can re-init the workflow (`koto init --template <path>
  --force`) to populate `template_source_dir`.
- Call sites matching on `BatchError::TemplateResolveFailed` require
  mechanical update to match the two new variants. No wire-format
  compatibility issue because the enum isn't persisted -- it's a
  runtime error that maps into the CLI response via CD11.
- Existing compiled templates and state files don't need to change;
  CD14 is entirely about error classification, warning emission, and
  documentation.
<!-- decision:end -->

---

## Decision Result Summary

```yaml
decision_result:
  status: "COMPLETE"
  chosen: "Per-task template failures, absent-source-dir skips cleanly, split TemplateResolveFailed, node-count depth, documented cloud-sync portability with runtime warning"
  confidence: "high"
  rationale: >-
    Each sub-question admits one clearly-better answer given the existing
    variant shapes and explicit constraints. Per-task errors match the
    BatchError::TemplateResolveFailed { task, ... } variant's shape and
    preserve dynamic-additions partial success. Absent template_source_dir
    treated as ENOENT preserves pre-D4 backward compat. Splitting the
    variant satisfies the new "distinguish not-found from compile-failed"
    constraint. Node-count depth matches user intuition and produces clear
    error messages. Documenting portability plus a runtime warning
    surfaces the limitation at the right moments without committing to a
    mechanism fix that belongs in a separate design. The five resolutions
    are non-conflicting and ship as one coordinated update.
  assumptions:
    - "CD11 will expose a warnings vector on SchedulerOutcome for the new SchedulerWarning variants"
    - "Agents reading per-task errors understand that a failing task does not abort siblings"
    - "std::path::Path::exists() on template_source_dir is a cheap per-tick probe"
    - "Repo-layout stability across machines is the norm, not the exception -- the portability limitation matters but isn't the common case"
  rejected:
    - name: "Whole-submission halt on bad template"
      reason: "Inconsistent with variant's task-level field; kills partial-success needed by dynamic additions"
    - name: "Error at submission when template_source_dir is absent"
      reason: "Breaks pre-D4 backward-compat constraint"
    - name: "Silent fallback on absent template_source_dir"
      reason: "Hides diagnostic signal agents need to understand later failures"
    - name: "Single TemplateResolveFailed variant with kind discriminator"
      reason: "Bloats variant with fields meaningful to only one kind; violates distinguishability constraint"
    - name: "Edge-count DAG depth"
      reason: "Produces off-by-one surprises at the 50/51 boundary relative to user intent"
    - name: "Doc-only portability note, no runtime warning"
      reason: "Leaves agents with no signal when cross-machine resolution is about to fail"
    - name: "koto session retarget subcommand"
      reason: "Real fix but out of CD14's scope; noted as future extension"
    - name: "Repo-relative template_source_dir alongside absolute"
      reason: "Real fix but requires git-root detection and new header field; out of CD14's scope"
  migration_note: >-
    Pre-D4 state files continue to deserialize via #[serde(default)]. On
    first batch submission after upgrade, agents see
    SchedulerWarning::MissingTemplateSourceDir; resolution falls through
    to submitter_cwd. BatchError call sites that matched on
    TemplateResolveFailed migrate mechanically to the two new variants.
    No persisted schema change; CD14 is entirely about error
    classification, warning emission, and documentation.
  affects:
    - "Decision 4 resolution order (adds explicit absent-source-dir branch)"
    - "BatchError enum (removes TemplateResolveFailed; adds TemplateNotFound and TemplateCompileFailed)"
    - "R1-R7 table (R1 reclassified as per-task)"
    - "Data Flow step 4 text (lines 1862-1868; enumerate only R3/R4/R5 as whole-submission)"
    - "Phase 3 limit block (line 2128; DAG depth definition)"
    - "Security Considerations path-resolution section (new cross-machine paragraph)"
    - "SchedulerOutcome warnings vector (CD11 coordinates wire shape)"
  report_file: "wip/design_batch-child-spawning_decision_14_report.md"
```
