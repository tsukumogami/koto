# Lead: CLI Augmentations for Agent-Provided Narrative Context

## Findings

### Current StateFileHeader Structure

The header (line 1 of every `.state.jsonl` file) contains:

```
schema_version, workflow, template_hash, created_at,
parent_workflow (optional), template_source_dir (optional), session_id
```

There is no human-readable title, description, or intent field. The workflow name is the only identifier shown in the dashboard list view, and it's machine-generated (e.g., `task_session-feed-issue-1`).

### Current EventPayload Variants

Fifteen variants exist. Relevant to narrative context:

- `WorkflowInitialized` — records `template_path` and `variables` but no intent
- `EvidenceSubmitted` — `state` + `fields` (arbitrary key-value) + `submitter_cwd`; no human summary
- `Transitioned` — records `from`, `to`, `condition_type`; no note field
- `DirectedTransition` — has an optional `rationale: Option<String>` field (already present)
- `Rewound` — has an optional `rationale: Option<String>` field (already present)
- `DecisionRecorded` — records arbitrary JSON `decision` per-state; already serves as structured note, but is not surfaced in dashboard detail pane

### Dashboard Data Layer

`src/cli/dashboard_data.rs` maintains a `CachedSession` per session. It stores:
- `header: StateFileHeader` — used for `parent_workflow` to build the tree
- `current_state: Option<String>` — derived from log replay
- `is_terminal`, `is_blocked` — derived booleans

`DetailData` (loaded on demand for the focused session) contains:
- `gate_name`, `command`, `result`, `elapsed`
- `evidence: Vec<EvidenceEntry>` — each with `state` and `fields`

There is no `title`, `intent`, or `summary` field anywhere in the dashboard pipeline.

### Dashboard Render Layer

The list view shows four columns: **Name**, **State**, **Elapsed**, **Tasks**. The "Name" column is the raw session name. No human-readable description is shown.

The detail pane shows: gate name/result, command, and up to 3 evidence entries as raw JSON field dumps (e.g., `[gather] {"file": "plan.md", "decision": "approve"}`).

### Additive Field Pattern

The codebase has an established pattern for optional header fields:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub parent_workflow: Option<String>,
```

Fields that are `Option<T>` with these serde attributes round-trip cleanly against older state files — absent fields deserialize to `None` and `None` fields are omitted from serialization. The `schema_version` guard only applies to structural breaking changes; additive optional fields do **not** require a bump.

The same pattern is used for `template_source_dir` and `session_id` (the latter uses `#[serde(default)]` without `skip_serializing_if`).

### Existing Rationale Surface

`DirectedTransition` and `Rewound` already carry an optional `rationale: Option<String>`. This is exposed via `koto next --rationale` and `koto rewind --rationale`. The field is visible via `koto query --events` but not rendered in the dashboard.

### WorkflowMetadata

`WorkflowMetadata` (returned by `koto workflows`) derives from the header. It exposes `name`, `created_at`, `template_hash`, `parent_workflow`. Any new header field would need to be added here too for consistent `koto workflows --json` output.

### init_child_core Header Construction

The `StateFileHeader` is built in `init_child_core` (line 467-475 of `src/cli/init_child.rs`). Adding a new optional field to the header only requires updating this construction site and the `Init` command's arg parser. No other structural change is needed.

### DecisionRecorded Not Used by Dashboard

`DecisionRecorded` events are recorded by `koto decisions record --with-data` and support per-state structured notes (arbitrary JSON with `choice` + `rationale` fields encouraged). However, `dashboard_data.rs` does not read `DecisionRecorded` events at all — `DetailData` only surfaces gate evaluations and evidence. This is an existing gap: decisions are invisible in the dashboard even though they carry the richest narrative content.

---

## Implications

### Minimal Change Set for Maximum Dashboard Value

**Change 1 — `--intent` flag on `koto init` → header field `intent: Option<String>`**

This is the single highest-leverage change. Adding:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub intent: Option<String>,
```

to `StateFileHeader` and `--intent "Implement issue #42: add retry logic"` to `koto init` gives every workflow a human-readable purpose from the moment it starts. The dashboard list view can show this in the Name column (or a new Description column) immediately.

No schema version bump is needed. `WorkflowMetadata` needs the same field for `koto workflows --json` consistency.

**Change 2 — `summary` field on `EvidenceSubmitted`**

Add:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub summary: Option<String>,
```

to `EventPayload::EvidenceSubmitted`. Agents already submit evidence at state boundaries; adding a `summary` key lets them narrate what happened in one sentence. The dashboard detail pane currently renders `fields` as raw JSON — it could instead render `summary` as a readable line and fold the raw fields into a secondary display.

The existing `--with-data` mechanism is the natural carrier. No new command needed: agents pass `{"summary": "Found 3 open issues, created branch", "files_changed": 5}` and the summary key is extracted at render time.

**Change 3 — Expose `DecisionRecorded` in detail pane**

`DecisionRecorded` events already exist and carry structured narrative (`choice` + `rationale`). The dashboard currently ignores them. Adding them to `DetailData` and rendering them in the detail pane costs no new API surface — it's a pure dashboard-side change.

### What Does NOT Need a New Command

A `koto annotate` command would add complexity without much benefit over the existing evidence and decision paths. Agents can attach narrative to `EvidenceSubmitted` via the `summary` field and structured rationale via `DecisionRecorded`. The only gap is the workflow-level intent, which `--intent` on `koto init` covers.

### Backwards Compatibility

All three changes use `#[serde(default, skip_serializing_if = "Option::is_none")]`. Older state files without these fields deserialize cleanly. No `CURRENT_SCHEMA_VERSION` bump is needed. The `Evidence` deserialization helper struct (`EvidenceSubmittedPayload`) and `WorkflowInitialized` struct would each need the new optional field added with `#[serde(default)]`.

### API Surface Summary

| Change | CLI surface | State file change | Dashboard change |
|--------|-------------|-------------------|------------------|
| Intent on init | `koto init --intent "..."` | `StateFileHeader.intent: Option<String>` | Show in list Name column or tooltip |
| Evidence summary | `{"summary": "...", ...}` in `--with-data` | `EvidenceSubmitted.summary: Option<String>` | Render summary line in detail pane |
| Decisions in detail | none | none | Read `DecisionRecorded` in `read_detail()` |

---

## Surprises

1. **`DecisionRecorded` is invisible to the dashboard.** This event type was clearly designed for narrative context (`choice` + `rationale` structure, per-state scoping), but `dashboard_data.rs` never reads it. The richest existing narrative surface is completely dark in the UI.

2. **`DirectedTransition.rationale` and `Rewound.rationale` exist but aren't rendered.** These fields are already on-wire and populated by `--rationale` flags. They don't appear in the dashboard either. These are two more dark narrative fields.

3. **The `EvidenceSubmitted.fields` is rendered as a raw JSON blob** in the detail pane (`serde_json::to_string(&entry.fields)`). Agents submit structured evidence but the dashboard shows it as `{"file":"plan.md","decision":"approve"}`. A `summary` field added to `EvidenceSubmitted` would immediately improve this without changing how existing fields are stored.

4. **`session_id` uses `#[serde(default)]` without `skip_serializing_if`**, meaning old state files that didn't have a UUID get an empty string when round-tripped. The `intent` field should use the stricter `skip_serializing_if = "Option::is_none"` pattern (like `parent_workflow`) to avoid introducing `null` into clean old state files.

---

## Open Questions

1. **Should `intent` be a separate header field or encoded as a variable?** Templates already declare `variables`, and agents pass `--var` bindings. An `--intent` flag is cleaner for non-templated context (it's workflow-level, not template-variable-level), but some teams might prefer `--var INTENT="..."` to avoid a new CLI flag. The tradeoff: header field is always present and visible to the dashboard without template coupling; variable approach reuses existing machinery but mixes operational metadata with template inputs.

2. **Where should the dashboard render `intent`?** Options: (a) replace the raw session name in the Name column, (b) show as a subtitle below the session name, (c) show in the detail pane header. Option (a) risks truncation; option (b) doubles row height; option (c) requires entering detail mode to see it.

3. **Should `summary` be a reserved key in `EvidenceSubmitted.fields` or a top-level sibling?** Making it a top-level sibling (as proposed) is structurally clean and avoids changing how `fields` is validated or iterated. But it means the engine needs to extract it before passing `fields` to the dashboard. Making it a convention inside `fields` (e.g., `{"_summary": "..."}`) requires no struct change but muddles the schema.

4. **Should `Transitioned` gain a `note` field?** This would let template authors embed state-level narrative at compile time rather than at runtime. The template could set `"note": "Waiting for CI to pass"` on state entry, visible in the dashboard without agent action. This is different from agent-submitted context (it's static, not runtime-generated) but would improve readability for any workflow.

5. **How do Temporal and LangChain handle this?** Temporal activity names are human-readable workflow identifiers set at invocation time (similar to `--intent`). LangChain chains carry a `name` field and accept `metadata` dicts per run. Both separate the human label from the machine execution path — the same separation `--intent` + `EvidenceSubmitted.summary` would provide.

---

## Summary

The header already has an established additive-field pattern (`parent_workflow`, `template_source_dir`, `session_id`) that makes adding `intent: Option<String>` to `StateFileHeader` a low-risk, high-value change — it's the missing workflow-level label the dashboard list needs to be legible. The most surprising finding is that `DecisionRecorded` events (already designed for narrative) and `DirectedTransition.rationale` / `Rewound.rationale` (already on-wire) are completely invisible to the dashboard, meaning significant narrative richness is already being captured but not displayed. The biggest open question is whether `intent` belongs in the header or is better expressed as a template variable, since both have merit but different coupling tradeoffs.
