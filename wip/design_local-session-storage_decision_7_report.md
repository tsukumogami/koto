# Decision 7: SESSION_DIR Substitution Mechanism

## Question

How should `{{SESSION_DIR}}` be substituted into directives and gate commands? Where in the pipeline does substitution happen, what infrastructure is needed, and how does it interact with future `--var` support?

## Decision

**Chosen: Runtime substitution at the output boundary in `handle_next` and the gate closure (Option 1, narrowed)**

Substitute `{{SESSION_DIR}}` at two points in the `handle_next` flow:

1. **Gate commands**: In the `gate_closure` lambda (line ~767 of `src/cli/mod.rs`), replace `{{SESSION_DIR}}` in each `Gate.command` string before passing gates to `evaluate_gates()`. This happens per-gate, just before shell execution.

2. **Directives**: After `dispatch_next` / `StopReason` mapping produces a `NextResponse`, replace `{{SESSION_DIR}}` in the `directive` string before serialization. This is the last step before `println!`.

Both substitutions use a single `fn substitute_vars(input: &str, vars: &HashMap<String, String>) -> String` utility that does literal `str::replace` for each key-value pair. For now, the only entry in the map is `SESSION_DIR` -> the resolved session directory path.

## Confidence

High.

## Rationale

The codebase has a clear output funnel: `handle_next` is the single function that (a) feeds gate commands to the shell and (b) serializes NextResponse to stdout. Substituting at these two points covers both consumers (shell execution and agent-facing JSON) without touching the engine, template compiler, or state file format.

This is the minimum-touch approach:

- **No new types or traits.** A plain `HashMap<String, String>` and a free function.
- **No engine changes.** `dispatch_next`, `advance_until_stop`, and `evaluate_gates` stay untouched. The substitution wraps their inputs/outputs at the CLI layer.
- **No compile-time baking.** The state file stores raw `{{SESSION_DIR}}` tokens. If the session directory moves (backend change, repo relocation), the next `koto next` call resolves it correctly.
- **No shell env leakage.** Unlike Option 3 (env var injection), the substitution is explicit and visible in the JSON output. The agent sees the resolved path in the directive, not an opaque `$SESSION_DIR` reference.
- **Clean --var extension point.** When `--var` lands (issue #67), the `vars` HashMap grows: CLI-provided vars are merged after built-ins, built-ins like `SESSION_DIR` refuse override, and the same `substitute_vars` function handles everything. No second substitution path needed.

### Why runtime, not compile-time

`SESSION_DIR` depends on the backend's session directory, which is resolved from the workflow name and the backend configuration at runtime. Baking it at `koto init` time would freeze the path into the state file, breaking if the user switches backends or moves `~/.koto/`. Runtime substitution is strictly more correct.

### Why not shell env vars for gates

Option 3 (setting `SESSION_DIR` as an env var for `sh -c`) would work for gates but leaves directives unresolved. That forces two different substitution mechanisms. It also makes gate behavior depend on implicit environment state rather than explicit string content, making debugging harder. A single `str::replace` pass before shell execution is simpler and keeps gates and directives symmetric.

## Implementation Sketch

```rust
// src/cli/vars.rs (new file, ~15 lines)
use std::collections::HashMap;

pub fn substitute_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for (key, value) in vars {
        let token = format!("{{{{{}}}}}", key); // produces {{KEY}}
        result = result.replace(&token, value);
    }
    result
}
```

In `handle_next`:

```rust
// Build the vars map once after resolving session_dir
let mut vars = HashMap::new();
vars.insert("SESSION_DIR".to_string(), session_dir.to_string());

// Wrap the gate_closure to substitute before evaluation
let gate_closure = |gates: &BTreeMap<String, Gate>| {
    let substituted: BTreeMap<String, Gate> = gates.iter().map(|(name, gate)| {
        let mut g = gate.clone();
        g.command = substitute_vars(&g.command, &vars);
        (name.clone(), g)
    }).collect();
    evaluate_gates(&substituted, &current_dir)
};

// After building NextResponse, substitute in directive
// (apply to the directive field before println)
```

Total new code: ~30 lines in a new `vars.rs` module, ~15 lines of wiring in `handle_next`.

## Assumptions

- `SESSION_DIR` is the only built-in variable needed for the local session storage feature. If more built-ins emerge before `--var`, they slot into the same HashMap.
- Template authors use the `{{VAR_NAME}}` syntax consistently. No escaping mechanism is needed yet (no template will contain literal `{{SESSION_DIR}}` that should NOT be substituted).
- The `substitute_vars` function does not need to be recursive or order-dependent. Simple sequential `str::replace` is sufficient.

## Rejected Alternatives

### Option 2: Compile-time substitution in `koto init`

Bakes the resolved path into the state file at initialization. Rejected because SESSION_DIR depends on runtime backend configuration. If the session directory changes after init (backend switch, `~/.koto` relocation, repo move), the frozen path becomes stale. Runtime substitution is strictly more correct and costs nothing extra.

### Option 3: Shell environment variable for gates only

Sets `SESSION_DIR` as an env var when spawning `sh -c` for gate commands. Rejected because it only solves half the problem -- directives still need a separate substitution mechanism. Two different approaches for the same concept increases maintenance burden and makes behavior harder to reason about. Also makes gate behavior depend on implicit env state rather than explicit command strings.

### Option 4: Build the full --var infrastructure now

Implements `Variables::substitute()` as a general-purpose system with CLI flag parsing, variable validation, required/optional semantics, and override protection. Rejected because it violates the "narrow scope" constraint. The `--var` flag needs its own design (issue #67 is tagged needs-design). Building the full system now risks designing the wrong abstraction before understanding all use cases. The chosen approach (HashMap + substitute_vars) is the natural foundation that `--var` will extend, so no work is wasted.
