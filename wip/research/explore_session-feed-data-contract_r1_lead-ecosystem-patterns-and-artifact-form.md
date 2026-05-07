# Lead: Ecosystem Patterns and Artifact Form

## Findings

### Industry Patterns

#### CloudEvents

CloudEvents (cloudevents.io, CNCF Graduated 2024) defines its core contract as a markdown specification hosted on GitHub. The schema itself is backed by a JSON Schema file (`cloudevents.json`) that validates the four required context attributes — `id`, `source`, `specversion`, `type` — and five optional ones. The JSON Schema uses permissive defaults: `additionalProperties` is not set to false, meaning consumers must accept unknown fields without validation errors.

Versioning lives in a single `specversion` attribute on every event (currently `"1.0"`). Patch-level changes to the spec do not increment this value. The spec does not define behavior for receivers that encounter an unrecognized `specversion` — it only mandates that producers write `"1.0"`. Unknown extension attributes (the mechanism for custom fields) are described as carrying "no defined meaning in this specification." Intermediaries SHOULD forward optional attributes they don't recognize. Consumers are implicitly expected to ignore unknown attributes, though the spec does not state this as a MUST.

The artifact form — markdown spec with a companion JSON Schema at a stable URL — is the dominant pattern here. The markdown is the authoritative human contract; the JSON Schema enables tooling without making the spec machine-first.

#### OpenTelemetry (OTLP)

OpenTelemetry's log data model (status: Stable) is specified in markdown. Forward-compatibility rules live in the OTLP protocol specification, not the data model document. The explicit requirement: "OTLP/JSON receivers MUST ignore message fields with unknown names and MUST unmarshal the message as if the unknown field was not present in the payload." This is stated as a hard requirement, not a recommendation.

The versioning philosophy avoids explicit version fields in favor of capability-based discovery at the protocol level. For the log record data model itself, adding new attributes to semantic conventions is always allowed without a schema bump. Removing or renaming attributes requires schema files (transformation rules). The stability guarantee for a Stable signal: no breaking changes within the same major version. The OTel project uses a stability level indicator — Development / Stable / Deprecated — as a first-class signal in the spec document itself.

The takeaway is separating the data model contract (field names, types, semantics) from the transport compatibility rules. The data model is the consumer-facing contract; the transport handles version negotiation separately.

#### NDJSON

NDJSON (ndjson.org spec, v1.0.0) is deliberately minimal. Its scope is the line-delimiter transport format, not the schema of JSON objects within lines. It specifies: each line MUST be a valid JSON text per RFC 8259, followed by `\n` (0x0A). It says nothing about versioning, schema evolution, unknown fields, or consumer behavior. Those are explicitly out of scope. NDJSON is a framing spec, not a contract spec — the application layer must define everything schema-related.

This matters for koto: NDJSON gives the file format, but the session feed data contract must come entirely from koto's own documentation. There is no NDJSON convention to inherit for versioning or reader behavior.

#### Event Sourcing Community (Greg Young / EventSourcingDB / event-driven.io)

The event sourcing literature converges on a few patterns directly applicable to append-only JSONL:

**Weak schema / tolerant reader**: Using JSON as the serialization format already provides a weak schema. The recommended pattern is to map JSON fields to the expected struct by name, defaulting any field not found in the JSON, and ignoring any JSON field not needed by the consumer. Martin Fowler's Tolerant Reader pattern (2011) formalizes this: "only take the elements you need, ignore anything you don't." Most JSON libraries implement this by default (serde with `#[serde(default)]`).

**Unknown event types**: The community consensus is that consumers SHOULD skip unknown event types rather than failing. Greg Young's book (chapter on Weak Schema) describes a pattern where an `Unknown` catch-all variant is reserved so deserialization never fails. EventSourcingDB's guidance: "systems must continue to support old event versions" — phrased as backward compat, but the implication is that forward compat (old reader + new writer) is handled by the skip-unknown pattern. The event-driven.io versioning patterns article recommends a two-phase deployment for breaking changes: first deploy code that handles both old and new schemas, then remove the old path.

**Naming by type suffix for breaking changes**: When a field change is genuinely breaking (e.g., changing the type of a field, removing a required field), the dominant convention is to create a new event type name rather than bumping a schema version field. Example: `book_acquired_v2` instead of version-bumping `book_acquired`. This makes version history visible in the event stream itself.

**Adding optional fields is always safe**: Adding nullable or optional fields to an existing event type is universally treated as non-breaking. The tolerant reader pattern handles this automatically. The contract constraint is: never remove a field that consumers currently use; never change the type of an existing field; never make an optional field required.

#### Confluent Schema Registry / Kafka (for contrast)

Confluent's Schema Registry approach is the high end of the tooling spectrum: a centralized registry with monotonically versioned schemas, compatibility rules (BACKWARD / FORWARD / FULL), and automatic transformation rules for migration between incompatible versions. This tooling makes sense for high-volume, multi-producer Kafka topics where producers and consumers are decoupled and may be on different release cycles simultaneously.

For koto's use case this is overkill. koto is a single-writer system (one koto binary per session). The consumers are dashboard developers, not a fleet of independent services. A centralized registry and transformation engine introduces more infrastructure than the problem warrants.

---

### Artifact Form Options

#### Option A: Markdown spec in docs/

A markdown document (e.g., `docs/reference/session-feed.md`) that defines the header fields, event envelope, all 15+ event type payloads, versioning semantics, and reader guarantees in prose and tables.

**Enables**: Human readability. Directly editable alongside code changes. Fits koto's existing documentation pattern — all other contracts (CLI output contract, error codes, template format) are specified in markdown. Zero tooling overhead. Searchable in GitHub. Can be linked from release notes and changelogs.

**Costs**: Not machine-validated. A schema change in `types.rs` can silently diverge from the markdown without a CI check. No code generation for consumers. No automated validation that example JSON in the spec is actually valid.

**Koto's existing pattern**: Markdown is the primary artifact form for all contracts. `docs/reference/error-codes.md` specifies the CLI error contract as tables and JSON examples. `docs/designs/current/DESIGN-koto-cli-output-contract.md` specifies `koto next` output as prose with JSON examples. `docs/reference/cli-usage.md` covers command-line flags.

#### Option B: JSON Schema at a stable URL

A JSON Schema document that formally defines the header and each event payload, hosted at a predictable path (e.g., `docs/schema/session-feed-v1.json`) and potentially served from a CDN or GitHub Pages URL.

**Enables**: Machine-validated contracts. Consumers can run `ajv validate` against logs. Tooling (code generators, validators, stub generators) can consume the schema. The schema is the authoritative artifact; markdown is generated from it.

**Costs**: JSON Schema has limited expressibility for the kind of semantic guarantees koto needs to document (ordering guarantees, atomicity semantics, epoch boundary rules, what seq gaps mean). These are behavioral properties, not structural ones — JSON Schema cannot encode them. JSON Schema would need to be supplemented with prose anyway, making it a second artifact to maintain. For the untagged enum pattern koto uses (`#[serde(untagged)]` on `EventPayload`), JSON Schema's `oneOf` / `anyOf` handling is non-trivial and produces verbose schemas. The `type` field that drives dispatch would require `if/then/else` chains or `$ref` to discriminated union schemas — achievable but complex.

**Koto's context**: No existing JSON Schema in the project. No JSON Schema tooling in CI. Adding a JSON Schema for the session feed would be a standalone addition with no existing infrastructure to integrate with.

#### Option C: OpenAPI / AsyncAPI document

AsyncAPI is designed for event-driven APIs — channels, messages, message schemas, protocol bindings. It supports JSONL-style append streams via custom bindings.

**Enables**: Industry-standard document structure for async message contracts. Tools can generate documentation, mock servers, and client stubs from it. AsyncAPI v3 supports discriminated unions via `oneOf`.

**Costs**: AsyncAPI is primarily for networked async APIs with explicit channels and protocol bindings (Kafka, AMQP, MQTT, HTTP webhooks). koto's session feed is a local filesystem artifact, not a networked channel. Mapping a local JSONL file to AsyncAPI concepts is a category mismatch. Consumers (dashboard developers) won't get meaningful tooling from an AsyncAPI document since there's no server to connect to. AsyncAPI documents are YAML or JSON, not markdown — a different artifact type than all other koto docs. High learning curve for a small audience.

**Koto's context**: No AsyncAPI tooling in the project. Audience of 2-5 implementers. The session feed is read from local files or relayed; there's no channel subscription model.

#### Option D: Versioned changelog alongside types.rs

A `CHANGELOG.md` next to `src/engine/types.rs` that records each schema change, what was added, what the version is, and what readers can rely on.

**Enables**: Traceability of changes. Directly co-located with the code being documented. Simple to maintain — one entry per PR.

**Costs**: A changelog alone doesn't define the full schema. Consumers still need a full field reference to build against. A changelog is a supplement, not a primary contract artifact.

---

### Form that fits koto's context

koto uses markdown for all contracts. The project has no JSON Schema tooling in CI. The consumer audience is small (2-5 implementers building dashboards), so toolchain complexity should be minimized. Design docs use YAML frontmatter + prose + JSON examples as a well-established internal convention.

The behavioral aspects of the contract — ordering guarantees, epoch boundary rules, seq gap semantics, atomicity of appends, partial-write recovery — cannot be encoded in JSON Schema. Prose is required regardless. For a small, focused audience that will read the spec once to understand the format and then build against it, a well-structured markdown reference document with embedded JSON examples is more valuable than a JSON Schema that requires a validator to interpret.

That said, JSON Schema and the markdown spec are not mutually exclusive. The markdown spec can be primary; a companion JSON Schema can be added later if consumers request tooling support. Starting with JSON Schema first and generating markdown from it would be harder to reverse than the opposite direction.

---

## Implications

**Reader behavior rule (from multiple sources)**: The contract must state explicitly that readers MUST ignore unknown fields within known event types, and SHOULD skip unknown event types rather than failing. koto's current deserializer errors on unknown event types (hard error in the custom `Deserialize` impl). The contract should acknowledge this as a known limitation and specify the recommended client behavior: buffer and skip unknown types, or use a catch-all `Unknown` variant. This is the single most significant gap between koto's current behavior and industry best practice.

**Schema versioning rule (from CloudEvents, OTel, EventSourcingDB)**: The `schema_version` field is the right mechanism, but it needs a defined semantics: when does it increment, and what must readers do when they encounter a version they don't recognize? The current koto code writes `1` everywhere and reads it nowhere — the field exists but has no contract. The data contract is the opportunity to define this.

**Additive fields are already handled correctly**: koto's pattern of `#[serde(default, skip_serializing_if = "Option::is_none")]` for optional fields matches industry best practice exactly. The contract should formalize this as a reader guarantee: optional fields absent in older logs default to their zero/None value; readers must not fail on their absence.

**Artifact form**: A markdown reference document in `docs/reference/` is the right primary artifact. It fits the existing project convention, can encode behavioral guarantees that JSON Schema cannot, and serves the small consumer audience well. A companion JSON Schema is a nice-to-have for validation tooling but should not block the primary contract.

**Type names are the versioning mechanism for breaking changes**: The industry convention (event-driven.io, EventSourcingDB) is to create a new type name (e.g., `transitioned_v2`) rather than bumping a schema version field when a field change is genuinely breaking. This is consistent with koto's existing practice — new event types have been added (e.g., `context_added`, `child_completed`) without touching existing types. The contract should codify this: existing event type names are stable; breaking changes require a new type name.

---

## Surprises

**CloudEvents does not specify unknown-field handling**. This is notable: CloudEvents is the most widely adopted event format spec, yet it does not include a MUST/SHOULD statement for consumers encountering unknown attributes. The behavior is implied by the permissive JSON Schema (`additionalProperties` not restricted) but never stated as a requirement. OTLP is more explicit: "MUST ignore message fields with unknown names." The gap shows that even mature specs sometimes omit this critical reader guidance.

**NDJSON is strictly a framing spec, not a contract spec**. It specifies only the line-delimiter. Nothing about schema evolution, versioning, or consumer behavior. This means koto's session feed data contract cannot inherit any conventions from NDJSON — it must define everything from scratch. NDJSON's simplicity is both a feature and a gap.

**The tolerant reader pattern is universally recommended but not universally implemented**. koto's current custom `Deserialize` for `Event` hard-errors on unknown type strings — a common implementation choice that trades forward-compat for exhaustive matching. The industry recommendation is always to skip/catch-all unknown types. The contract document should explicitly call this out as a known trade-off and advise external consumers to build their own tolerant deserialization layer.

**JSON Schema's practical limit**: JSON Schema can validate field presence and types but cannot encode semantic guarantees (ordering, atomicity, epoch boundaries). CloudEvents acknowledged this implicitly by keeping its JSON Schema to structural validation only and documenting behavioral guarantees in the markdown spec. This pattern is directly applicable to koto.

---

## Open Questions

1. **Should koto add a catch-all `Unknown` event variant to its own deserializer, or leave that to consumer implementations?** The contract can document both behaviors, but koto's internal behavior sets the precedent. Changing the hard-error to a skip would be a code change, not just a contract change.

2. **Should `schema_version` increment when a new event type is added (which hard-errors in old koto binaries), or only for changes to the header or existing event envelopes?** The distinction matters for how consumers use the version field to gate compatibility.

3. **Is a companion JSON Schema worth producing alongside the markdown spec?** If any consumer has a need to validate session logs with an off-the-shelf JSON validator, the answer is yes. If consumers are building parsers in code, the markdown spec is sufficient. This is a question for the dashboard implementers.

4. **What stability level signal should the contract carry?** OTel uses Development / Stable / Deprecated per signal. CloudEvents graduated the entire 1.0 spec. Should koto's session feed contract declare itself "beta" (may add event types in minor releases, breaking changes require major) or immediately stable?

5. **How should the contract reference the Rust source types?** Option: add a note to each event type in the markdown spec linking to the corresponding `EventPayload` variant in `types.rs`. This ties human and machine representations together without requiring generated docs.

---

## Summary

Industry specs (CloudEvents, OTel, event sourcing literature) converge on three reader behavior rules: ignore unknown fields within known event types, skip or catch-all unknown event types, and treat additive-optional fields as non-breaking. koto already handles additive fields correctly but hard-errors on unknown event types — a divergence from best practice that the contract should explicitly address. For artifact form, a markdown reference document in `docs/reference/` fits koto's existing conventions and can encode behavioral guarantees (ordering, atomicity, epoch boundaries) that JSON Schema cannot; a companion JSON Schema is useful for tooling but is not the right primary artifact for a small audience building against a local JSONL format. The biggest open question is whether koto should change its internal deserializer to skip unknown event types, or leave that responsibility to consumers building their own parsers.
