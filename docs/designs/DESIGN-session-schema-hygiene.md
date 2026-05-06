---
status: Proposed
upstream: docs/prds/PRD-session-schema-hygiene.md
problem: |
  koto's JSONL session event log is missing four fields — session UUID, sub-second
  timestamps, context_added events, and rationale on directed transitions — that
  cannot be back-filled once external consumers begin reading sessions. The changes
  touch four separate struct definitions and three CLI command paths, and one of
  them (context_added event emission) requires plumbing SessionBackend access into
  a CLI path that currently has none.
decision: |
  Generate UUID v4 inline (no new crate, using /dev/urandom, mirroring the
  now_iso8601() pattern). Pass &dyn SessionBackend into handle_add alongside
  the existing ContextStore parameter, then emit context_added synchronously
  after store.add() returns. Add --rationale as an Option<String> flag on
  koto next --to and koto rewind, serialized with skip_serializing_if. Keep
  schema_version at 1; all four additions fit the established #[serde(default)]
  additive-field pattern with no runtime dispatch to motivate a bump.
rationale: |
  Every decision favors the minimal, pattern-consistent option. Inline UUID
  matches the now_iso8601() precedent and adds zero dependencies. Passing
  SessionBackend explicitly to handle_add mirrors handle_next exactly and
  preserves trait-object boundaries. The --rationale flag follows the existing
  koto overrides record surface. No schema_version bump is warranted because no
  code dispatches on that field and five prior additive changes set the same
  precedent. All four additions are backward-compatible via #[serde(default)].
---

# DESIGN: Session Schema Hygiene

## Status

Proposed

## Context and Problem Statement

koto records AI agent workflow sessions as JSONL event logs. Each line is an immutable
event appended at the moment it occurs. Four fields are absent from the current schema
that cannot be back-filled once external consumers adopt the log format: a session UUID,
sub-second timestamp precision, a `context_added` event, and rationale on directed
transitions and rewinds.

The implementation touches four code areas:

**Session identifier plumbing.** `StateFileHeader` is written once at `koto init` and
rewritten during `relocate()`. UUID v4 generation must produce a cryptographically random
value and copy it unchanged through `relocate()`.

**Timestamp precision.** `now_iso8601()` is a single pure function called from around a
dozen sites. Changing it to emit millisecond precision is low-risk in isolation; the risk
is breaking any consumer that hardcodes the 20-character whole-second string length or
parses the fixed field boundaries.

**Context event emission path.** `koto context add` currently writes to `ContextStore`
without touching the session's JSONL log. Emitting a `context_added` event requires
`SessionBackend` access in a CLI path that currently has none — `handle_add` receives
only `&dyn ContextStore` today, and the `run()` dispatch site does not forward the
`Backend` value into the context handler.

**Schema versioning.** `StateFileHeader.schema_version` is currently `1`. The design
must decide whether four additive fields warrant a bump.

## Decision Drivers

- **Backward compatibility is non-negotiable.** Existing JSONL logs must parse without
  failure after the change. No `deny_unknown_fields` is in use; additive fields with
  `#[serde(default)]` are the established pattern.
- **Minimal new dependencies.** `now_iso8601()` was written without `chrono` to keep
  the binary lean. The same principle applies — external crates for trivial operations
  should be avoided where the inline implementation is not materially more complex.
- **Single PR delivery.** All four additions must ship together. Staged delivery is not
  an option; external readers will see all fields or none.
- **Ordering guarantee is strict.** The PRD's R3.4 ordering guarantee (`context_added`
  seq < subsequent `koto next` seq) must be mechanically enforced, not advisory.
- **Existing test infrastructure.** Changes extend the existing integration test suite
  patterns; no new test harnesses.

## Considered Options

### Decision 1: UUID v4 generation

**Option A — `uuid` crate**
Add `uuid = { version = "1", features = ["v4"] }`. Single call-site expression:
`uuid::Uuid::new_v4().to_string()`. RFC 4122 compliance is tested upstream.

Ruled out: `uuid` is absent from `Cargo.lock`; adding it contradicts the pattern
established by `now_iso8601()`. One constructor and one formatter is a poor
crate-to-value ratio when the inline implementation is the same length.

**Option B — Inline UUID v4 (chosen)**
Open `/dev/urandom` via `std::fs::File::open`, read 16 bytes, set version nibble
(`byte 6 & 0xF0 = 0x40`) and variant bits (`byte 8 & 0xC0 = 0x80`), format with
`format!`. About 15 lines, mirroring `now_iso8601()` in complexity. Zero new
dependencies. Unix-only by design; koto's `[target.'cfg(unix)'.dependencies]` block
confirms this is the accepted target constraint.

**Option C — `getrandom` crate**
`getrandom` is already in `Cargo.lock` as a transitive dependency of `sha2` and
`tempfile`. Promoting it to a direct dependency adds RFC 4122 bit manipulation work
without eliminating any of Option B's manual steps, while pinning a concern that
currently belongs to its parent crates.

Ruled out: worst of both worlds — new direct dependency, same manual work, no
meaningful advantage.

### Decision 2: context_added event emission architecture

**Option A — Pass `SessionBackend` into `handle_add` (chosen)**
The `run()` call site already holds a `Backend` implementing both traits. Adding
`backend: &dyn SessionBackend` as a second parameter to `handle_add`, alongside the
existing `store: &dyn ContextStore`, requires one new parameter declaration and one
new argument at the single dispatch site. Emit `context_added` inside `handle_add`
after `store.add()` returns. `persistence::append_event` assigns `seq = last_seq + 1`
by reading the file at call time; emitting the event before `handle_add` returns
guarantees a lower seq than any future `koto next` invocation. This exactly mirrors
`handle_next`, which already takes both traits as separate parameters.

**Option B — `LoggingContextStore` wrapper**
Wrap `ContextStore` in a new type that holds both a store and a backend, emitting the
event inside the wrapper's `add()`. Ruled out: introduces a new type for a concern
addressable with one parameter; the call site still supplies both concerns so
encapsulation is illusory; hidden coupling inside the wrapper is harder to audit.

**Option C — Trait consolidation**
Merge `SessionBackend` and `ContextStore` into a supertrait or pass the concrete
`Backend` directly. Ruled out: breaks the trait-object boundary that all CLI handlers
enforce; forecloses future backends that implement one trait but not the other.

**Option D — Two-phase reconciliation at `koto next` time**
Defer `context_added` event emission until the next `koto next` call reads the sidecar
manifest and reconstructs what was added. Ruled out: directly violates PRD R3.3
(synchronous emission) and R3.4 (ordering guarantee); events would be assigned seq
numbers at `koto next` time, inverting the required ordering.

### Decision 3: schema_version bump

**Option A — No bump (stay at 1, chosen)**
`schema_version` is never read or branched on at runtime — every occurrence is either
a write (`schema_version: 1`) or a test assertion. Five prior additive fields
(`parent_workflow`, `template_source_dir`, `spawn_entry`, `submitter_cwd`,
`skip_if_matched`) all shipped without a bump using `#[serde(default)]`. The four new
fields fit the same pattern. The PRD's field-presence contracts (R1.5, R2.3, R4.5)
already define reader behavior for absent fields without needing a version signal.

**Option B — Bump to schema_version 2**
Allows `schema_version >= 2` as a shorthand for "expect the four new fields." Ruled
out: introduces branch complexity across all readers for a field that no current reader
dispatches on; would require updating ~22 hardcoded write sites; inconsistent with
five prior additive changes.

**Option C — Bump with strict reader contract**
Bump to 2 and add a reader failure path for unknown versions. Ruled out: introduces a
failure mode for legitimate old-format logs, contradicts PRD R1.5, and inherits all
Option B downsides.

### Decision 4: --rationale CLI surface

**Option A — Simple `--rationale <text>` flag (chosen)**
`koto overrides record` already accepts `--rationale <text>` as an `Option<String>`
clap argument. Adding the same flag to `koto next --to` and `koto rewind` follows the
existing surface exactly. The batch scheduler never calls either command (confirmed:
`batch.rs` never creates `DirectedTransition` events; `koto rewind` is manual-only).
Agent callers construct flag arguments programmatically; inline string values are the
natural form.

**Option B — stdin support**
Allow `--rationale -` to read from stdin. Ruled out: no koto command currently reads
from stdin; agent callers don't have a convenient stdin pipe; significantly more
implementation and test complexity for no practical benefit.

**Option C — `--rationale-file`**
A separate flag for file-based rationale. Ruled out: doubles the flag surface; if
file-based rationale is ever needed, the `@` prefix convention from `--with-data` can
extend `--rationale` without a second flag.

## Decision Outcome

Four decisions, all high confidence, all pattern-consistent:

1. **UUID v4 inline.** `generate_session_id()` opens `/dev/urandom`, reads 16 bytes,
   sets version and variant bits per RFC 4122, returns a lowercase hyphenated string.
   Zero new dependencies.

2. **`SessionBackend` parameter on `handle_add`.** `handle_add` gains a
   `backend: &dyn SessionBackend` parameter. After `store.add()` returns successfully,
   `backend.append_event()` emits a `context_added` event. If `append_event` fails, the
   error propagates to the caller (satisfying R3.5). `persistence::append_event`'s
   last-seq-plus-one strategy mechanically satisfies R3.4.

3. **No schema_version bump.** All four fields use `#[serde(default)]` with
   `skip_serializing_if = "Option::is_none"` where applicable. Readers tolerate absence
   via the established field-presence contract.

4. **`--rationale` as `Option<String>`.** Added to `Command::Next` and `Command::Rewind`
   clap variants. Threaded through `handle_next` and `handle_rewind` into
   `DirectedTransition` and `Rewound` event payloads. Serialized with
   `#[serde(default, skip_serializing_if = "Option::is_none")]`.

## Solution Architecture

### Struct changes

**`StateFileHeader`** (`src/engine/types.rs`):
```rust
pub struct StateFileHeader {
    pub schema_version: u32,       // unchanged; stays at 1
    pub session_id: String,        // NEW: UUID v4, generated at koto init
    pub workflow: String,
    pub template_hash: String,
    pub created_at: String,        // format changes to millisecond RFC 3339
    pub parent_workflow: Option<String>,
    pub template_source_dir: Option<PathBuf>,
}
```

`session_id` has no `#[serde(default)]` on write — new sessions always emit it.
Readers encountering a header without `session_id` receive an empty string via
`#[serde(default)]` deserialization, satisfying R1.5 without failing.

**`EventPayload`** (`src/engine/types.rs`) — three changes:

```rust
// Modified variants
DirectedTransition {
    from: String,
    to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rationale: Option<String>,    // NEW
},
Rewound {
    from: String,
    to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rationale: Option<String>,    // NEW
},
// New variant
ContextAdded {
    key: String,
    hash: String,
    size: u64,
},
```

**`Event`** — no struct change. The `timestamp: String` field format changes from
`YYYY-MM-DDTHH:MM:SSZ` to `YYYY-MM-DDTHH:MM:SS.mmmZ`. RFC 3339 parsers accept both.

### Function changes

**`now_iso8601()`** (`src/engine/types.rs`): Change from `as_secs()` to include
`subsec_millis()`:

```rust
// Before: "2026-05-06T14:30:00Z"
// After:  "2026-05-06T14:30:00.123Z"
format!("...{:02}.{:03}Z", sec % 60, duration.subsec_millis())
```

The fractional-second component uses `subsec_millis()` from `std::time::Duration`,
which is already available on the `SystemTime::now().duration_since(UNIX_EPOCH)` value.
No new types required.

**`generate_session_id()`** (new function, `src/engine/types.rs`):
```rust
fn generate_session_id() -> String {
    let mut buf = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    buf[6] = (buf[6] & 0x0F) | 0x40;  // version 4
    buf[8] = (buf[8] & 0x3F) | 0x80;  // variant bits
    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes(buf[0..4].try_into().unwrap()),
        u16::from_be_bytes(buf[4..6].try_into().unwrap()),
        u16::from_be_bytes(buf[6..8].try_into().unwrap()),
        u16::from_be_bytes(buf[8..10].try_into().unwrap()),
        /* 6 bytes -> u64 */ ...)
}
```

Called once in the `koto init` path where `StateFileHeader` is constructed.

### CLI changes

**`src/cli/mod.rs`** — two changes:

1. `Command::Next` gains `#[arg(long)] rationale: Option<String>`.
2. `Command::Rewind` gains `#[arg(long)] rationale: Option<String>`.
3. The `ContextCommand::Add` dispatch site forwards `&backend as &dyn SessionBackend`
   to `handle_add`.

**`src/cli/context.rs`** — `handle_add` gains a `backend: &dyn SessionBackend`
parameter. After `store.add(session, key, &content)` succeeds, it emits:

```rust
let hash = sha256_hex(&content);
let size = content.len() as u64;
backend.append_event(session, EventPayload::ContextAdded {
    key: key.to_string(),
    hash,
    size,
})?;
```

### relocate() invariant

`relocate()` in `src/session/local.rs` rewrites `StateFileHeader` when a session is
renamed. The rewrite must copy `session_id` unchanged from the existing header rather
than generating a new UUID. The implementation reads the existing header, preserves
`session_id`, and writes the updated header with the new workflow name. A test should
assert that the `session_id` value is identical before and after rename.

### Hash computation

`context_added.hash` is the SHA-256 hex digest of the artifact content. koto already
depends on `sha2` (transitively through `ring`); the `sha2` crate can be used directly,
or the existing `ring::digest` API can compute SHA-256. Either approach avoids a new
dependency.

## Implementation Approach

All four changes ship in a single PR. The natural implementation order is:

1. **Timestamp precision** — change `now_iso8601()` and update every test that
   asserts a specific timestamp format. This is mechanical; do it first to clear the
   test noise before other changes.

2. **Session UUID** — add `generate_session_id()`, add `session_id` to
   `StateFileHeader`, add `#[serde(default)]` on the deserialization side, update
   `koto init` path to call the generator, update `relocate()` to preserve the value,
   add unit test for UUID format and `relocate()` preservation.

3. **context_added event** — add `ContextAdded` variant to `EventPayload`, add
   `backend: &dyn SessionBackend` to `handle_add`, emit the event, propagate errors.
   Add integration test: `koto context add` followed by `koto next` and verify
   `context_added.seq < transition.seq`.

4. **--rationale flag** — add `rationale: Option<String>` to `DirectedTransition` and
   `Rewound`, add clap arguments, thread through handlers. Add integration tests for
   directed transition with and without rationale.

Each step builds on the previous (timestamp changes clear test noise; UUID doesn't
depend on context_added; rationale is independent of both). Steps 3 and 4 can be
done in either order.

## Security Considerations

**Cryptographic random source.** `/dev/urandom` is a CSPRNG on Linux, macOS, and all
BSDs. It is non-blocking and produces output indistinguishable from a true random
source for UUID purposes. The RFC 4122 collision risk for v4 UUIDs from a CSPRNG is
negligible (2^122 possible values).

**Hash integrity.** SHA-256 is used for `context_added.hash`. It is collision-resistant
for the purpose of detecting content changes between log reads. It is not a
cryptographic commitment scheme — no HMAC or signing is applied. This is appropriate
for an audit field; the log itself is not tamper-evident.

**Rationale field injection.** `rationale` is free text stored verbatim in the event
log. There is no injection risk within the JSONL format since the field is a JSON
string (serde handles escaping). Consumers that render rationale in UI contexts must
apply their own output escaping.

**Log append-only assumption.** The session JSONL log is append-only by convention,
not by enforcement. Malicious local access could modify or truncate it. This design
does not change that threat model. `session_id` improves trackability but does not add
tamper evidence.

## Consequences

### Positive

- Every new session carries a stable unique identifier that survives rename operations,
  enabling downstream consumers to correlate sessions reliably.
- Millisecond timestamps allow event ordering across concurrent child sessions that
  overlap within a one-second window.
- `context_added` events make the context state at any transition point reconstructible
  from the event log alone, without reading the mutable sidecar.
- Rationale on directed transitions and rewinds creates an auditable record of agent
  decision points.
- All additions are backward-compatible: existing JSONL logs parse without failure.

### Negative

- Old koto binaries cannot read JSONL logs that contain `context_added` events. The
  `EventPayload` untagged enum has no catch-all variant, so deserialization of an
  unknown variant fails. This is forward-compatibility breakage (old reader + new log),
  not backward-compatibility breakage, and is outside the PRD's compatibility scope.
- `handle_add`'s signature grows by one parameter. Any future call sites for
  `handle_add` must supply `&dyn SessionBackend`. Currently there is exactly one call
  site; adding a second requires the same plumbing.
- `relocate()` must copy `session_id` unchanged. There is no compile-time enforcement
  of this invariant — it relies on the implementation reading the existing header
  before writing a new one. A test covers this, but the constraint is behavioral.

### Mitigations

- The `ContextAdded` forward-compatibility break is mitigated by the single-PR delivery
  requirement: all four fields ship together. Consumers who upgrade get the full schema
  at once; there is no intermediate state where some fields exist and others don't.
- A unit test for `generate_session_id()` verifies the version nibble, variant bits,
  and hyphenated format, catching RFC 4122 implementation errors before they reach
  production logs.
- A `relocate()` integration test asserts `session_id` equality before and after rename.
