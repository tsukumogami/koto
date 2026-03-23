<!-- decision:start id="cli-surface-decision-recording" status="confirmed" -->
### Decision: CLI surface for decision recording

**Context**

The mid-state decision capture design needs a CLI mechanism for agents to record
structured decisions (choice, rationale, alternatives_considered) during long-running
states without triggering the advancement loop. The event type (DecisionRecorded),
schema, and surfacing mechanism (--decisions flag on koto next) are all settled. The
open question is how the recording operation is invoked.

The prior design chose a --record flag on koto next. On review, the rationale was
found to rest on weak grounds: the "no new subcommands" constraint originated in the
parent template design, not in koto's architecture; the code-sharing concern is
trivially solved by extracting a helper; and the command count increase from 6 to 7
is not meaningful complexity.

**Assumptions**

- The --decisions flag on koto next is settled and not affected by this decision.
  Surfacing decisions on koto next is semantically appropriate because the consumer
  is asking "what should I do next, and what decisions have been made?"
- No other new subcommands are imminent that would make the 6-to-7 transition feel
  like the start of command proliferation.

**Chosen: New `koto record` subcommand**

Add a `Record` variant to the `Command` enum in `src/cli/mod.rs`. The command takes
a workflow name and a required `--with-data` flag containing the decision JSON payload.
The handler:

1. Loads the state file and verifies the template hash (shared helper with handle_next).
2. Validates the payload against the fixed decision schema.
3. Appends a `DecisionRecorded` event to the state file.
4. Returns the current directive without running the advancement loop.

```
koto record my-workflow --with-data '{"choice": "...", "rationale": "...", "alternatives_considered": [...]}'
```

The command is self-documenting: `koto record --help` describes recording a decision
in the current state. It appears at the top level in `koto --help`, making it
discoverable without knowing to look inside `koto next --help`.

**Rationale**

Three factors converge on a dedicated subcommand:

1. **Semantic accuracy.** koto's commands are named for their action: init initializes,
   cancel cancels, rewind rewinds. "next" means "advance or tell me what to do next."
   Recording a decision does neither. A --record flag on next overloads a command whose
   name contradicts the operation. `koto record` says what it does.

2. **Argument simplicity.** The --record flag on next creates a fourth mode with two
   new mutual exclusivity rules (--record + --to is error, --record without --with-data
   is error). A dedicated command has one argument pattern: name + --with-data. No
   exclusivity checks, no mode branching, no ambiguity.

3. **Discoverability.** Top-level commands appear in `koto --help`. Flags are one level
   deeper. For a new capability that agents need to learn exists, top-level visibility
   is the right default.

**Alternatives Considered**

- **`--record` flag on `koto next`** (prior decision). Keeps command count at 6,
  but overloads "next" with an operation that doesn't advance. The mutual exclusivity
  matrix grows, the flag is less discoverable, and the original rationale ("no new
  subcommands") was a parent design constraint, not a koto principle. Rejected for
  semantic mismatch.

- **`koto next --annotate`**. Renames --record to a more general term. Inherits all
  the problems of the --record flag (semantic mismatch with "next", mutual exclusivity,
  buried discoverability) while adding a new one: the flag name suggests generic
  annotation, but the schema enforces decision-specific structure (choice, rationale,
  alternatives_considered). The design already rejected generic annotation because it
  can't enforce decision structure. Rejected as strictly worse than both other options.

**Consequences**

koto's command count increases from 6 to 7. This is a marginal increase that adds
a clear, self-documenting entry point for a new capability. The handle_next function
stays focused on its three existing modes (bare, --with-data, --to) without additional
branching. State-file loading code should be extracted into a shared helper used by
both handle_next and handle_record, which is a small cleanup that improves the
codebase regardless.

The interaction pattern from the design doc changes:

```bash
# Recording (changed: koto record instead of koto next --record)
koto record my-workflow --with-data '{"choice": "Use retry with backoff", "rationale": "..."}'

# Surfacing (unchanged: --decisions stays on koto next)
koto next my-workflow --decisions
```

The asymmetry (record on its own command, retrieve via --decisions on next) is
intentional. Recording is a distinct action. Retrieval is context for "what should
I do next?" -- it belongs on next.
<!-- decision:end -->
