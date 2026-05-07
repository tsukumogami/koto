# Lead: Versioning Strategy

## Findings

### schema_version: written everywhere, read nowhere

`schema_version` is a `u32` field on `StateFileHeader` (types.rs:12). Every place in the codebase that constructs a `StateFileHeader` hardcodes it to `1`:

- `src/engine/persistence.rs:441` (test helper)
- `src/discover.rs:125, 310`
- `src/session/local.rs:755, 1212, 1295, 1726, 1852`
- `src/session/cloud.rs:1011, 1099, 1136, 1176`
- `src/cli/batch.rs:4014, 4124`
- `src/cli/init_child.rs:468, 603`

A search for `.schema_version` (field reads) returns nothing. No code anywhere dispatches on `schema_version` at runtime. It is written on init and then ignored. The field exists in the serialized header of every log file, but koto itself treats it as dead data.

### The forward-compat story lives in the custom Deserialize, not in schema_version

`Event` has a hand-rolled `Deserialize` implementation (types.rs:431–618) that dispatches on the `type` field string. When it encounters a type it doesn't recognize, it returns a hard error:

```rust
other => {
    return Err(serde::de::Error::custom(format!(
        "unknown event type: {}",
        other
    )));
}
```

This is the critical forward-compat boundary. An old reader encountering a log written by a newer koto that emits a new event type will fail, not silently skip. There is no `Unknown` catch-all variant. The custom Deserialize was deliberately chosen over `#[serde(untagged)]` to get typed dispatch, but it sacrifices forward-compat on new event types.

For fields *within* known event types, the story is much friendlier. Optional/additive fields use `#[serde(default, skip_serializing_if = "Option::is_none")]` throughout (e.g., `spawn_entry`, `skip_if_matched`, `submitter_cwd`, `rationale`, `superseded_by`, `template_source_dir`). Old readers silently default missing optional fields. New readers silently omit the field when re-serializing pre-feature events.

So koto has two distinct compat layers:
1. **Additive fields on known event types**: fully forward- and backward-compatible via serde defaults.
2. **New event type strings**: hard parse failure for old readers.

### Versioning options assessed

**Option A: schema_version in header is the contract version (readers check it)**

The header's `schema_version` is visible at line 1 before any events are parsed. A reader could read it first and decide whether to proceed. The field is already there and already at value `1`. The gap is that nothing reads it today—it would require implementing the check on both sides. A version bump would mean: "this log may contain event types or header fields you don't know about." The per-type serde default pattern handles additive fields without a bump, so a bump here would be reserved for structural breaks: new mandatory header fields, removed event types, or changes to the event envelope itself (renaming `seq`, `type`, `payload`, `timestamp`). This option is low-overhead and well-positioned—the field already exists in every log at line 1, which is the ideal placement for a format-version signal.

**Option B: A separate, independent spec version document**

The contract version lives only in a spec document; the JSONL files carry no machine-readable version signal. Consumers check the spec version to know what they can expect, but they cannot validate it against the file. This is the lowest implementation cost option: no code change required. The cost is that a consumer reading an old log has no in-band signal that the log predates feature X. Debugging mismatches requires correlating log timestamps against spec version publish dates—fragile and manual. This option is workable at zero scale but becomes a gap as soon as external consumers exist.

**Option C: Per-event-type versioning (each event type has its own version)**

Each event JSON object would carry a version field, e.g., `{"seq":1,"type":"transitioned","version":1,"payload":{...}}`. This gives the finest-grained compatibility signal. A reader can handle `transitioned@v1` and `transitioned@v2` independently. The overhead is significant: 15 event types × independent version tracking = a complex compatibility matrix. Given that koto's additive-field pattern (`#[serde(default)]`) already handles intra-type evolution without any version signal, the per-type version would only add value when a breaking change occurs within a type. koto has shipped five additive field expansions without a single breaking intra-type change. The signal-to-noise ratio is low at koto's current scale.

**Option D: Semantic versioning on the contract spec itself, separate from koto releases**

The JSONL spec is versioned independently (e.g., `event-log-spec@1.2.0`) and the spec document tracks major/minor/patch. Log files could optionally carry a `spec_version` field in the header, or the spec version could be implied by the koto release that wrote the file. This is a common pattern for protocols (think OpenAPI spec versions). The challenge at koto's scale: a separate spec version requires coordinated release processes between the spec and the implementation. With one maintainer team and no external API consumers, this overhead is disproportionate. The value would increase sharply if koto published client SDKs that independently versioned against the spec.

### What a version bump means today

Because `schema_version` is never read, a bump from `1` to `2` would currently be inert—it would appear in new log files but nothing would act on it. To make it meaningful, koto would need to add a version check in `read_header` or `read_events`: reject (or warn) when `schema_version > SUPPORTED_VERSION`. Existing logs (all at version 1) would remain readable. New logs at version 2 would fail to open in old readers that implement the check.

## Implications

### The most viable path: activate schema_version for structural breaks only

Option A (schema_version as the contract version) is already structurally present. The implementation gap is small: add a `CURRENT_SCHEMA_VERSION` constant, enforce it in `append_header`, and add a version guard in `read_header` that rejects or warns on unknown versions. This adds one validation in `persistence.rs` and one constant in `types.rs`.

Additive changes—new optional fields, new optional header fields—continue to require no version bump, exactly as F1's five additive fields shipped without one. A version bump would be reserved for genuinely breaking changes:
- New event type added (old readers would hard-fail on unknown type strings today)
- An existing event type's required field removed or renamed
- The header envelope structure changed

The hard-fail on unknown event types (`unknown event type: {}`) is the most likely trigger for a real version bump. If koto ships a new event type, old readers that read the spec contract need a signal that the log may contain unknowns. Either the version bump or a fallback `Unknown` variant in the deserializer (or both) would address this.

### Per-event versioning (Option C) is over-engineered

koto's additive-field track record—five expansions, zero intra-type breaking changes—suggests per-event versioning would add overhead with no near-term payoff. If an intra-type break ever becomes necessary, a schema_version bump gates it cleanly.

### A separate spec document (Option B) complements, not replaces, in-band versioning

A human-readable contract specification is valuable for external consumers. But without an in-band machine-readable version in the log itself, there's no way to programmatically validate that a given log was written by a koto version that supports a particular spec. Option B works as documentation; it doesn't work as a compat check. The right answer is both: a spec document that tracks what schema_version N means, plus the in-band field that lets readers verify compatibility at parse time.

## Surprises

**schema_version is written in over 20 places but read in zero.** This is more widespread than expected given that a field serving no runtime function has so many construction sites. Every `StateFileHeader` literal in tests and production code includes `schema_version: 1`, which suggests it was intended to be meaningful from the start—it just hasn't been wired to any check yet. This makes activation low-risk: the field is already present in all existing logs.

**The unknown-event-type failure is a hard parse error, not a graceful skip.** `read_events` calls `serde_json::from_str::<Event>` for each line (persistence.rs:178). When the custom Deserializer hits an unknown type string, it returns `Err`. Since unknown types appear on non-final lines of the log (the log is append-only and can grow after the unknown event), the error falls into the non-final-line branch and returns a `StateFileCorrupted` error. This means any old reader loading a log that a newer koto appended a new event type to will see a corruption error, not a graceful degradation. This is the sharpest forward-compat gap in the current design.

**The truncated-final-line recovery (persistence.rs:192–199) is a narrow exception.** It handles the case where the writer crashed mid-write. It does not handle unknown event types on the final line—those still fail with a custom error from the Deserializer before the parse-error branch checks whether it's the last line.

## Open Questions

1. **Should new event types trigger a schema_version bump, or should the deserializer gain an `Unknown` catch-all variant?** A bump gates readers explicitly. A catch-all variant lets old readers silently skip new event types—useful for consumers that only care about a subset of events (e.g., a monitoring tool that only reads `transitioned` events). The two are not mutually exclusive.

2. **If schema_version is activated, what is the rejection policy?** Should a reader encountering `schema_version > SUPPORTED` return an error or a warning-with-best-effort parse? For koto's current internal-only consumers, hard rejection is safe. For future external consumers, a warning may be preferable.

3. **Should the spec document exist independently of the code?** The JSONL format is currently documented only in code comments and type doc strings. A standalone spec (even a short one) would let external consumers understand the format without reading Rust. This is a documentation question as much as a versioning one.

4. **Does session_id (defaulting to empty string for old files) set a precedent for how new header fields should be handled?** The `session_id` field uses `#[serde(default)]` with a default of empty string rather than `Option<String>`. This means old logs parse without a version bump, but readers cannot distinguish "session was created without a session_id" from "session_id was empty by design." Should future header additions prefer `Option` + `skip_serializing_if` instead?

5. **At what scale does Option D (independent spec versioning) become worth it?** The answer is likely "when koto publishes a client SDK or when an external system integrates against the log format." Neither has happened yet.

## Summary

`schema_version` is present in every log file at value `1` but is never read by any code, making it a dormant versioning hook rather than an active compatibility mechanism. Option A—activating `schema_version` as the in-band contract version, bumped only for structural breaks—is the lowest-cost path that fits koto's current architecture, since the field is already universally written and activation requires only a validation check in `read_header`. The biggest open question is how to handle the hard parse failure on unknown event types: should new event types trigger a version bump, should the deserializer gain a silent `Unknown` catch-all, or both?
