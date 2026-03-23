# Decision: How should substitution integrate into the advance loop?

## Chosen
Option B: Construct Variables in handle_next, capture in gate closure only

## Confidence
high

## Rationale

The code in `handle_next` (src/cli/mod.rs:384) already constructs closures that capture local state -- `gate_closure` captures `current_dir`, `append_closure` captures `state_path_clone`. Adding `Variables` follows this established pattern exactly.

The `advance_until_stop` signature (src/engine/advance.rs:130) takes three injected closures (`append_event`, `evaluate_gates`, `invoke_integration`) plus a shutdown flag. The `evaluate_gates` closure type is `Fn(&BTreeMap<String, Gate>) -> BTreeMap<String, GateResult>` -- it already hides I/O details (working directory, shell spawning) behind the closure boundary. Variable substitution on gate commands is the same kind of I/O detail: the closure in `handle_next` captures `&variables`, substitutes `gate.command` before passing to `evaluate_gates(gates, &current_dir)`, and `advance_until_stop` never needs to know about it.

Directive substitution happens after `advance_until_stop` returns. In `handle_next` (lines 787-931), the code reads `final_template_state.directive` when building every `NextResponse` variant. A single `variables.substitute(&directive)` call at that point covers all response paths. This is exactly two substitution sites: one inside the gate closure, one after the advance loop returns. Both are in `handle_next`, within about 150 lines of each other.

Option B preserves the `advance_until_stop` public API, which matters because:
- The function has extensive tests (9 test cases) that would all need signature updates with Option A
- The engine module is intentionally I/O-free; variable substitution is a caller concern
- Issue #71 (default action execution) will add its own closure or modify the integration closure -- keeping `advance_until_stop` lean gives that work more room

The two substitution sites are not a real concern. They serve different purposes (pre-execution shell safety vs. user-facing display) and live in the same function. If a third site appears, refactoring to a shared helper is straightforward.

## Rejected Alternatives

- **Option A (pass Variables as parameter)**: Changes the public `advance_until_stop` signature, breaking 9 existing tests. Pushes a caller-level concern (string replacement) into the engine module, which is deliberately I/O-free and operates on abstract closures. The benefit of "explicit data flow" is marginal since the variable data is already visible in `handle_next` where both substitution sites live.

- **Option C (generic string transformer closure)**: Adds a closure parameter that `advance_until_stop` would call on both gate commands and directives. This couples the engine to string transformation semantics it doesn't need to know about. It also conflates two different substitution contexts (shell commands vs. display text) under one interface, when future needs might diverge (e.g., shell-escaping for commands but not for directives). The added abstraction has no current consumer beyond variable substitution.

## Assumptions

- The `Variables` type's `substitute()` method is a pure string transformation with no error cases that need to propagate through the advance loop. If substitution could fail (e.g., missing required variable), Option A's explicit parameter would allow error propagation from inside the loop. The design doc's `from_events()` constructor and simple `HashMap`-backed newtype suggest substitution is infallible (missing variables left as-is or replaced with empty string).
- Issue #71 (default action execution) won't need variable substitution to happen inside the advance loop itself. If it does, the closure-capture approach still works since action execution will likely get its own closure parameter.
