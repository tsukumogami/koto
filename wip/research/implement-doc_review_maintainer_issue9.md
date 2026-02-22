# Maintainer Review: Issue #9 -- feat(cli): add remaining CLI subcommands

**Reviewer**: maintainer-reviewer
**Scope**: cmd/koto/main.go, cmd/koto/main_test.go, pkg/controller/controller.go, pkg/controller/controller_test.go
**Question**: Can the next developer understand and modify this code with confidence?

---

## Summary

The change adds 7 new CLI subcommands (status, rewind, cancel, validate, workflows, query, next) to the koto binary, introduces state file auto-discovery via `resolveStatePath`, and reworks the controller to accept a `*template.Template` instead of a nil-only path. The test suite is solid at 23 tests with a full lifecycle scenario and multi-workflow scenario.

Overall, this is readable code. The command dispatch is clear, the helper functions (`resolveStatePath`, `loadTemplateFromState`, `parseFlags`) are well-scoped, and the tests cover the important paths. The findings below are specific traps for the next person.

---

## Blocking Findings

### B1. `cmdValidate` uses `os.Exit(1)` mid-function while all other commands return errors

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 388-396

`cmdValidate` breaks the contract established by every other `cmd*` function. All other commands return an `error` which `main()` handles uniformly (lines 51-58). But `cmdValidate` calls `os.Exit(1)` directly on hash mismatch (line 392) and prints its own message format instead of going through `printError`.

The next developer will see the consistent `err = cmdFoo(args); if err != nil { ... os.Exit(1) }` pattern in `main()`, assume all error exits go through that path, and add logging or cleanup there. The `cmdValidate` exit will bypass it silently.

**Suggestion**: Return a `*engine.TransitionError` with code `engine.ErrTemplateMismatch` (or a new validation-specific code) instead of calling `os.Exit(1)`. The mismatch path can return an error like all other commands, and `main()` will handle the exit. The "OK" message can go through `printJSON` for consistency with the structured output convention.

### B2. `cmdStatus` outputs unstructured text while all other commands output JSON

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 288-292

The project convention (from CLAUDE.md and the existing codebase) is "CLI outputs structured JSON to stdout for agent consumption." Every other command (`init`, `transition`, `next`, `query`, `rewind`, `workflows`) outputs JSON. `cmdStatus` outputs `fmt.Printf` with "Workflow:", "State:", "History:" labels.

The next developer (or agent consumer) will assume all koto commands return parseable JSON. Status won't parse.

**Suggestion**: Either make `cmdStatus` output JSON (consistent with everything else), or add a comment explaining this is the intentional human-readable command and document the exception. Given the "structured JSON to stdout" convention, JSON seems like the right default, perhaps with a `--human` flag for the formatted version.

### B3. `cmdValidate` reads and parses state file manually, duplicating `loadTemplateFromState`

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 372-395

`cmdValidate` manually reads the state file (line 373), unmarshals into `engine.State` (lines 377-379), then reads the template path from it (line 383). Meanwhile, `loadTemplateFromState` (lines 454-480) does exactly the same thing -- reads a state file, extracts the template path, and parses the template.

These are divergent twins. The `loadTemplateFromState` helper uses a minimal parse struct (only extracting `workflow.template_path`), while `cmdValidate` unmarshals into the full `engine.State`. If the state file schema changes (e.g., the `workflow` field moves), one parse path will break and the other won't, and the developer fixing it will only find one of them.

**Suggestion**: Have `cmdValidate` call `loadTemplateFromState` to get the template, then compare `tmpl.Hash` against a hash extracted from a minimal state read. Or better, extend `loadTemplateFromState` to also return the stored hash so `cmdValidate` can be: load template, compare hashes, done.

### B4. `isFlag` treats any string starting with `-` as a flag, including negative numbers

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 482-484

```go
func isFlag(s string) bool {
    return len(s) > 0 && s[0] == '-'
}
```

This means `--var OFFSET=-1` will fail because `parseFlags` sees `-1` as a flag (line 89: `if isFlag(next) { return error }`). The next developer adding numeric parameters will hit this and won't understand why their flag values are rejected.

Current usage doesn't pass negative numbers, but the flag parser is general-purpose and reused across all commands. A value like `KEY=-1` as a `--var` argument would work (since it's a single argument), but `--some-flag -1` would not.

**Suggestion**: At minimum, add a comment to `isFlag` documenting the limitation. Better: check for `--` prefix specifically (which is the actual convention used by all flags in this CLI).

---

## Advisory Findings

### A1. `parseFlags` has no `--` (end-of-flags) support

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 71-102

Standard CLI convention uses `--` to signal "everything after this is a positional argument." The `parseFlags` function doesn't support this. If a positional argument happens to start with `-` (unlikely today, but possible if workflow names or states ever contain dashes at the start), the parser will misinterpret it.

This is advisory because current commands use flags for everything that could conflict, and positional args (like the `transition` target) are state names that won't start with `-`.

**Suggestion**: Add `--` support in `parseFlags`, or document the limitation in the function's comment.

### A2. `TestCmdNext_ReturnsDirective` doesn't verify output content

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main_test.go`, lines 205-229

The test name says "ReturnsDirective" but lines 225-228 only check that `cmdNext` doesn't error. It doesn't verify the returned directive contains interpolated variables ("build feature") or has the correct action type. Lines 222-223 parse the template and immediately discard it (`_ = tmpl`), making it look like verification was planned but not implemented.

The lifecycle test (TestScenario23, line 503-506) also calls `cmdNext` without checking output. Since `cmdNext` writes to stdout and the test doesn't capture stdout, there's no way to verify the directive content at the CLI layer.

**Suggestion**: Either capture stdout (via test helper that redirects os.Stdout) and verify the JSON output, or rename the test to `TestCmdNext_NoError` and remove the dead template parse. The controller-level test (`TestNext_WithTemplate_InterpolatesVariables` in controller_test.go) does properly verify interpolation, so this is partly covered at a lower layer.

### A3. Default state directory `"wip"` is a relative path magic value

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main.go`, lines 121-122, 405-406, 426-427

The string `"wip"` appears in three places as a default state directory: `cmdInit` (line 122), `cmdWorkflows` (line 406), and `resolveStatePath` (line 427). This is a context-free magic value tied to the tsuku project's `wip/` convention.

If one of these is changed and the others aren't, commands will disagree about where to look for state files.

**Suggestion**: Extract `"wip"` to a package-level constant like `defaultStateDir`.

### A4. Test names for "requires" tests don't clarify the error behavior

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/cmd/koto/main_test.go`, lines 130, 140, 196, 322

Tests like `TestCmdInit_RequiresName` and `TestCmdRewind_RequiresTo` verify that an error is returned, but don't verify the error message content. A future change could accidentally change the error to something unhelpful (e.g., nil pointer dereference) and these tests would still pass.

This is advisory because the tests do prevent the most dangerous case (silent success when required args are missing).

### A5. Controller's `New()` comment says "When tmpl is nil... Next returns a generic directive stub" -- this implicit nil contract is easy to misuse

**File**: `/home/dangazineu/dev/workspace/tsuku/tsuku-7/public/koto/pkg/controller/controller.go`, lines 27-30

The Controller has two behavioral modes based on whether `tmpl` is nil. This is documented in the comment, but there's nothing in the type system enforcing it. A caller passing nil will get generic "Execute the X phase" stubs without any warning. The CLI's `cmdNext` always passes a non-nil template (line 231), so the nil path exists only for backward compatibility.

**Suggestion**: If nil templates are no longer the expected path (now that template parsing is fully implemented), consider making `tmpl` required (return error on nil). If the nil path is still needed, the comment is sufficient.

---

## What's Clear

- The command dispatch in `main()` (lines 17-59) is straightforward. Each command maps to exactly one function with a consistent signature.
- `resolveStatePath` (lines 421-449) has a good comment explaining its auto-selection behavior, and the error messages are specific ("multiple state files found... use --state to select one").
- `loadTemplateFromState` (lines 451-480) is well-named and its comment accurately describes what it does.
- The test file organization with section comments (`// --- cmdInit tests ---`) makes navigation easy.
- `TestScenario23_FullLifecycle` and `TestScenario24_MultiWorkflowAutoSelection` are excellent integration tests that exercise the real command flow end-to-end.
- The `parseFlags` function is simple enough that its behavior is obvious from reading the code.
- Controller signature change from `New(eng)` to `New(eng, tmpl)` is clean and the hash verification at construction time is the right place for it.
