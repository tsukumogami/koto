# Review: Pragmatic -- Issue #4 (Walking Skeleton)

## Findings

### 1. MarshalJSON on TransitionError is dead code (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/engine/errors.go:21-24`

```go
func (e *TransitionError) MarshalJSON() ([]byte, error) {
	type alias TransitionError
	return json.Marshal((*alias)(e))
}
```

This method is a no-op. The `type alias TransitionError` trick is used to avoid infinite recursion when a struct's `MarshalJSON` needs to call the default marshaler with extra logic (adding/removing fields, wrapping in an envelope). But this method does nothing beyond what `json.Marshal` would do by default for a struct with JSON tags. It produces identical output to removing the method entirely.

Not blocking because it's inert -- it doesn't change behavior. But it's dead weight that could confuse a future reader into thinking there's custom serialization logic.

**Fix**: Remove the `MarshalJSON` method.

### 2. Controller.New signature diverges from design doc (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go:27`

Design doc specifies:
```go
func New(eng *engine.Engine, tmpl *template.Template) (*Controller, error)
```

Implementation:
```go
func New(eng *engine.Engine) *Controller
```

The comment says "template hash verification is skipped, added in issue #6." This is fine for a walking skeleton -- the template package doesn't exist yet, and forcing a nil template parameter would be speculative. The signature will change in #7/#6 when the template is wired in.

Not blocking. The comment tracks the deviation. Just noting the gap for the record.

### 3. CLI flag parsing silently ignores missing values (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go:53-71`

```go
case "--name":
    if i+1 < len(args) {
        i++
        name = args[i]
    }
```

If a user passes `koto init --name` with no value, the flag is silently ignored and the later `if name == ""` check produces a generic "required" error. This isn't wrong, but `koto init --name --template foo.md` would silently swallow `--template` as the value of `--name`. This is the standard failure mode of hand-rolled flag parsing.

Not blocking because this is a walking skeleton and issue #9 will likely replace this with proper flag parsing. But worth noting: the silent swallow of `--template` as a name value is a correctness issue, not just a UX one.

### 4. `internal/buildinfo/` is scope creep from issue #4 (Advisory)

**Files**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/internal/buildinfo/version.go`, `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/internal/buildinfo/version_test.go`

Issue #4 says: "core types, Init/Load/Transition methods, atomic persistence, and a minimal CLI with init/transition/next subcommands." A `version` subcommand with goreleaser ldflags integration, VCS revision extraction, and dirty flag handling is beyond the walking skeleton scope. The 87-line version_test.go file tests edge cases of a feature not in the issue's requirements.

Not blocking because the code is self-contained in its own package and doesn't affect the core engine. It's harmless scope creep, but it's scope creep.

### 5. `lint_test.go` is scope creep from issue #4 (Advisory)

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/lint_test.go`

Running golangci-lint, gofmt, go mod tidy, go vet, and govulncheck as test cases is CI infrastructure, not walking skeleton functionality. If there's a `.github/workflows/validate.yml` already, these may be redundant with CI. If CI doesn't exist yet, they're a reasonable bootstrap choice.

Not blocking. Standard Go project scaffolding.

## Overall Assessment

Clean walking skeleton. The engine, controller, and CLI all do what the issue requires: Init, Load, Transition, and Next work correctly with a hardcoded machine. Atomic writes follow the design doc's pseudocode closely. Types match the design doc's JSON schema. Error handling is correct with proper structured errors.

No blocking findings. The code is the simplest correct approach for a walking skeleton with two small advisory items on dead code (MarshalJSON) and scope additions (buildinfo, lint tests) that don't impact the core deliverable.
