<!-- decision:start id="session-feed-artifact-form-v2" status="confirmed" -->
### Decision: Primary artifact form for the session-feed data contract

**Context**

koto records workflow sessions as append-only JSONL files with 15 event types plus a
header. Three external consumers need to build parsers against a published contract
without access to koto's internal Rust types. The contract covers both structural
properties (field names, types, required vs optional) and behavioral guarantees
(ordering, atomicity, partial-write recovery, terminal state detection gap) — prose is
mandatory regardless of form chosen. The stated requirement is that structured
validation of real JSONL log files must be possible.

Research found: ~57 total fields across header + 15 event types; koto already ships
`split_frontmatter()` and `serde_yaml_ng` as production dependencies; the untagged
enum pattern is 2–3x simpler to express via a type-keyed YAML map than via JSON Schema
`oneOf`/`if`/`then` chains; YAML frontmatter + markdown body is already the established
koto idiom for every template file and design doc.

**Assumptions**

- Consumers are willing to use a koto-provided validator tool rather than off-the-shelf
  JSON Schema validators (ajv, jsonschema). If a consumer's CI pipeline already has JSON
  Schema validation and they want zero-effort integration, the two-file path becomes
  more attractive.
- A validator tool will be built as part of the implementation track alongside the spec.
  If this is not built, the machine-readable frontmatter provides no validation benefit
  (though it still serves as a structured, authoritative schema reference).

**Chosen: Combined YAML frontmatter + markdown body**

`docs/reference/session-feed.md` uses a YAML frontmatter block as the machine-readable
event schema and a markdown body for behavioral guarantees, reader requirements, JSON
examples, and tier classification. The frontmatter defines the header fields and all 15
event types with their fields, types, required/optional status, and tier assignment. The
markdown body covers everything JSON Schema cannot express. The file is the single
source of truth.

A companion validator CLI reads the frontmatter, iterates over a JSONL log file, and
validates each event against the schema for its type. It reuses koto's existing
`split_frontmatter()` function and `serde_yaml_ng` dependency. Estimated effort: 4–8
hours. The validator is part of the implementation scope, not deferred.

**Rationale**

Three factors favor the combined format over the two-file (markdown + JSON Schema) path:

1. **Single file eliminates drift.** The hardest maintenance problem with a two-file
   approach is keeping the JSON Schema in sync with the markdown spec across every PR
   that touches `types.rs`. Without a CI check, drift is probable. With a CI check,
   the overhead is continuous. A single combined file has no sync surface — the schema
   and the prose update together because they are the same file.

2. **YAML is simpler than JSON Schema for this type structure.** koto's `EventPayload`
   uses an untagged enum dispatched on a `"type"` string. Representing this in JSON
   Schema requires a `oneOf` with 15 `if`/`then` branches, one per event type. In a
   YAML map keyed by event type string, each event type is simply a named block — the
   same structure the deserializer and the validator both naturally use. The YAML is
   approximately 2–3x more concise and directly mirrors how koto reads the events.

3. **This is already koto's idiom.** Every koto template file uses YAML frontmatter
   (name, version, initial_state, states, gates, transitions) plus a markdown body for
   phase descriptions. Every design doc uses YAML frontmatter for status, decision, and
   rationale. The session-feed spec extends this established pattern to field-level
   schema rather than introducing a new artifact type.

**Alternatives Considered**

- **Markdown only**: Meets koto's documentation convention and covers all behavioral
  content. Rejected because it provides no path to structured validation — the stated
  requirement.
- **Markdown + separate JSON Schema**: Off-the-shelf validators (ajv, jsonschema) work
  directly, which is the main advantage. Rejected because: two files create a sync
  surface that requires CI enforcement to manage; JSON Schema's `oneOf` representation
  of the untagged enum is significantly more complex than the YAML alternative; no JSON
  Schema tooling exists in koto CI today so the off-the-shelf benefit requires net-new
  infrastructure on the consumer side anyway.

**Consequences**

- `docs/reference/session-feed.md` is the single source of truth: one file to update
  when `types.rs` changes, one file for consumers to read.
- A validator CLI must be built as part of the implementation (4–8 hours, reusing
  existing koto YAML parsing infrastructure). It is part of this work, not future work.
- Consumers using standard JSON Schema tooling (ajv) cannot validate directly against
  the spec. The tradeoff is custom tooling for a single, authoritative file.
- The frontmatter at ~120–180 lines is at the upper bound of what feels natural for
  frontmatter. If the event type count grows substantially (beyond ~25 types), the
  frontmatter should be split into a companion `session-feed-schema.yaml` file and
  the combined format revisited.
<!-- decision:end -->
