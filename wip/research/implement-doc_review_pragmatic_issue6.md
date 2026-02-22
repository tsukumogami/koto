# Pragmatic Review: Issue #6 -- Version Conflict Detection and TransitionError JSON

Reviewer: pragmatic-reviewer
Commit: ec993ad on branch docs/koto-engine

## Summary

The implementation is clean and appropriately scoped. Six error codes, a single `TransitionError` type, version conflict detection via re-read-before-write, and template hash verification in Controller. No speculative generality, no unused abstractions. A few minor findings below.

## Findings

### 1. Impossible-case handling: `os.IsNotExist` in `checkVersionConflict` (Advisory)

**File:** `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine.go:285-288`

`checkVersionConflict` is only called when `expectedVersion > 0`, meaning a prior persist already wrote the file. The file-not-found branch can't fire under normal operation. However, this is a two-line guard and protects against external deletion between operations (plausible in a filesystem-backed workflow tool). Not worth removing.

**Severity:** Advisory
**Suggestion:** Acceptable defensive code given the filesystem context. No action needed.

### 2. Impossible-case handling: `ErrUnknownState` in `Controller.Next()` (Advisory)

**File:** `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go:55-61`

`Controller.Next()` checks if the current state exists in the machine. The engine validates this during `Load()` and `Init()`, and only sets `CurrentState` to validated targets. This check can't fire unless the caller mutates the `*Machine` pointer passed to the engine (since the engine stores the pointer directly without copying on intake).

**Severity:** Advisory
**Suggestion:** The real fix is to deep-copy the machine in `Init`/`Load` so the engine is fully encapsulated. The defensive check here papers over the shared-pointer issue. Acceptable for now since the copy-on-read (`Machine()` getter) already exists and no caller is likely to mutate the input machine.

### 3. Duplicate test: `TestTransitionError_JSON` vs `TestTransitionError_JSONShape` (Advisory)

**File:** `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/engine_test.go:275-300` and `:889-925`

`TestTransitionError_JSON` (line 275) tests a strict subset of what `TestTransitionError_JSONShape` (line 889) covers. Both marshal a `TransitionError` with `ErrInvalidTransition`, check the same fields. The first test adds nothing the second doesn't already cover.

**Severity:** Advisory
**Suggestion:** Remove `TestTransitionError_JSON` (lines 275-300). `TestTransitionError_JSONShape` is the complete version.

### 4. `ErrTemplateMismatch` defined in engine, triggered only by controller (Advisory)

**File:** `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/errors.go:16-19`

The `ErrTemplateMismatch` constant is defined in the `engine` package but only used by `controller.New()`. The comment acknowledges this ("Defined here but triggered by the Controller layer"). This is a reasonable design choice to keep all error codes in one place for machine-parseable output consistency. Not blocking since the intent is documented and the constant has a consumer.

**Severity:** Advisory
**Suggestion:** No action needed. The centralized error code catalog is a defensible choice.

## No Blocking Findings

The implementation is well-scoped to the issue requirements. No dead code that breaks contracts, no single-entry registries, no speculative parameters. The version conflict check is the right level of sophistication for a file-based state machine (re-read version, not a full CAS, appropriate for the single-writer-with-race-detection use case). The `TransitionError` type is lean with appropriate `omitempty` tags.
