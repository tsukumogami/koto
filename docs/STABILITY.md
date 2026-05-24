# Stability Contract

This document pins the koto crate's public stability surface against
breaking changes. It is the operator- and integrator-facing artifact
referenced by `#[stable]` doc-comment blocks throughout the source.

The contract is the outcome of Decision 5 in
`docs/designs/DESIGN-koto-request-store.md` (lines 626-749). Bunki BK2
and other external substrates import `koto::engine::types::*` and the
four frozen `SessionBackend` methods listed below; this document is
the durable record of what they can rely on.

## Sections

1. [`CURRENT_SCHEMA_VERSION` bump protocol](#current_schema_version-bump-protocol)
2. [Frozen `SessionBackend` surface](#frozen-sessionbackend-surface)
3. [`StateFileHeader` additive evolution](#statefileheader-additive-evolution)
4. [`EventPayload` additive variants](#eventpayload-additive-variants)

---

## `CURRENT_SCHEMA_VERSION` bump protocol

The constant lives at `koto::engine::types::CURRENT_SCHEMA_VERSION`.
It encodes the maximum wire-format version koto knows how to read; a
state file whose `schema_version` exceeds the constant rejects with
`EngineError::IncompatibleSchemaVersion`. The constant's value is
bumped under one of three rules:

### Patch releases — `CURRENT_SCHEMA_VERSION` rises by **0**

The vast majority of patch-level changes do NOT alter the wire format:
bugfixes, performance improvements, internal refactors, dependency
bumps, and additive doc updates all leave the constant untouched.

### Minor releases — `CURRENT_SCHEMA_VERSION` rises by **0 or 1**

Minor releases are permitted to introduce additive evolution to the
wire format under the rules in
[`StateFileHeader` additive evolution](#statefileheader-additive-evolution)
and [`EventPayload` additive variants](#eventpayload-additive-variants).
When an additive change ships, the constant rises by 1; when no
wire-format change ships, the constant stays put. Additive changes
that ship in the same release MAY be batched under a single version
bump.

### Major releases — `CURRENT_SCHEMA_VERSION` rises by **1+**, with a 6-week deprecation window

A breaking change to the wire format (field removal, type-shape
change, rename, semantics shift) requires:

1. **6-week deprecation window** before the major release ships.
   During the window the breaking change is announced via release
   notes and the project changelog; bunki BK2 and other downstream
   consumers get warning of the upcoming break.
2. **Migration tool** ships in the same major release. The tool reads
   state files in the pre-break format and rewrites them in the
   post-break format. Tool is published as `koto migrate` or under a
   similar discoverable subcommand.
3. **`CURRENT_SCHEMA_VERSION` rises by at least 1** so post-break
   files are unambiguously identified.
4. **Pre-break reader retained** for at least one full release cycle
   after the major bump so operators who haven't migrated yet can
   still read their existing files. The retained reader emits a
   warn-level log naming the deprecation status.

Major bumps are explicitly costly under this protocol and are
expected to be rare — once or twice in the crate's lifetime.

---

## Frozen `SessionBackend` surface

Four methods on the `SessionBackend` trait are part of the **Stage 1
frozen surface**. Bunki BK2 imports these methods by name; signature
changes, removal, or rename require a 6-week deprecation window.

| Method | Why frozen |
|--------|-----------|
| `list` | Surfaces session metadata for the discovery scan and bunki's workspace inventory. |
| `read_events` | Single canonical read path for the JSONL event log. |
| `create` | Initializes the session directory; bunki spawns child sessions through this. |
| `init_state_file` | Atomic create-or-fail header + initial events; collision semantics are part of the contract. |

The remaining `SessionBackend` methods (`session_dir`, `exists`,
`cleanup`, `append_header`, `append_event`, `read_header`,
`ensure_pushed`, `relocate`, `lock_state_file`) carry an
**additive-only doc note**. Their signatures may evolve in minor
releases; downstream consumers should not depend on their exact
shape.

**Adding new methods** to the trait is permitted in minor releases.
The additive-only policy applies to the trait as a whole: introducing
a new method is a non-breaking change as long as existing
implementations don't need to provide it (i.e., the new method has a
default implementation or the trait gains the method as part of an
extension trait).

Each frozen method carries a `# Stability: Stage 1 — Frozen` doc
block in `src/session/mod.rs` so `cargo doc --no-deps` readers see
the lockdown status prominently.

---

## `StateFileHeader` additive evolution

The header at `koto::engine::types::StateFileHeader` evolves under a
strict additive-only policy:

### Rules

1. **New fields are `Option<T>` only.** A required field cannot be
   added in a minor release — pre-bump state files lack the field
   and deserialization would fail.
2. **Every new field carries `#[serde(default, skip_serializing_if = "Option::is_none")]`.**
   The `default` attribute lets the deserializer fill `None` when the
   field is absent on disk; the `skip_serializing_if` keeps the
   serialized form bytes-identical for callers that don't use the
   field.
3. **No field is ever renamed in a minor release.** A rename is a
   breaking change and requires the major-bump deprecation window.
4. **No field is ever removed in a minor release.** Field removal
   breaks downstream consumers that read it; removal goes through
   the deprecation window.

### Worked examples

Issue 1 added the request-store fields under these rules:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub parent_workflow: Option<String>,

#[serde(default, skip_serializing_if = "Option::is_none")]
pub requested_by: Option<String>,

#[serde(default, skip_serializing_if = "Option::is_none")]
pub assignment_claim: Option<AssignmentClaim>,
```

Pre-Issue-1 state files lack these fields; the `default` attribute
fills `None` on read and the `skip_serializing_if` keeps the
on-disk representation unchanged for callers that didn't set them.

Issue 16 added `respawn_generation` under the same discipline:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub respawn_generation: Option<u32>,
```

Both additions shipped in a minor release without bumping
`CURRENT_SCHEMA_VERSION`.

---

## `EventPayload` additive variants

The enum at `koto::engine::types::EventPayload` admits new variants
in minor releases under these rules:

### Rules

1. **New variants are additive only.** Adding a variant in a minor
   release is permitted; removing or renaming an existing variant is
   not.
2. **The `Unknown` catch-all is the forward-compatibility hook.** Any
   variant the reader doesn't recognize falls into `Unknown { type_name,
   raw_payload }`. The reader preserves the original `type` string and
   raw JSON payload so a future version with the variant defined can
   re-parse the same file losslessly.
3. **No variant ever changes its serialized name.** The `type` field
   on disk pins the variant; renaming the variant breaks all
   pre-rename writers and goes through the major-bump deprecation
   window.

### How the catch-all works

The `EventPayload` enum uses `#[serde(untagged)]` for the body, but
the outer `Event` struct's custom `Deserialize` impl reads the `type`
field FIRST and matches it against the variant names. Any `type` value
not in the match table falls into:

```rust
other => EventPayload::Unknown {
    type_name: other.to_string(),
    raw_payload: payload_val.clone(),
},
```

Round-trip is byte-identical: `Event::Serialize` writes back the
original `type_name` and `raw_payload`. A koto 1.5 reader can parse a
koto 1.6 event log without losing data — the new variants survive
in the `Unknown` form and a future upgrade re-decodes them losslessly.

---

## Future Stage 2 surface (informational)

The KT1 substrate-orchestration traits (`SubstrateSpawner`,
`SubstrateWaker`, `SubstrateRespawner`) are bunki BK2's swap-in
surface for dispatching agents, delivering wakes, and respawning
crashed coordinators. Their APIs are still in flux as Issues 15 and
16 land; **Stage 2 lockdown is deferred** to a future minor release.
External substrates may exercise these traits but should not depend
on their exact shape across minor releases until the Stage 2 lockdown
ships.

---

## Operator-facing references

- `docs/workspace-layout.md` — operator catalog of KT1 derived files
  (`_terminal_index.jsonl`, scan cursors, compaction lock, claim
  sidecars) and their safe-deletion semantics.
- `docs/designs/DESIGN-koto-request-store.md` — full KT1 design,
  Decision 5 (this contract's source of authority).
