# Lead: Session header structure

## Findings

`StateFileHeader` is defined in `src/engine/types.rs` (lines 8-42) with six fields:

| Field | Type | Required |
|-------|------|----------|
| `schema_version` | `u32` | Yes |
| `workflow` | `String` | Yes |
| `template_hash` | `String` | Yes |
| `created_at` | `String` | Yes |
| `parent_workflow` | `Option<String>` | No |
| `template_source_dir` | `Option<PathBuf>` | No |

No UUID field exists. The header is written once at workflow init via `append_header()` in `src/engine/persistence.rs`. However, `relocate()` in `src/session/local.rs` (lines 252-317) explicitly rewrites the header during session rename operations. No immutability is enforced in code.

Optional fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`, establishing the pattern for additive fields that older readers ignore.

## Implications

A `session_id` field (UUID v4 string) can be added as a required field on `StateFileHeader`. The PRD must specify that `session_id` is copied unchanged during `relocate()` — it identifies the session independent of its name.

## Surprises

The header is not truly immutable: `relocate()` rewrites it. The PRD must define the UUID's immutability contract explicitly, since the code does not enforce it.

## Open Questions

None — enough detail to write the PRD specification.

## Summary

`StateFileHeader` has six fields with no UUID; optional fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`. The header is rewritten during `relocate()`, so the PRD must specify that `session_id` survives relocation unchanged. No codebase obstacles to adding `session_id` as a required field.
