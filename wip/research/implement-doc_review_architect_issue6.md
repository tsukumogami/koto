# Architect Review: Issue #6 -- Version Conflict Detection and TransitionError JSON Serialization

## Scope

Review of commit ec993ad on branch docs/koto-engine. Files changed:
- `pkg/engine/errors.go`
- `pkg/engine/engine.go`
- `pkg/engine/engine_test.go`
- `pkg/controller/controller.go`
- `pkg/controller/controller_test.go`
- `cmd/koto/main.go`

Reviewed against: `docs/designs/DESIGN-koto-engine.md`

## Architecture Alignment Summary

The implementation fits the designed architecture well. Package boundaries, dependency directions, and responsibility ownership all match the design document. Key structural decisions are correct:

- **Dependency direction**: `controller` imports `engine`, `engine` imports only stdlib, CLI imports both. No circular dependencies. Matches the design's stated import direction.
- **Engine owns version conflict detection**: `persist()` in `pkg/engine/engine.go:260-278` re-reads the disk version before writing. This is in the engine layer where all state mutation happens. Correct placement per the design.
- **Controller owns template hash verification**: `controller.New()` in `pkg/controller/controller.go:32-45` compares the template hash. The engine stores the hash but doesn't verify it. Correct separation per the design.
- **TransitionError is the single error type**: All engine failures return `*TransitionError` with structured codes. The six codes from the design doc are all defined as constants in `errors.go`. No parallel error types introduced.
- **State file schema matches design**: The `State`, `WorkflowMeta`, `HistoryEntry` structs in `types.go` match the design's JSON schema exactly. No extra fields, no missing fields.
- **Atomic write with symlink check**: Implementation in `engine.go:310-351` matches the design's pseudocode nearly line-for-line, including temp file cleanup on failure and symlink detection.
- **Error JSON shape matches design**: `TransitionError` uses `omitempty` on optional fields, so version_conflict errors serialize with just `code` and `message` while invalid_transition errors include `current_state`, `target_state`, and `valid_transitions`. This matches the design's example JSON.

## Findings

### 1. Controller.New signature diverges from design API

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go:32`

**Severity**: Advisory

**Description**: The design specifies `New(eng *engine.Engine, tmpl *template.Template) (*Controller, error)` but the implementation uses `New(eng *engine.Engine, templateHash string) (*Controller, error)`. The `templateHash string` parameter is a temporary stand-in because the template package (#7) doesn't exist yet. The empty-string-skips-verification behavior is documented in the godoc comment.

This is an expected interim state -- the template package is issue #7, and the CLI wiring to use it is issue #9. The controller's signature will change when #7 lands. Since this is pre-v1 with no external consumers, the future breaking change is acceptable. The empty-string bypass is explicitly documented and all call sites pass "" (the CLI in `cmdNext`). No risk of the bypass being copied as the permanent pattern.

**Suggestion**: No action needed now. When #7 lands, the signature should change to accept `*template.Template` as the design specifies. The empty-string bypass should be removed at that point -- the design says "no override flag exists" for template hash verification.

### 2. Version conflict check has a TOCTOU gap

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:260-278`

**Severity**: Advisory

**Description**: The `persist()` method reads the disk version (line 272), then writes (line 277). Between the read and the atomic rename inside `atomicWrite`, another process could write. This is a classic time-of-check-time-of-use gap.

However, the design explicitly acknowledges this trade-off in Decision 2: "In Phase 1, this is a diagnostic signal rather than a retry mechanism." The version check catches the common case (stale engine instance from a previous Load), not the race condition case. The design also says file locking is deferred to a later release. The implementation matches the design's stated trade-off.

**Suggestion**: No action needed. The TOCTOU gap is an accepted Phase 1 trade-off. The version check catches most real-world conflicts (loading a state file, doing work, then finding another process wrote in between). True concurrent-write protection requires file locking, which the design explicitly defers.

### 3. Version conflict check re-reads the entire file to extract one integer

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:282-307`

**Severity**: Advisory

**Description**: `checkVersionConflict` reads the full file and unmarshals into a struct to get the version integer. The state file is small (typically under 10KB), so this is not a performance concern. The implementation uses a minimal anonymous struct `struct { Version int }` to avoid unmarshaling the full state, which is a reasonable approach.

**Suggestion**: No action needed. The approach is proportionate to the file size.

### 4. CLI error output uses two different shapes

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go:246-261`

**Severity**: Advisory

**Description**: `printError` (line 246) wraps errors in `{"error": {"code": "...", "message": "..."}}`, and `printTransitionError` (line 256) wraps them in `{"error": <TransitionError>}`. Both produce `{"error": {...}}` at the top level, but `printError` always has exactly `code` and `message`, while `printTransitionError` may also have `current_state`, `target_state`, and `valid_transitions`.

This is consistent -- `printError` handles non-TransitionError cases (internal errors, unknown commands), while `printTransitionError` handles structured engine errors. The shapes are compatible: both are `{"error": {"code": ..., "message": ...}}` with optional extra fields. An agent parsing the output can handle both uniformly by looking at `error.code`.

**Suggestion**: No action needed. The two functions handle distinct error categories and produce compatible JSON shapes.

### 5. cmdStub uses "not_implemented" error code not in the design's code list

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go:207-211`

**Severity**: Advisory

**Description**: `cmdStub` returns a `TransitionError` with code `"not_implemented"`. This code isn't in the design's list of six error codes. The stub commands (rewind, cancel, query, status, validate, workflows) will be implemented in #5 and #9, at which point this code disappears. The `TransitionError` type is being used as a general structured error here, which slightly stretches its semantic scope (it's not really a "transition" error).

This is temporary scaffolding that will be replaced by real implementations. The code is confined to `cmdStub` with no other callers. Not worth blocking.

**Suggestion**: When #9 replaces the stubs, verify that `not_implemented` is removed entirely. If a general "CLI error" struct is needed, it should be separate from `TransitionError`, but that decision can wait for #9.

## Structural Assessment

The implementation correctly respects the four-package architecture (`engine`, `controller`, `template`, `discover`) with `engine` as the anchor package. No parallel patterns are introduced. The version conflict detection lives in `persist()` inside the engine (where all state mutation happens), and template hash verification lives in the controller constructor (where template awareness belongs). These match the design's stated ownership.

Test coverage is thorough: the engine tests cover all six error codes, version conflict detection on both Transition and Rewind paths, normal flow without false conflicts, and JSON serialization with and without optional fields. The controller tests cover hash match, mismatch, and empty-string bypass.

No blocking findings.
