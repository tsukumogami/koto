# Go Library Usage Guide

koto's engine is designed to be imported directly as a Go package. You can build workflow orchestration into your own tools without going through the CLI.

```bash
go get github.com/tsukumogami/koto
```

The main packages:

| Package | Import Path | Purpose |
|---------|-------------|---------|
| engine | `pkg/engine` | State machine, persistence, transitions |
| template | `pkg/template` | Template types, interpolation, compiled template parsing |
| compile | `pkg/template/compile` | Compile source templates into the intermediate format |
| controller | `pkg/controller` | Generate directives from engine state + template |
| discover | `pkg/discover` | Find active state files in a directory |

Most consumers only need `engine`. The other packages are useful if you want template compilation or directive generation.

## Building a Machine

A `Machine` defines the state machine: named states, allowed transitions, and which states are terminal. You can construct one directly or parse it from a template file.

### Programmatic construction

```go
package main

import "github.com/tsukumogami/koto/pkg/engine"

func buildMachine() *engine.Machine {
    return &engine.Machine{
        Name:         "deploy",
        InitialState: "validate",
        States: map[string]*engine.MachineState{
            "validate": {Transitions: []string{"build"}},
            "build":    {Transitions: []string{"test"}},
            "test":     {Transitions: []string{"deploy", "validate"}},
            "deploy":   {Transitions: []string{"done"}},
            "done":     {Terminal: true},
        },
    }
}
```

States with `Terminal: true` have no outgoing transitions. The workflow ends when it reaches one.

### From a template file

The compiler reads a source template (YAML front-matter + markdown body), validates it, and produces a `CompiledTemplate`. From there, `ToTemplate()` gives you the `Machine`, `Sections`, and `Variables` needed by the engine and controller.

```go
import (
    "os"

    "github.com/tsukumogami/koto/pkg/template"
    "github.com/tsukumogami/koto/pkg/template/compile"
)

sourceBytes, err := os.ReadFile("/path/to/workflow.md")
if err != nil {
    log.Fatal(err)
}

// Compile source into the intermediate format.
ct, warnings, err := compile.Compile(sourceBytes)
if err != nil {
    log.Fatal(err)
}
// warnings contains non-fatal issues found during compilation.

// Compute a deterministic hash of the compiled output.
hash, _, err := compile.Hash(ct)
if err != nil {
    log.Fatal(err)
}

// Convert to a Template for use with the engine and controller.
tmpl, err := ct.ToTemplate()
if err != nil {
    log.Fatal(err)
}
tmpl.Hash = hash
tmpl.Path = "/path/to/workflow.md"

// tmpl.Machine is ready for engine.Init or engine.Load
// tmpl.Sections maps state names to their directive text
// tmpl.Variables holds default values from variable declarations
```

> **Note:** The older `template.Parse()` function is deprecated. It reads a source file and returns a `Template` directly, but it skips the compiler pipeline and uses a legacy hash (SHA-256 of raw source bytes). New code should use `compile.Compile()` + `ct.ToTemplate()` as shown above.

## Creating a workflow

`engine.Init` creates a new state file and returns an Engine positioned at the machine's initial state.

```go
import "github.com/tsukumogami/koto/pkg/engine"

machine := buildMachine()

eng, err := engine.Init("wip/koto-deploy.state.json", machine, engine.InitMeta{
    Name:         "deploy",
    TemplateHash: "sha256:abc123...",  // from compile.Hash, or your own
    TemplatePath: "/path/to/template.md",
    Variables: map[string]string{
        "ENV":     "production",
        "VERSION": "2.1.0",
    },
})
if err != nil {
    log.Fatal(err)
}

fmt.Println(eng.CurrentState()) // "validate"
```

The state file is written atomically using write-to-temp-then-rename. A crash during init won't leave a partial file.

## Loading an existing workflow

`engine.Load` reads a state file from disk and validates it against the machine definition.

```go
eng, err := engine.Load("wip/koto-deploy.state.json", machine)
if err != nil {
    log.Fatal(err)
}

fmt.Println(eng.CurrentState()) // wherever it left off
```

If the state file's current state doesn't exist in the machine, Load returns an `unknown_state` error. This catches cases where the machine definition changed incompatibly.

## Transitioning

`Transition` validates and advances to a target state. It checks that the target is in the current state's allowed transitions, updates the in-memory state, and persists atomically.

```go
err := eng.Transition("build")
if err != nil {
    // Handle structured error (see below)
    log.Fatal(err)
}

fmt.Println(eng.CurrentState()) // "build"
```

If persistence fails, the in-memory state rolls back to its pre-transition value. The engine stays consistent even on disk errors.

## Rewind

`Rewind` resets to a previously visited state. The target must appear in the transition history or be the machine's initial state.

```go
err := eng.Rewind("validate")
if err != nil {
    log.Fatal(err)
}
```

Rules:
- You can rewind to any state that appears as a `To` field in the history.
- The initial state is always a valid rewind target, even if no transitions have happened.
- You can rewind from a terminal state (recovery path).
- You can't rewind to a terminal state (it would leave the workflow stuck).
- History is preserved, not truncated. A rewind entry with `Type: "rewind"` is appended.

## Cancel

`Cancel` deletes the state file.

```go
err := eng.Cancel()
if err != nil {
    log.Fatal(err)
}
```

After canceling, the engine instance is no longer usable. Any further operations will fail because the state file is gone.

## Querying state

The engine provides several read methods. All return copies, so you can't accidentally mutate internal state.

```go
// Current state name
state := eng.CurrentState()

// Template variables (copy)
vars := eng.Variables()

// Transition history (copy)
history := eng.History()

// Full state snapshot (for JSON serialization)
snap := eng.Snapshot()
data, _ := json.MarshalIndent(snap, "", "  ")
fmt.Println(string(data))

// State file path
path := eng.Path()

// Machine definition (deep copy)
m := eng.Machine()
```

`Snapshot()` returns an `engine.State` struct that maps directly to the JSON state file schema. It's useful for serializing the full state.

## Handling errors

All engine errors are `*engine.TransitionError` values. They carry a machine-parseable code, a human-readable message, and context fields.

```go
err := eng.Transition("deploy")
if err != nil {
    var te *engine.TransitionError
    if errors.As(err, &te) {
        switch te.Code {
        case engine.ErrTerminalState:
            fmt.Println("workflow is finished")
        case engine.ErrInvalidTransition:
            fmt.Printf("can't go to %s from here, try: %v\n",
                te.TargetState, te.ValidTransitions)
        case engine.ErrVersionConflict:
            fmt.Println("state file was modified by another process")
        default:
            fmt.Printf("engine error [%s]: %s\n", te.Code, te.Message)
        }
    } else {
        // Non-engine error (I/O, etc.)
        log.Fatal(err)
    }
}
```

`TransitionError` implements `json.Marshaler` through its struct tags, so you can serialize it directly:

```go
data, _ := json.Marshal(te)
// {"code":"invalid_transition","message":"...","current_state":"test","target_state":"deploy","valid_transitions":["deploy","validate"]}
```

See the [error code reference](../reference/error-codes.md) for the full list of codes and when each one occurs.

## Using the controller

The controller combines an engine with a compiled template to generate directives. It handles template hash verification and variable interpolation.

```go
import (
    "os"

    "github.com/tsukumogami/koto/pkg/controller"
    "github.com/tsukumogami/koto/pkg/engine"
    "github.com/tsukumogami/koto/pkg/template"
    "github.com/tsukumogami/koto/pkg/template/compile"
)

// Compile the source template.
sourceBytes, _ := os.ReadFile("/path/to/workflow.md")
ct, _, _ := compile.Compile(sourceBytes)
hash, _, _ := compile.Hash(ct)

tmpl, _ := ct.ToTemplate()
tmpl.Hash = hash

eng, _ := engine.Load("wip/koto-deploy.state.json", tmpl.Machine)

ctrl, err := controller.New(eng, tmpl)
if err != nil {
    // template_mismatch if the hash doesn't match
    log.Fatal(err)
}

directive, err := ctrl.Next()
if err != nil {
    log.Fatal(err)
}

if directive.Action == "execute" {
    fmt.Println(directive.Directive) // interpolated template section
} else {
    fmt.Println(directive.Message) // "workflow complete"
}
```

The `Directive` struct:

```go
type Directive struct {
    Action    string `json:"action"`              // "execute" or "done"
    State     string `json:"state"`               // current state name
    Directive string `json:"directive,omitempty"`  // instruction text (execute only)
    Message   string `json:"message,omitempty"`    // completion message (done only)
}
```

## Discovering workflows

The discover package finds active state files in a directory.

```go
import "github.com/tsukumogami/koto/pkg/discover"

workflows, err := discover.Find("wip/")
if err != nil {
    log.Fatal(err)
}

for _, w := range workflows {
    fmt.Printf("%s: %s (at %s)\n", w.Name, w.CurrentState, w.Path)
}
```

`Find` scans for files matching `koto-*.state.json` and reads only the metadata header from each. It returns an empty slice (not nil) when no files match. If some files can't be parsed, it returns partial results along with a non-nil error.

## Template interpolation

The `template.Interpolate` function does single-pass `{{KEY}}` replacement. Unresolved placeholders are left as-is.

```go
import "github.com/tsukumogami/koto/pkg/template"

text := "Deploy {{APP}} version {{VERSION}} to {{ENV}}."
ctx := map[string]string{
    "APP":     "api-server",
    "VERSION": "2.1.0",
}

result := template.Interpolate(text, ctx)
// "Deploy api-server version 2.1.0 to {{ENV}}."
// {{ENV}} is left unchanged because it's not in ctx
```
