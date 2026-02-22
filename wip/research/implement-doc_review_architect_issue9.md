# Architect Review: Issue #9 (feat(cli): add remaining CLI subcommands)

## Summary

The CLI is structurally sound. It acts as a thin translation layer: parse flags, construct engine/controller, call one method, format output. Dependency direction is correct (cmd/ imports pkg/, never the reverse). The four-package layout matches the design exactly. The `controller.New` signature accepting `*template.Template` fits the design's Controller API specification.

Two findings. One is blocking.

---

## Finding 1: `cmdTransition` skips template hash verification

**Severity**: Blocking

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 173-208

**Description**: The design document specifies template hash verification on every `koto transition` call (DESIGN-koto-engine.md lines 256 and 579-589, "Transition Validation Sequence" step 1: "Template hash check"). The `cmdNext` path correctly creates a `controller.New(eng, tmpl)` which performs hash verification, but `cmdTransition` loads the template solely to extract the Machine and never checks the hash.

This means an agent can advance the workflow state after the template has been modified mid-execution. The design explicitly treats this as a blocking integrity guarantee with no override: "Template hash verification is a blocking failure -- there is no flag to bypass it."

**Code in question**:

```go
func cmdTransition(args []string) error {
    // ...
    tmpl, err := loadTemplateFromState(resolved)
    // ...
    eng, err := engine.Load(resolved, tmpl.Machine)
    // ...
    if err := eng.Transition(target); err != nil {  // no hash check before this
        return err
    }
    // ...
}
```

**Suggestion**: Add template hash verification before calling `eng.Transition()`. The simplest approach is to compare `eng.Snapshot().Workflow.TemplateHash` against `tmpl.Hash` directly (same logic as `controller.New`). Alternatively, create a Controller and discard it -- but that couples transition to the controller unnecessarily. A standalone hash check function (perhaps on the engine, or a helper in the CLI) is cleaner. The same gap exists in `cmdRewind` (lines 295-330), though the design's Rewind Implementation section doesn't explicitly list hash verification as a rewind step. Applying it uniformly to all mutation commands would match the spirit of "no override flag."

---

## Finding 2: `cmdValidate` inlines state file parsing instead of using `loadTemplateFromState`

**Severity**: Advisory

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 361-396

**Description**: `cmdValidate` reads the state file with `os.ReadFile` + `json.Unmarshal` into `engine.State` directly, then compares hashes inline. The helper `loadTemplateFromState` already exists for extracting the template path from a state file. The hash comparison logic also partially duplicates what `controller.New` does.

This doesn't compound because `validate` is a diagnostic command with no other callers. The inline approach is arguably more explicit for a validation-only command that intentionally skips engine loading.

**Suggestion**: Consider using `loadTemplateFromState` for the template path extraction, then comparing the stored hash from a minimal state file read. This keeps the validation logic visible while reducing the number of places that parse state files directly. Not urgent -- this is contained to one function.

---

## Architecture Fit Assessment

### Correct patterns observed

- **CLI as thin layer**: Each `cmd*` function follows the same shape: parse args, resolve state path, load template, load engine, call one method, print result. No business logic in the CLI.
- **Dependency direction**: `cmd/koto/` imports `pkg/engine`, `pkg/controller`, `pkg/template`, `pkg/discover`, and `internal/buildinfo`. No reverse imports.
- **Controller signature**: `controller.New(eng *engine.Engine, tmpl *template.Template)` matches the design's Controller API. The nil-template path for backward compatibility with pre-template tests is a clean extension.
- **State file discovery**: Auto-selection via `resolveStatePath` uses `discover.Find` rather than reimplementing the glob pattern.
- **Error formatting**: All errors flow through `printTransitionError` or `printError` to produce structured JSON, matching the design's machine-parseable output requirement.
- **Variable merging**: Template defaults overlaid with `--var` flags matches the design's "set at init time, immutable thereafter" contract.
- **Output format split**: Agent-facing commands (`next`, `transition`, `query`, `workflows`) use JSON. Human-facing commands (`status`, `cancel`) use plain text. This matches the design's dual-output specification.
