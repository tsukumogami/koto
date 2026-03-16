# Architecture Review: DESIGN-template-format-v2

Reviewer role: architect-reviewer
Date: 2026-03-15
Input: `docs/designs/DESIGN-template-format-v2.md`, current source (`src/template/types.rs`, `src/template/compile.rs`, `src/cli/mod.rs`), upstream design (`docs/designs/DESIGN-unified-koto-next.md`)

---

## Question 1: Is the architecture clear enough to implement?

**Yes, with one gap.**

The design specifies exact Rust types (`Transition`, `FieldSchema`), exact YAML syntax, exact compiler validation rules, and exact files to modify. An implementer can open the three files and start coding from what's written.

The gap: the design doesn't specify how `SourceState.transitions` should deserialize. The v1 YAML uses `transitions: [done]` (a list of strings). The v2 YAML uses `transitions: [{target: deploy, when: {decision: proceed}}]` (a list of objects). The compiler needs a `SourceTransition` struct, and the design mentions it in Phase 2 ("Add `SourceTransition` and `SourceFieldSchema` deserialization types") but doesn't define its shape. This is minor -- the compiled `Transition` type is defined and the source type mirrors it -- but it means the implementer must decide the deserialization approach. Since there's no v1 compatibility requirement (no users), there's no ambiguity about dual-format parsing; this is just a missing paragraph, not a design hole.

## Question 2: Are there missing components or interfaces?

### 2a. Gate struct retains dead fields -- advisory

The design says `GATE_TYPE_FIELD_NOT_EMPTY` and `GATE_TYPE_FIELD_EQUALS` constants are removed, and the compiler rejects field gate types. But the `Gate` struct itself (`types.rs:44-56`) still has `field: String` and `value: String` fields that only field gates used. With only command gates surviving, the `Gate` struct could drop those fields. The design doesn't mention this cleanup.

**Severity: Advisory.** The dead fields don't cause structural harm -- they'll serialize as empty strings and be skipped by `skip_serializing_if`. No other code will copy the pattern because the constants are gone and the compiler rejects the gate types. But a cleanup note would prevent confusion during implementation.

### 2b. No consumer for `accepts` and `integration` in this issue -- blocking concern, but correctly deferred

The design adds `accepts: Option<BTreeMap<String, FieldSchema>>` and `integration: Option<String>` to the compiled `TemplateState`. Within this issue's scope (#47), nothing reads these fields. `koto next` extracts target names from transitions but ignores `accepts` and `integration`. The `expects` field derivation is #48's job; the integration runner is #49's.

This would normally be a state contract violation (adding fields with no consumer). The design explicitly acknowledges this with the note that `koto next` preserves current output format "for #48". Given the phased implementation plan from the upstream design, this is the correct sequencing -- the fields must exist in the compiled schema before downstream consumers can read them. The fields will have consumers within the planned scope.

**Severity: Not blocking**, provided #48 and #49 are implemented. If those issues are dropped or deferred indefinitely, the fields become dead schema.

### 2c. `koto next` output contract for transitions -- needs clarification

The design says `koto next` should "extract target names from structured transitions to preserve the current output format." Currently (`cli/mod.rs:296-298`), `koto next` serializes `template_state.transitions` directly as a `Vec<String>`. After v2, `transitions` becomes `Vec<Transition>`. The design implies extracting `.target` from each `Transition` and outputting `Vec<String>` -- but this isn't stated precisely.

The integration test at `tests/integration_test.rs:219-222` asserts `json["transitions"].is_array()`. If the implementer serializes `Vec<Transition>` directly (structured objects instead of strings), the test breaks and the CLI output contract changes. The design should explicitly state: "serialize as `Vec<String>` by mapping `|t| t.target.clone()`" to prevent an accidental output contract change.

**Severity: Advisory.** The design's intent is clear enough ("preserve the current output format"), but the implementation path could go wrong without an explicit transformation step.

### 2d. Existing plugin templates need updating

The hello-koto template (`plugins/koto-skills/skills/hello-koto/hello-koto.md`) uses v1 syntax (`transitions: [eternal]`). Since `SourceState.transitions` changes from `Vec<String>` to `Vec<SourceTransition>`, this template won't deserialize after v2 unless the compiler handles both formats or the template is updated.

The design says "States without `accepts` or `when` use the same syntax as v1 but with structured transition objects instead of plain strings." This appears to mean the YAML must change to `transitions: [{target: eternal}]` even for simple cases. The design should note that all existing templates (including hello-koto and test fixtures) must be migrated.

**Severity: Advisory.** No users means no migration burden, but the implementer needs to know the full blast radius. The integration tests (`tests/integration_test.rs`) embed v1 templates in source code (lines 10-28, etc.) that all need updating.

## Question 3: Are the implementation phases correctly sequenced?

**Yes.** The three phases have the right dependency order:

1. **Phase 1 (types)**: Defines the compiled schema. No dependencies.
2. **Phase 2 (compiler)**: Transforms YAML source to compiled types. Depends on Phase 1.
3. **Phase 3 (CLI + tests)**: Adapts consumers. Depends on Phase 2.

One practical concern: Phase 1 changes `TemplateState.transitions` from `Vec<String>` to `Vec<Transition>`. This breaks the compiler (Phase 2) and CLI (Phase 3) immediately. The implementer can't land Phase 1 with passing tests unless all three phases are done in one commit or the tests are temporarily adjusted. This is fine for a single PR but the phasing suggests three separate deliverables. Clarify whether these are implementation phases within a single PR or separate PRs.

## Question 4: Are there simpler alternatives we overlooked?

### 4a. Transition representation -- considered and correct

The design replaces `Vec<String>` with `Vec<Transition>` where `Transition` has `target` and optional `when`. An alternative would be to keep `transitions: Vec<String>` and add a separate `routing: BTreeMap<String, Vec<RoutingRule>>` field. This would avoid changing the existing type, but it splits transition information across two fields and creates a synchronization requirement (every routing target must appear in transitions). The design's single `Vec<Transition>` is the right call -- transitions and their conditions belong together.

### 4b. `serde_json::Value` for `when` conditions -- worth questioning

The `when` field uses `BTreeMap<String, serde_json::Value>` to allow future type extensions. This means any JSON value is syntactically valid in a `when` condition, and the compiler must validate by inspection rather than by type. An alternative is a dedicated `WhenValue` enum (`String(String)`, `Bool(bool)`, etc.) that enforces valid types at the deserialization boundary. The current design's approach is defensible for now (only string equality is used in practice) but will accumulate match arms as types are added. Not blocking -- just a type-safety trade-off to revisit when numeric or boolean conditions are needed.

### 4c. `format_version` bump strategy

The design bumps `format_version` from 1 to 2. The validator currently hard-rejects anything other than 1 (`types.rs:74-78`). The design should note that `validate()` must accept version 2 (or both 1 and 2, but given no users, just 2). This is implied but worth making explicit to avoid a bug where the compiler emits version 2 but the validator rejects it.

---

## Summary of findings

| # | Finding | Severity | Location |
|---|---------|----------|----------|
| 1 | `SourceTransition` deserialization shape not defined | Advisory | Design, Phase 2 |
| 2 | `Gate` struct retains dead `field`/`value` fields | Advisory | `types.rs:48-50` |
| 3 | `accepts`/`integration` have no consumer in this issue | Not blocking (correctly deferred to #48/#49) | `types.rs` (proposed) |
| 4 | `koto next` transition output extraction not precisely specified | Advisory | Design, Phase 3 / `cli/mod.rs:297` |
| 5 | Existing templates and test fixtures need v1-to-v2 migration | Advisory | `hello-koto.md`, `integration_test.rs`, `compile.rs` tests |
| 6 | Phase 1 type change breaks compilation -- phases can't land independently | Advisory | Design, Implementation Approach |
| 7 | `validate()` must accept `format_version: 2` | Advisory | `types.rs:74` |

No blocking findings. The design fits the existing architecture: it modifies types, compiler, and CLI through their established boundaries without introducing parallel patterns. The `Transition` and `FieldSchema` types extend the existing compiled schema pattern. The compiler pipeline (YAML source -> deserialization -> transformation -> compiled types -> validation) is preserved.
