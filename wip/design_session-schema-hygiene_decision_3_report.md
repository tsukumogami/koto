# Decision 3: schema_version bump policy

## Recommendation

Option A: No version bump (stay at 1)

## Confidence

High

## Rationale

No code in the codebase reads or dispatches on `schema_version` at runtime — it is
stored but never used as a branching signal. All prior additive fields followed the
`#[serde(default)]` pattern without a bump, and the four new fields fit the same
pattern. A version bump would impose branch complexity on every future reader without
delivering any enforcement the serde attributes don't already provide.

## Option Analysis

### Option A: No version bump

**Pros**

- Consistent with established precedent. Five previous additive fields
  (`parent_workflow`, `template_source_dir`, `spawn_entry`, `submitter_cwd`,
  `skip_if_matched`) all used `#[serde(default, skip_serializing_if = "Option::is_none")]`
  without bumping `schema_version`. Following the same pattern keeps the schema
  versioning semantics coherent: `schema_version = 1` means "the JSONL format
  described in the event-log design doc", not "which optional fields are present".
- No new reader branches. No existing code reads `schema_version` at runtime to
  decide how to parse a log. Grep across all `.rs` files confirms every occurrence
  of `schema_version` is either a write (`schema_version: 1`) or a test assertion
  (`Some(1)`). Zero dispatch sites exist. A bump would require adding and then
  maintaining branches that today have no reason to exist.
- Readers check field presence, not version, by design. The PRD's R1.5 already
  specifies that readers encountering a header without `session_id` must treat it as
  a pre-schema-hygiene session and must not fail. This is a field-presence contract,
  not a version contract. The same model applies to all four additions: readers
  tolerate absence via `Option::is_none` defaults.
- `session_id` is required-when-present but optional-in-practice. The PRD marks
  `session_id` as required on new sessions (every `koto init` after the change must
  generate one), but old sessions without it are valid. This is indistinguishable
  from the established pattern for `template_source_dir` and `spawn_entry`, both of
  which are semantically required in the contexts where they're written but absent
  in older logs.
- Single writer controls. koto is both the only writer and the primary reader of
  these logs. External consumers at this stage are downstream analytics tools, not
  a multi-team wire protocol. The barrier for external-consumer adoption is low
  enough that "check for field presence" is an acceptable reader contract.

**Cons**

- A reader cannot determine from `schema_version` alone whether `session_id` is
  guaranteed present. It must check the field. For automated pipelines that want to
  enforce "session_id must be present", this requires field-level validation code
  rather than a simple version check.
- The PRD's non-back-fillable framing ("these fields must ship before external
  consumers adopt the schema") argues that this batch of changes is a coherent
  schema milestone. A version bump would signal that milestone clearly to readers
  who want to reason about "which generation of sessions is this log from?".
- Future schema bumps become harder to motivate. If four significant additions don't
  justify a bump, it's unclear what the threshold is.

### Option B: Bump to schema_version 2

**Pros**

- Readers can use `schema_version >= 2` as a shorthand for "expect session_id,
  millisecond timestamps, context_added events, rationale". Single integer check
  instead of four optional-field checks.
- Signals the schema milestone. The PRD's framing is that these four fields represent
  a coherent "completeness" gate before external consumer adoption. A version
  bump documents that gate in the file itself.
- `session_id` becomes required-when-version-2 rather than always-optional, which
  matches its intended semantics (required for new sessions, absent for old ones).

**Cons**

- Introduces reader branch complexity for zero current readers. No code dispatches on
  `schema_version` today. A bump creates a branch that every future reader must handle
  even though the only practical reader behavior difference is "field may or may not be
  present" — which the serde defaults already communicate.
- Breaks consistency with prior additive changes. Five previous additions used the
  `#[serde(default)]` pattern without a bump. Bumping now implies those additions were
  handled differently, which is false, and suggests inconsistent versioning philosophy.
- All write sites hardcode `schema_version: 1`. Bumping requires updating
  approximately 22 write sites across `src/`, `tests/`, and ensuring they all stay
  in sync. The integration test at line 143 of `integration_test.rs` asserts
  `schema_version = 1`; it would need updating and the semantics are non-obvious
  (should the test check for 1 or 2 or either?).
- No version dispatch mechanism exists to take advantage of the bump. The
  `read_events` function in `persistence.rs` does not inspect `schema_version`. A
  bump without dispatch machinery provides the signal but not the enforcement.

### Option C: Bump with strict reader contract

**Pros**

- Strongest guarantee: readers of v2 can assert `session_id` is always present and
  treat its absence as malformed. This is the tightest possible contract.

**Cons**

- All the downsides of Option B, plus a new failure mode for legitimate logs. The
  PRD explicitly states "readers that encounter a header without `session_id` must
  not fail." Option C requires readers to fail on v2 logs without `session_id`. But
  any implementation bug, future refactor, or third-party writer could produce a v2
  log without `session_id`. These logs would be permanently unreadable under Option C
  even though the actual content is recoverable.
- Creates an asymmetric strictness rule (v1-with-session_id: ok; v2-without-session_id:
  fail) that adds no practical correctness benefit over Option A or B while making
  the reader contract harder to reason about.
- Contradicts the PRD's explicit backward-compatibility requirement (R1.5, R2.3, R4.5).

## Key Assumptions

- The codebase assessment is current: no code dispatches on `schema_version` at
  runtime. If a dispatch site exists that was missed, Option A would still be
  preferable unless the dispatch site actually needs version-based branching.
- koto has not yet acquired external consumers for its session logs. The PRD's framing
  ("before external consumers adopt the schema") is accurate. If adoption has already
  happened, the decision window is closed and whichever option is chosen should be
  implemented immediately.
- The four additions will ship as a single PR. If they ship incrementally, the
  version-bump question re-opens because partial field presence becomes possible.
- `session_id` is functionally required on new sessions but treated as absent on old
  ones. No attempt will be made to back-fill it into existing logs. This is the field-
  presence contract the PRD defines.

## Rejected Options

**Option B (Bump to schema_version 2)** was rejected because it adds reader-branch
complexity for zero existing dispatch sites, breaks consistency with five prior
additive changes that used `#[serde(default)]` without a bump, and requires
updating ~22 hardcoded write sites. The schema milestone the bump would signal is
real, but its value is informational rather than enforcement-producing. The existing
field-presence contracts deliver the same reader behavior with less structural change.

**Option C (Bump with strict reader contract)** was rejected because it introduces
a failure mode for legitimate logs, contradicts the PRD's explicit requirement that
readers must not fail on logs lacking the new fields, and inherits all of Option B's
downsides while adding new ones.
