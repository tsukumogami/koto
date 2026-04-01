# Decision 2 Report: override_default declaration and advance loop resolution

**Decision ID:** 2
**Topic:** gate-override-mechanism
**Question:** How is `override_default` declared in templates (new optional field on the Gate struct
in template/types.rs)? What are the built-in defaults per gate type? How does the advance loop
resolve "no --with-data" cases? What happens when no default exists and no --with-data is provided?

---

## Options evaluated

### Option A: Optional field on Gate struct (inline)

Add `override_default: Option<serde_json::Value>` to the `Gate` struct. Template authors declare
it inline, adjacent to the rest of the gate's configuration:

```yaml
gates:
  ci_check:
    type: command
    command: "run_ci.sh"
    override_default: {exit_code: 0, error: ""}
```

Built-in defaults per type are constants in code — not in templates. Resolution order:
instance `override_default` > built-in type default > error (no default, no --with-data supplied).

### Option B: Separate override_defaults block at state level

Template states carry a separate `override_defaults` map alongside `gates`:

```yaml
gates:
  ci_check:
    type: command
    command: "run_ci.sh"
override_defaults:
  ci_check: {exit_code: 0, error: ""}
```

Gate struct stays clean. The state struct gains a `BTreeMap<String, serde_json::Value>` field.

### Option C: No instance override_default; type defaults only

Only built-in per-type defaults exist. Agents must use `--with-data` for any custom override
value. Template authors cannot customize what "override" means per gate instance.

---

## Analysis

### Option A

The main virtue of Option A is co-location. A gate's override behavior sits next to the gate
itself — the template author never has to cross-reference two separate map structures. This
matches how the existing Gate fields work: `timeout` is similarly optional and gate-specific,
serialized with `skip_serializing_if = "is_zero"`. Adding `override_default` follows the same
pattern.

The implications for downstream features are straightforward. Feature 3 (compiler validation)
checks `gate.override_default.is_some()` and validates the JSON against the gate type's schema —
no indirection. The `agent_actionable: true` flag (R4a) is set when `gate.override_default.is_some()
|| built_in_default_exists(gate.gate_type)` — the check lives at the gate level without needing
to traverse a parent structure. The advance loop resolves "no --with-data" as:

```
if let Some(val) = &gate.override_default {
    val.clone()                         // instance default
} else {
    built_in_type_default(gate.gate_type)  // type default (always exists)
}
```

The constraint that "the field must be discoverable at compile time" is trivially satisfied —
the field is part of the struct that the compiler already processes gate-by-gate.

Adding a sixth field to Gate is explicitly acceptable per the stated constraints. The existing
struct already has fields that don't apply to all gate types (`command` is unused for context
gates; `key` and `pattern` are unused for command gates), so `override_default: Option<Value>`
with `#[serde(default, skip_serializing_if = "Option::is_none")]` adds negligible noise.

### Option B

Option B's motivation — keeping the Gate struct clean — is real but minor. The cost is higher.
A parallel `override_defaults` map at the state level means a gate's full specification is split
across two keys. Template authors can write an `override_defaults` entry for a gate name that
doesn't exist, and vice versa. The compiler must cross-validate two independent maps to catch
this, adding a validation path that has no equivalent elsewhere in the template schema.

The CLI-layer `agent_actionable` check becomes awkward. The advance loop receives a `&Gate`
reference, but to determine whether the gate has an instance override_default it must reach back
to the parent state's `override_defaults` map. This requires threading additional state through
the gate evaluation path or restructuring the function signature. Neither is necessary with
Option A.

Option B is a pattern sometimes used when the primary struct is shared and the annotation doesn't
belong to it — but `Gate` is not shared across contexts. It's declared per-state and consumed
in a single evaluation path. The separation provides no reuse benefit.

### Option C

Option C fails on requirements. PRD R4 explicitly says "Template authors can also declare
`override_default` per gate to route overrides to a different transition." The acceptance
criteria include: "Template author can declare a custom `override_default` per gate that differs
from the gate type's default, and the compiler validates it against the schema." Option C removes
this capability entirely.

The consequence is also behavioral: without per-instance override_default, every command gate
override yields `{exit_code: 0, error: ""}`. A template that uses override to route to a
"manual review" transition rather than the default "passing" transition can't express that intent
without forcing agents to always use `--with-data`. This shifts burden to the agent side for what
is fundamentally a template author's concern.

---

## Resolution rules for "no --with-data"

With Option A chosen, the advance loop resolves override values as follows:

1. **Gate has `override_default`** (instance): use `gate.override_default.clone()`.
2. **No instance default, gate type is known**: use the built-in type default:
   - `command` -> `{"exit_code": 0, "error": ""}`
   - `context-exists` -> `{"exists": true, "error": ""}`
   - `context-matches` -> `{"matches": true, "error": ""}`
3. **No instance default, gate type is unknown**: this is a compile-time error caught by Feature 3.
   At runtime it should not occur; if it does, treat as an override error and surface it.

This three-level rule means there is always a defined value when the agent calls `koto overrides
record` without `--with-data` — the call never fails for lack of a default on any valid gate type.

---

## Chosen: Option A

Option A keeps override configuration co-located with the gate, avoids cross-referencing two
parallel maps, and makes `agent_actionable` detection and compile-time validation trivially
straightforward. The incremental struct size is acceptable and follows existing patterns.

---

## Rejected options

**Option B** rejected: the parallel-map structure requires cross-validating two independent keys
at compile time, makes the CLI-layer `agent_actionable` check require traversal back to the parent
state, and provides no reuse benefit since `Gate` is not a shared type. Co-location is strictly
better here.

**Option C** rejected: directly contradicts PRD R4 and the stated acceptance criteria. Removing
per-instance override_default forces agents to use `--with-data` for cases the template author
should handle, and makes it impossible to route overrides to a non-default transition without
agent-side knowledge of the gate's value schema.
