# Coherence Review: batch-child-spawning artifacts

## Scope

Cross-checked the design doc, walkthrough, process guide, summary,
decisions log, coordination manifest, cross-validation notes,
exploration scope/findings/decisions/crystallize for contradictions,
stale references, numbering inconsistencies, and wip/ fragility.

---

## 1. Contradictions between design doc and walkthrough

### 1a. `expects` response structure: `expects.tasks` vs `expects.fields.tasks`

The walkthrough (interaction 2, line 285) shows the `expects` block as:

```json
"expects": {
  "tasks": {
    "type": "tasks",
    "item_schema": { ... }
  }
}
```

The design doc Decision 8 (line 1296) shows it wrapped in a `fields`
sub-object:

```json
"expects": {
  "fields": {
    "tasks": {
      "type": "tasks",
      "item_schema": { ... }
    }
  }
}
```

One of these is wrong. The walkthrough omits `fields`; the design doc
includes it. A future implementer looking at one artifact but not the
other will produce the wrong JSON shape. **Severity: high.** This
directly affects the response schema that agents parse.

### 1b. Template YAML and core response shapes match

The walkthrough's `coord.md` template YAML matches the design doc's
Decision 1 template example exactly: same `plan_and_await` state,
same `materialize_children` fields (`from_field`, `failure_policy`,
`default_template`), same `children-complete` gate, same transition
condition. The `gate_blocked` response shapes in the walkthrough
(interactions 3, 6, 8) match the design doc's Decision 5 gate output
schema (per-child `outcome`, aggregate counts, `blocked_by`). No
field name differences found.

### 1c. Walkthrough directive text minor drift

The walkthrough's directive (interaction 2, line 281) includes the
phrase `template="impl-issue.md"` in step 3 of the task-building
instructions, but the walkthrough's own task list (line 218) and the
design doc both show that `template` is omitted when `default_template`
is set. The directive text telling agents to include `template` in each
entry contradicts the example that omits it. This is a cosmetic issue
since `template` is optional, but the mismatch could confuse someone
reading both.

---

## 2. Stale references

### 2a. `src/engine/batch.rs` in the design doc's architecture diagram

The design doc's architecture diagram (line 1418) labels the scheduler
module as `src/engine/batch.rs`. However, the design doc's own
Decision E3 text (line 286, 305), the Solution Architecture prose
(line 1547), the Implementation Approach (line 2025), and all
Phase 3 deliverables consistently say `src/cli/batch.rs`. The diagram
was likely drawn before the Phase 6 architecture review recommended
moving the module from `src/engine/` to `src/cli/`.

The following wip/ files also reference `src/engine/batch.rs`:
- `design_batch-child-spawning_cross_validation.md` (line 68)
- `design_batch-child-spawning_decisions.md` (line 21)
- `design_batch-child-spawning_decision_5_report.md` (line 1395)
- `design_batch-child-spawning_decision_6_report.md` (lines 15, 207, 215, 248)
- `explore_batch-child-spawning_findings.md` (lines 189, 276)
- `explore_batch-child-spawning_crystallize.md` (line 62)
- All five `research/explore_*_lead-koto-integration.md` references

The wip/ files are throwaway, but the **design doc diagram is the
permanent artifact** and should be fixed before merge. **Severity:
medium.**

### 2b. `type: json` in wip/ files

Decision E7 in the design doc explicitly replaced the generic `json`
field type with the purpose-built `tasks` type. The design doc itself
is clean, but these wip/ files still reference `type: json`:
- `design_batch-child-spawning_decision_1_report.md` (lines 78, 368,
  446, 555, 566)
- `design_batch-child-spawning_decision_8_report.md` (lines 12, 84)
- `research/explore_batch-child-spawning_r1_lead-evidence-shape.md`
  (line 396)

The walkthrough process guide correctly notes the replacement occurred
(PROCESS.md lines 12, 88). Since these are wip/ files deleted before
merge, this is low severity.

### 2c. `json` field type in exploration findings

`explore_batch-child-spawning_findings.md` lines 202 and 279 still
say `json` field type in the decisions and accumulated understanding
sections. These sections reflect the exploration's output before the
design phase replaced `json` with `tasks`. The walkthrough PROCESS.md
correctly notes the replacement, and the design doc is clean, but the
findings file tells a story that's one step behind.

### 2d. `batch_directive` — correctly rejected, no stale usage

`batch_directive` appears only in the Decision 7 report (as a rejected
alternative) and in the design doc's own "Alternatives considered"
section. No artifact uses it as if it were adopted. Clean.

### 2e. Two-state `plan -> await` pattern

The exploration findings (line 230) describe the feature shape using
a `batch` hook name and don't mention the single-state fan-out pattern
by name. The design doc explicitly documents why single-state fan-out
is required (lines 598-608) and uses `plan_and_await` as the canonical
state name. The Decision 1 report (line 435) still shows a `plan` /
`await` two-state pattern as an example for a "release.md" template,
which contradicts the design doc's requirement that the hook and gate
must be on the same state. This is a wip/ file issue only.

### 2f. `awaiting_children` state name in design doc

The design doc uses two different state names for the parent's
fan-out state: `plan_and_await` (in the template examples and data
flow walkthrough) and `awaiting_children` (in the scheduler-tick
ordering section at line 734, the retry section at line 1001, and the
Decision 6 `koto status` example at line 1083). These are from
different example templates, so it's not technically a contradiction,
but a reader may not realize they refer to the same architectural
role. The Decision 6 example uses `awaiting_children` while the
Decision 1 and 7 examples use `plan_and_await`.

---

## 3. Decision numbering consistency

### 3a. Decisions log says "6 decisions" but 8 exist

The decisions log header (line 10) says "6 decisions after merging
coupled questions" and lists exactly 6. The coordination manifest
correctly lists all 8 decisions (1-8). The PROCESS.md notes that
Decisions 7 and 8 were surfaced during the walkthrough. The decisions
log was never updated to reflect the addition. **Severity: medium** --
someone reading the decisions log without the coordination manifest
would miss two decisions.

### 3b. Cross-validation says "all six decisions" but only covers 1-6

The cross-validation notes (line 5) say "All six decisions composed
cleanly." This was written before Decisions 7 and 8 existed. The
cross-validation was never re-run with the full set. Decisions 7 and
8 don't interact with the others in complex ways (7 is "no new
features," 8 adds `default_template` and `item_schema`), so the gap
is unlikely to hide a real conflict, but the document claims
completeness it doesn't have.

### 3c. Design doc Decision Outcome says "six decisions"

The design doc's Decision Outcome section (lines 1347, 1357) says
"The six decisions interlock" and "All six decisions are consistent."
This should say eight. The design doc's Consequences section (line
2130) also says "six decisions' worth of schema changes." **Severity:
medium** -- the design doc is the permanent artifact and
under-counts its own decisions.

### 3d. E1-E8 vs E1-E9 in compiler validation

The compiler validation table in Decision 1 lists 9 error rules
(E1-E9), where E9 validates `default_template` (added by Decision 8).
But the architecture diagram (line 1473) and Phase 2 deliverables
(line 2011) both say "E1-E8 errors and W1-W2 warnings." E9 was
appended to the table but the references were not updated. **Severity:
low** -- the table is authoritative and a reader would count the rows.

### 3e. Exploration decision numbering (E1-E8) is consistent

The design doc's "Decisions Settled During Exploration" section uses
E1-E8 consistently. These map to the exploration findings' decisions
section cleanly. No off-by-one.

---

## 4. Exploration findings vs design decisions

### 4a. All exploration decisions are reflected

The exploration findings' "Decisions" section lists 8 items:
1. Reading A primary -- maps to design E1
2. Storage: disk derivation -- maps to design E2
3. CLI-level scheduler tick -- maps to design E3
4. Child naming `<parent>.<task>` -- maps to design E4
5. Skip-dependents default -- maps to design E5
6. `@file.json` prefix -- maps to design E6
7. `json` field type -- maps to design E7 (renamed to `tasks`)
8. State-level `batch`/`materialize_children` hook -- covered in
   design Decision 1

All are accounted for. The exploration's `json` -> design's `tasks`
rename is the only semantic change, and it's properly documented in
the design doc's E7 section.

### 4b. Exploration gaps match design decisions

The findings listed 5 gaps: atomic spawn window, forward-compat,
child-template resolution, retry mechanics, observability. These
map exactly to design Decisions 2, 3, 4, 5, 6. No gaps were
overlooked, no design decisions were made without an exploration
basis.

---

## 5. wip/ references that would break after cleanup

### 5a. Design doc section that references wip/ files

The design doc (line 105-107) says: "Exploration is documented in
`wip/explore_batch-child-spawning_*.md` and the five research files
in `wip/research/`. Those artifacts are the primary input for this
design." After wip/ cleanup, these paths will be dead. The sentence
is context, not load-bearing -- the design doc is self-contained and
a reader can understand the decisions without the exploration
artifacts. **Severity: low.**

### 5b. Decision 2 crash-failure walkthrough reference

Line 705 says "(in `wip/design_batch-child-spawning_decision_2_report.md`)."
This is a pointer to detailed analysis that won't exist after merge.
The design doc itself summarizes the key crash-failure properties, so
this is supplementary, but a future reader clicking the link would
get a 404. **Severity: low.**

### 5c. Decision 5 resume walkthrough reference

Line 1034-1035 says "(full detail in
`wip/design_batch-child-spawning_decision_5_report.md`, section
'Walkthrough: 10-task batch with failure + crash')." Same pattern as
5b. The design doc includes a high-level resume summary but defers
the 4-scenario crash walkthrough to the report. After cleanup, this
detail is lost. **Severity: low-to-medium** -- the crash scenarios
are operationally important, but the design doc captures enough to
reconstruct them.

### 5d. PROCESS.md lists wip/ artifact paths

PROCESS.md (lines 106-114) enumerates wip/ paths including
`wip/design_batch-child-spawning_decision_[1-8]_report.md` and
research files. PROCESS.md itself is a wip/ file and will be deleted,
so this is not an issue.

### 5e. Design summary lists wip/ exploration artifacts

The design summary (lines 47-55) lists exploration artifact paths.
The summary itself is in wip/ and will be deleted. Not an issue.

---

## 6. Other observations

### 6a. Walkthrough shows scheduler outcome on same tick as evidence submission

The walkthrough interaction 3 shows that submitting evidence AND
getting a scheduler outcome with spawned children happen in the same
response. The design doc's "Scheduler-tick ordering on first
submission" section (lines 728-763) describes a two-call contract
where the gate result is finalized before the scheduler runs, but
the scheduler result is attached to the same response. The
walkthrough is consistent with this -- the gate shows `completed: 0`
while `scheduler.spawned` shows children -- but the design doc warns
this needs documentation. The walkthrough could explicitly note this
subtlety.

### 6b. Walkthrough failure scenario's `all_complete: true` with failures

The walkthrough's failure scenario (line 559) shows
`all_complete: true` even though one child failed. This matches the
design doc's Decision 5 definition where `all_complete` means
`pending == 0 AND blocked == 0` (line 979). Consistent.

---

## Top 3 inconsistencies

1. **`src/engine/batch.rs` in the design doc's architecture diagram
   (line 1418) contradicts the rest of the design doc which says
   `src/cli/batch.rs`.** This is in the permanent artifact's most
   visual summary of the architecture. A reader scanning the diagram
   gets the wrong module location. The prose text is correct (Decision
   E3, Solution Architecture, Implementation Approach all say
   `src/cli/`), but the diagram is what people look at first.

2. **The design doc says "six decisions" in three places (lines 1347,
   1357, 2130) but there are eight.** Decisions 7 and 8 were added
   during the walkthrough and the Decision Outcome and Consequences
   sections were not updated. A future reader counting decisions in the
   Considered Options section (8 decisions) and then reading "the six
   decisions interlock" in the Decision Outcome section will be
   confused about which two don't count.

3. **`expects` response structure differs between the walkthrough
   (`expects.tasks`) and the design doc (`expects.fields.tasks`).** An
   implementer building the `derive_expects` function will produce one
   shape or the other. The walkthrough is the more concrete artifact
   (full JSON responses an agent would see), but the design doc is
   the authoritative spec. One must be corrected to match the other.
