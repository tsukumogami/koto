# Go State Machine Research for koto Engine

Research for [#311: design state machine engine](https://github.com/tsukumogami/vision/issues/311).

Date: 2026-02-21

## 1. Go State Machine Libraries

### 1.1 qmuntal/stateless

**Repository**: https://github.com/qmuntal/stateless
**Stars**: ~1,200 | **License**: BSD-2-Clause | **Latest**: v1.8.0 (Feb 2026)
**API style**: Fluent builder DSL (port of .NET's dotnet-state-machine/stateless)

**State machine configuration**:
```go
phoneCall := stateless.NewStateMachine(stateOffHook)
phoneCall.Configure(stateOffHook).
    Permit(triggerCallDialed, stateRinging)
phoneCall.Configure(stateRinging).
    OnEntryFrom(triggerCallDialed, func(_ context.Context, args ...any) error {
        onDialed(args[0].(string))
        return nil
    }).
    Permit(triggerCallConnected, stateConnected)
```

**Guard clauses**:
```go
phoneCall.Configure(stateOffHook).
    Permit(triggerCallDialled, stateRinging, func(_ context.Context, _ ...any) bool {
        return IsValidNumber()
    }).
    Permit(triggerCallDialled, stateBeeping, func(_ context.Context, _ ...any) bool {
        return !IsValidNumber()
    })
```

Guards must be side-effect free and mutually exclusive within a state. They're evaluated every time a trigger fires.

**External state storage** (critical for koto):
```go
machine := stateless.NewStateMachineWithExternalStorage(
    func(_ context.Context) (stateless.State, error) {
        return myState.Value, nil
    },
    func(_ context.Context, state stateless.State) error {
        myState.Value = state
        return nil
    },
    stateless.FiringQueued,
)
```

This decouples the state machine from in-memory state, letting the caller persist state however they want (JSON file, database, etc.). The two callbacks -- stateAccessor and stateMutator -- are the full persistence interface.

**Thread safety**: Yes, documented as thread-safe.
**Hierarchical states**: SubstateOf() support.
**Introspection**: PermittedTriggers(), DOT graph export.
**Parameterized triggers**: SetTriggerParameters() with type-checked args.

**Assessment for koto**: Strong candidate as a library dependency. The external storage pattern maps directly to koto's JSON state file. Guards map to evidence gates. The fluent API works well for building state machines from parsed YAML templates at runtime. Thread safety handles concurrent `koto next` / `koto transition` calls. The main gap is that guards are Go functions, not declarative data -- koto would need to build guard functions from the YAML evidence gate declarations, which is straightforward.

### 1.2 looplab/fsm

**Repository**: https://github.com/looplab/fsm
**Stars**: ~3,300 | **License**: Apache-2.0 | **Latest**: v1.0.3 (May 2025)
**API style**: Struct-based declarative configuration

**State machine definition**:
```go
fsm := fsm.NewFSM(
    "closed",
    fsm.Events{
        {Name: "open", Src: []string{"closed"}, Dst: "open"},
        {Name: "close", Src: []string{"open"}, Dst: "closed"},
    },
    fsm.Callbacks{
        "enter_state": func(_ context.Context, e *fsm.Event) {
            // handler
        },
    },
)
```

**Event triggering**:
```go
err := fsm.Event(context.Background(), "open")
```

**Callbacks**: Lifecycle-based (enter_state, leave_state, before_event, after_event). Named by convention -- `enter_open`, `leave_closed`, etc.

**Guards/conditions**: Not first-class. Guards are implemented via `before_<event>` callbacks that cancel the event by calling `e.Cancel()`. This is imperative, not declarative.

**Thread safety**: Not explicitly documented. The API uses context.Context, suggesting awareness of concurrent usage, but no guarantee is stated.

**Persistence**: None built-in. State is in-memory only. Manual serialization via `fsm.Current()` and reinitializing from a saved state is possible but not supported by the API.

**Visualization**: Built-in Graphviz and Mermaid DOT output -- useful for debugging.

**Assessment for koto**: Simpler API, more stars, but weaker feature set for koto's needs. No external storage abstraction means koto would own all persistence logic. Guards via callback cancellation are workable but less clean than stateless's guard functions. The declarative Events struct maps naturally to YAML parsing, which is a plus. However, the lack of hierarchical states and the imperative guard model make this a weaker fit.

### 1.3 cocoonspace/fsm

**Repository**: https://github.com/cocoonspace/fsm
**Stars**: Small (~50) | **License**: MIT | **Latest**: v1.0.1

**API style**: Functional options pattern with int-typed states and events.

```go
const (
    StateFoo fsm.State = iota
    StateBar
)
const (
    EventFoo fsm.Event = iota
    EventBar
)

f := fsm.New(StateFoo)
f.Transition(
    fsm.On(EventFoo), fsm.Src(StateFoo),
    fsm.Dst(StateBar),
)
```

**Guards**:
```go
fsm.Check(func() bool { return someCondition })
fsm.NotCheck(func() bool { return someCondition })
```

**Unique feature -- Times-based transitions**:
```go
f.Transition(
    fsm.On(EventFoo), fsm.Src(StateFoo), fsm.Times(2),
    fsm.Dst(StateBar),
)
```
Transition only fires after the event occurs N consecutive times. Interesting for retry/confirmation patterns but not directly useful for koto.

**Performance**: Reported as significantly faster than looplab/fsm (40ns/op vs 488ns/op). Not a concern for koto's use case (transitions are human-initiated, not high-frequency).

**Assessment for koto**: Clean API with first-class guards, but int-typed states don't map well to koto's string-based state names from YAML templates. Small community, no persistence support. Not recommended.

### 1.4 Gurpartap/statemachine-go

**Repository**: https://github.com/Gurpartap/statemachine-go
**Stars**: ~109 | **License**: Apache-2.0 | **Last active**: ~2017 (dormant)

**API style**: Fluent builder DSL with rich guard support.

```go
m.States("unmonitored", "running", "stopped")
m.InitialState("unmonitored")
e.Transition().From("stopped").To("starting")
```

**Guards**: `If()` and `Unless()` with multiple signatures:
- `*bool` pointers
- `func() bool`
- `func(transition statemachine.Transition) bool`

**Callbacks**: Before, Around, After transition callbacks.

**Assessment for koto**: Good API design with the best guard model, but the project is dormant (last activity 2017). Can't depend on unmaintained code for a new project. Worth studying the API design for inspiration.

### 1.5 astavonin/gfsm

**Repository**: https://github.com/astavonin/gfsm
**Stars**: Small | **License**: unspecified

**API style**: Builder pattern with StateAction interface (OnEnter, OnExit, Execute methods). Inspired by C++ FSM patterns, focused on speed.

**Assessment for koto**: Performance-oriented, targets embedded/microservice use cases. Overkill for koto's use case and the interface-heavy design doesn't map to declarative YAML templates.

### 1.6 Custom Implementation (no library)

Given koto's specific requirements, a custom implementation is worth considering:

**Arguments for custom**:
- koto's state machine is simple: named states, named transitions, evidence-gated guards. No hierarchical states needed initially. No parameterized triggers.
- The core logic is ~200 lines of Go: a map of states, each with allowed transitions and gate definitions. Transition validation is a lookup + gate check.
- External storage is the default (JSON state file), not an afterthought bolted onto an in-memory library.
- Evidence gates are declarative data (parsed from YAML), not Go functions. A library forces translating declarative gates into function closures, adding a layer of indirection.
- Template hash verification, atomic writes, and state file discovery are koto-specific concerns that no library addresses.

**Arguments against custom**:
- Reinventing tested edge cases (reentrant transitions, concurrent firing, event queuing).
- No graph visualization for free (looplab/fsm and stateless both offer this).
- More code to maintain.

### 1.7 Recommendation

**Primary recommendation: qmuntal/stateless** as a library dependency, with koto owning the persistence layer (JSON state files with atomic writes) and the declarative-to-functional bridge (converting YAML evidence gate specs into Go guard functions).

Stateless provides:
- External state storage via accessor/mutator callbacks -- maps directly to JSON state file reads/writes
- Guard functions -- koto builds these from YAML `evidence:` declarations at template load time
- Thread safety -- handles concurrent CLI invocations
- Context support -- enables timeout and cancellation
- Active maintenance (latest release Feb 2026)

Koto adds on top:
- YAML template parsing that produces stateless configuration
- Evidence gate types (field_not_empty, field_equals, command) as guard function factories
- Atomic JSON state file persistence
- Template hash verification
- State file discovery
- Transition history tracking (stateless doesn't maintain history)

**Alternative: custom implementation** if the stateless dependency feels heavy for what amounts to a lookup table + guard check. The engine is simple enough that the library's value is marginal -- most of koto's complexity lives in persistence, template parsing, and evidence evaluation, not in state transition logic itself. A custom implementation would be ~200-300 lines, fully tailored to koto's needs, with zero dependency overhead.

The decision hinges on whether koto will eventually need hierarchical states (Phase 3's multi-issue orchestration might), which would justify stateless. For Phase 1, custom is sufficient.

## 2. Atomic File Write Patterns in Go

### 2.1 The Core Pattern: Write-to-Temp-Then-Rename

The standard pattern for atomic file writes:

1. Create a temporary file in the same directory as the target
2. Write all content to the temporary file
3. Call `fsync()` on the file descriptor
4. Close the file
5. Rename the temporary file to the target path

The rename is atomic on POSIX systems -- readers either see the old file or the new file, never a partial write.

**Why same directory**: `os.Rename()` fails across filesystem boundaries. Creating the temp file in the target's directory guarantees they're on the same filesystem. Using `os.TempDir()` (or TMPDIR) risks cross-filesystem renames.

**Why fsync**: Without fsync, a power failure after rename but before the kernel flushes the write buffer can result in a zero-length file. The rename succeeded (directory entry updated) but the data wasn't written to disk. fsync forces the data to disk before rename.

**Cleanup on failure**: A deferred cleanup function must remove the temp file if any step fails. But the cleanup must be conditional -- after a successful rename, the temp path now points to the target file, so removing it would delete the result.

### 2.2 Libraries

**google/renameio** (673 stars, by Google):
- Handles temp file placement (same directory as target, respects TMPDIR for performance when possible)
- Calls fsync for durability on POSIX
- Applies umask correctly (v2 change)
- **Does not support Windows** -- doesn't export functions on Windows due to OS-level constraints
- Recommended by Michael Stapelberg (the author of the canonical "atomically writing files in Go" blog post, who noted his original code had incorrect fsync assumptions and recommends renameio instead)

**natefinch/atomic** (208 stars):
- `WriteFile(filename, io.Reader) error` -- writes reader contents atomically
- `ReplaceFile(source, dest) error` -- atomic file replacement
- **Windows support** via `moveFileEx` Windows API call
- Simpler API than renameio

**Go standard library (internal)**:
- `cmd/go/internal/lockedfile` -- Go's own toolchain uses an internal package for atomic file operations
- Not exported, but the pattern is well-tested. Available via `github.com/rogpeppe/go-internal/lockedfile` as a community mirror.

### 2.3 Platform Considerations

| Platform | os.Rename atomicity | Notes |
|----------|-------------------|-------|
| Linux | Atomic | POSIX guarantees |
| macOS | Atomic | POSIX guarantees |
| Windows | Not atomic by default | Requires `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING` flag |

Go's `os.Rename` on Windows doesn't use `MoveFileEx` -- it uses `MoveFile` which fails if the destination exists. Libraries like natefinch/atomic handle this correctly.

### 2.4 Recommendation for koto

**Use natefinch/atomic** for cross-platform atomic writes. koto is a CLI tool that should work on all platforms. renameio explicitly doesn't support Windows. natefinch/atomic handles Windows correctly via MoveFileEx and has a simpler API.

The write pattern for koto state files:

```go
import "github.com/natefinch/atomic"

func writeStateFile(path string, state *StateFile) error {
    data, err := json.MarshalIndent(state, "", "  ")
    if err != nil {
        return fmt.Errorf("marshal state: %w", err)
    }
    return atomic.WriteFile(path, bytes.NewReader(data))
}
```

If the dependency is undesirable, the pattern is simple enough to implement inline (~30 lines) using `os.CreateTemp` + `os.Rename` for POSIX, with a note that Windows support would need MoveFileEx via syscall.

## 3. JSON State File Patterns

### 3.1 Schema Versioning

koto's state file needs a version field to handle schema evolution. The design doc already includes `template_version` but should also include a `schema_version` for the state file format itself, independent of the template.

**Recommended pattern**: Embed a `schema_version` field at the top level of the JSON. The engine checks this on load and rejects files with unknown versions. Migration logic maps old schemas to new ones.

```json
{
  "schema_version": 1,
  "template": "quick-task",
  "template_version": "1.0",
  "template_hash": "sha256:a1b2c3...",
  "current_state": "initial_jury",
  ...
}
```

**Migration approach**: The engine maintains a list of migrators keyed by version. On load, if `schema_version < current`, run migrators sequentially. This is the pattern used by Terraform, Atlas, and Go's module system.

```go
type Migrator func(data json.RawMessage) (json.RawMessage, error)

var migrators = map[int]Migrator{
    1: migrateV1ToV2,
    2: migrateV2ToV3,
}
```

### 3.2 Concurrent Access

koto faces a specific concurrency scenario: the agent calls `koto transition` while the user runs `koto status` or `koto rewind` in another terminal. Two writers at the same time is the worst case.

**Options**:

1. **File locking (gofrs/flock)**:
   - `gofrs/flock` (707 stars, BSD-3-Clause, v0.13.0 Oct 2025) provides cross-platform file locking
   - TryLock() for non-blocking acquisition, Lock() for blocking
   - Works on Linux, macOS, Windows
   - Caveat: locking behavior isn't identical across platforms (some UNIX systems transparently convert shared locks to exclusive)
   - Pattern: acquire lock on `<statefile>.lock`, read state, modify, write atomically, release lock

   ```go
   fileLock := flock.New(stateFilePath + ".lock")
   locked, err := fileLock.TryLock()
   if !locked {
       return fmt.Errorf("state file is locked by another process")
   }
   defer fileLock.Unlock()
   // read, modify, write
   ```

2. **Optimistic locking (version counter)**:
   - Store a `version` field in the state file. On write, check that the version in the file matches what was read. If not, someone else wrote in between -- fail with a conflict error.
   - Simpler than file locking, no platform-specific behavior
   - Drawback: requires a read-modify-write cycle that can still race without atomic compare-and-swap at the filesystem level
   - Terraform uses this pattern for remote state backends (DynamoDB for S3, conditional PUT for GCS)

3. **Single-writer assumption**:
   - Document that only one process should write to a state file at a time
   - Atomic writes prevent corruption (partial reads)
   - Lost writes are possible but rare in practice (agent and human rarely write simultaneously)
   - Simplest option, appropriate for v0.1.0

**Recommendation for koto Phase 1**: Start with atomic writes + single-writer assumption. State file corruption from concurrent writes is prevented by atomic rename. Lost updates (two transitions racing) are detectable from the history log. Add file locking in Phase 2 when /work-on introduces more complex multi-process scenarios. The design doc already notes this: "detailed concurrency strategy (file locking or optimistic version counter) deferred to DESIGN-koto-engine.md."

### 3.3 Integrity Verification

The design doc specifies `template_hash` for detecting template tampering. For state file integrity, there are several patterns:

**Hash chain (append-only history)**:
Each history entry includes a hash of the previous entry, forming a chain. Tampering with any entry breaks the chain.

```json
{
  "history": [
    {
      "from": "",
      "to": "initial_jury",
      "timestamp": "2026-02-21T10:00:00Z",
      "evidence": {},
      "prev_hash": "",
      "hash": "sha256:abc123..."
    },
    {
      "from": "initial_jury",
      "to": "research",
      "timestamp": "2026-02-21T10:05:00Z",
      "evidence": {"jury_consensus": "3/3 agree"},
      "prev_hash": "sha256:abc123...",
      "hash": "sha256:def456..."
    }
  ]
}
```

This is the pattern used by Atlas migration directory integrity files (reverse merkle tree of migration hashes) and Go's module checksum database.

**Whole-file checksum**:
A `.state.sha256` sidecar file containing the hash of the state file. Simpler but doesn't pinpoint which part was tampered with.

**Recommendation for koto**: Hash chain on the history array. It's the right granularity -- you want to detect if someone edited a history entry to forge evidence or skip a state. The implementation is ~20 lines (hash each entry including the previous hash). Defer to Phase 2 if it adds too much to the initial scope; the template hash already covers the most important tampering vector (template TOCTOU).

## 4. Evidence/Gate Patterns

### 4.1 How Workflow Engines Handle Pre-conditions

The term "guard" in state machine theory refers to a boolean condition that must be true for a transition to fire. Guards must be side-effect free -- they check conditions but don't change state.

**Stateless (qmuntal)**: Guards are Go functions passed as additional arguments to `Permit()`. Multiple guards on the same trigger must be mutually exclusive (the library doesn't define priority).

**Temporal.io**: Uses "workflow signals" -- external events pushed into a running workflow. The workflow code uses standard Go conditionals to check signal values before proceeding. Guards are implicit in the procedural code, not declared separately.

**WorkflowEngine.io**: Transitions have explicit `Condition` objects with types: `Always`, `Otherwise`, `IsTrue(expression)`. Conditions are evaluated in order; the first matching condition determines the transition.

**Symfony Workflows**: Guards are event listeners on the `workflow.guard` event. They can block a transition by calling `$event->setBlocked(true, 'reason')`. This is the most flexible model -- any code can guard any transition.

### 4.2 koto's Evidence Gate Types

The design doc defines three gate types. Here's how each maps to implementation:

**field_not_empty**: Check that a key exists in the evidence map and has a non-empty string value.
```go
func fieldNotEmpty(evidence map[string]string, fieldName string) bool {
    val, ok := evidence[fieldName]
    return ok && val != ""
}
```

**field_equals**: Check that a field has a specific value.
```go
func fieldEquals(evidence map[string]string, fieldName, expected string) bool {
    return evidence[fieldName] == expected
}
```

**command**: Execute a shell command and check exit code.
```go
func commandGate(command string) bool {
    cmd := exec.Command("sh", "-c", command)
    return cmd.Run() == nil
}
```

Command gates have distinct security considerations documented in the design doc (they bypass the agent's permission system).

### 4.3 Gate Evaluation Patterns

**When gates are checked**: On `koto transition <target> --evidence key=value`. The engine:
1. Validates the transition is allowed from the current state
2. Merges new evidence with existing evidence in the state file
3. Evaluates all gates defined for the current state
4. If all pass: update state, write atomically, return success
5. If any fail: return error with the specific gate that failed and why

**Gate composition**: The YAML format implies AND composition -- all gates for a state must pass. OR composition could be added later by allowing a list of gate sets, but AND is sufficient for Phase 1.

**Partial evidence**: An agent might accumulate evidence across multiple calls before attempting a transition. The state file's `evidence` map persists across calls, so gates can reference evidence set in previous transitions. New evidence from `--evidence` flags is merged (not replaced) with existing evidence.

### 4.4 Translating Declarative Gates to Go Functions

For either stateless or a custom implementation, koto needs to convert YAML gate declarations into executable checks. The factory pattern works well:

```go
type Gate struct {
    Type    string // "field_not_empty", "field_equals", "command"
    Field   string // for field_not_empty and field_equals
    Value   string // for field_equals
    Command string // for command
}

func (g Gate) Check(evidence map[string]string) error {
    switch g.Type {
    case "field_not_empty":
        if evidence[g.Field] == "" {
            return fmt.Errorf("evidence %q is empty", g.Field)
        }
    case "field_equals":
        if evidence[g.Field] != g.Value {
            return fmt.Errorf("evidence %q is %q, expected %q", g.Field, evidence[g.Field], g.Value)
        }
    case "command":
        cmd := exec.Command("sh", "-c", g.Command)
        if err := cmd.Run(); err != nil {
            return fmt.Errorf("command gate failed: %s: %w", g.Command, err)
        }
    default:
        return fmt.Errorf("unknown gate type: %s", g.Type)
    }
    return nil
}
```

If using stateless, wrap this in a guard function:

```go
func (g Gate) AsGuard(evidence map[string]string) func(context.Context, ...any) bool {
    return func(_ context.Context, _ ...any) bool {
        return g.Check(evidence) == nil
    }
}
```

### 4.5 Future Gate Types

The design doc mentions the plugin API (Phase 3+) as the extension mechanism for custom gate types. For koto's own growth, likely additions include:

- **file_exists**: Check that a file path exists (useful for artifact-based evidence in /work-on)
- **regex_match**: Evidence value matches a pattern (e.g., PR URL format validation)
- **git_branch_exists**: Check that the working branch matches expectations
- **http_status**: Check an HTTP endpoint returns a specific status (e.g., CI status)

These would follow the same Gate struct pattern with additional Type cases.

## 5. Summary of Recommendations

| Decision | Recommendation | Rationale |
|----------|---------------|-----------|
| State machine library | qmuntal/stateless (or custom for Phase 1) | External storage, guards, thread safety, active maintenance |
| Atomic file writes | natefinch/atomic (or inline ~30 lines) | Cross-platform including Windows, simple API |
| Concurrency | Atomic writes + single-writer assumption (Phase 1), file locking via gofrs/flock (Phase 2+) | Matches complexity gradient |
| Schema versioning | Top-level `schema_version` integer with sequential migrators | Standard pattern, forward-compatible |
| State integrity | Template hash (Phase 1), history hash chain (Phase 2+) | Template TOCTOU is the immediate risk; forgery is lower priority |
| Evidence gates | Gate struct with Check() method, factory from YAML | Clean separation between declarative (YAML) and executable (Go) |

## Sources

- [qmuntal/stateless](https://github.com/qmuntal/stateless) -- Go FSM library with external storage and guards
- [looplab/fsm](https://github.com/looplab/fsm) -- Go FSM library with callback-based events
- [cocoonspace/fsm](https://pkg.go.dev/github.com/cocoonspace/fsm) -- Lightweight FSM with functional options
- [Gurpartap/statemachine-go](https://github.com/Gurpartap/statemachine-go) -- Declarative FSM with rich guards (dormant)
- [GFSM](https://sysdev.me/2024/11/25/gfsm-a-simple-and-fast-finite-state-machine-for-go/) -- Performance-oriented FSM
- [natefinch/atomic](https://github.com/natefinch/atomic) -- Cross-platform atomic file writes
- [google/renameio](https://github.com/google/renameio) -- Atomic file writes (POSIX only)
- [Michael Stapelberg: Atomically writing files in Go](https://michael.stapelberg.ch/posts/2017-01-28-golang_atomically_writing/) -- Canonical reference, recommends renameio
- [gofrs/flock](https://github.com/gofrs/flock) -- Cross-platform file locking
- [Go internal lockedfile](https://pkg.go.dev/cmd/go/internal/lockedfile) -- Go toolchain's own atomic file pattern
- [Atlas Migration Directory Integrity](https://entgo.io/blog/2022/05/09/versioned-migrations-sum-file/) -- Hash chain for file integrity
- [Terraform State Locking](https://developer.hashicorp.com/terraform/language/state/locking) -- JSON state file locking patterns
- [Simple workflow engine in Go using Stateless](https://medium.com/@jhberges/simple-workflow-engine-in-go-using-stateless-9db4464b93ec)
- [WorkflowEngine.io: Conditions](https://workflowengine.io/documentation/scheme/conditions) -- Workflow guard condition patterns
