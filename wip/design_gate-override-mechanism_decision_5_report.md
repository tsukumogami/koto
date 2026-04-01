# Decision 5: gates.* namespace reservation enforcement

## Question

Where in the call stack should `gates.*` key submissions be rejected, and does this
reservation extend to `koto context set` as well as `koto next --with-data`?

## Code walkthrough

### Evidence submission path (`src/cli/mod.rs`, `handle_next`)

When `--with-data` is provided, `handle_next` performs these checks in order:

1. Terminal-state guard (rejects if the current state is terminal).
2. Accepts-block guard (rejects if the state has no `accepts` block).
3. JSON parse.
4. `validate_evidence(&data, accepts)` — rejects unknown fields, wrong types, missing
   required fields. Any key not in the `accepts` map produces `FieldError { reason: "unknown field" }`.
5. On success, the payload is stored as an `EvidenceSubmitted` event in the state file.

Because step 4 rejects all keys that are not declared in `accepts`, a submission of
`{"gates": {...}}` is already rejected today — provided no template ever declares `gates`
as an accepted field. PRD R7 adds an explicit reservation so this holds even for templates
that accidentally or maliciously declare a `gates` field in their `accepts` block.

### Engine merge point (`src/engine/advance.rs`, `advance_until_stop`)

After loading `current_evidence` from the state log (all `EvidenceSubmitted` fields for
the current epoch), the advance loop builds a merged map:

```
merged = current_evidence (flat agent keys)
merged["gates"] = gate_evidence_map   // engine-injected, overwrites any collision
```

The TODO comment at line 356 explicitly acknowledges: "once Feature 2 reserves the
`gates` namespace in evidence validation, the precedence comment above shifts from
'defense in depth' to 'invariant'."

This means the engine currently overwrites any `"gates"` key that an agent managed to
sneak in. The overwrite is silent -- it produces no error, no log entry, and the agent
has no feedback that their submission was discarded.

### Context store (`src/cli/context.rs`, `src/session/context.rs`)

`koto context set` maps to `handle_add`, which calls `store.add(session, key, content)`.
The context store is a flat filesystem key-value store (stored under `ctx/` in the
session directory). It is entirely separate from the evidence map -- its contents are
never merged into the `merged` map in `advance_until_stop`, nor are they validated
against `accepts`. The key `"gates"` in context maps to a file named `gates`, not to
the engine-internal `gates.*` evidence namespace. There is no collision risk.

## Option analysis

### Option A: CLI layer only (reject in `handle_next` before appending `EvidenceSubmitted`)

**Pros:**
- Error messages are immediate and user-facing. The agent learns about the problem at
  submission time, not after the fact.
- The rejection happens before the event is persisted, so state files never contain
  evidence with a `"gates"` top-level key.
- Clean implementation: add one check after JSON parse in `handle_next`, return
  `InvalidSubmission` with a clear message (`"gates" is a reserved field`).
- The `GATES_EVIDENCE_NAMESPACE` constant is already defined in `template/types.rs`;
  the CLI can import and use it directly.
- Consistent with how `validate_evidence` already rejects unknown fields. The reservation
  is simply an unconditional extension of that logic, running before the accepts check
  so it applies even when a template erroneously declares `gates` in its `accepts` block.

**Cons:**
- A crafted or corrupted state file containing an `EvidenceSubmitted` event with a
  `"gates"` key (written outside the CLI, e.g., by a test harness or manual edit) would
  reach the engine. The engine's overwrite behaviour handles this silently today, but it
  means the invariant is not enforced at the source of truth for state-file integrity.

### Option B: Engine layer only (strip/reject in `advance_until_stop`)

**Pros:**
- Catches any path that writes evidence, including test harnesses or future CLI additions
  that do not go through `handle_next`.

**Cons:**
- The agent gets no feedback. The `"gates"` key is silently overwritten by the engine
  merge. From the agent's perspective, the submission appeared to succeed.
- This is already the current behavior (overwrite on merge). Formalizing it without a
  CLI-layer rejection means the agent can submit garbage and never know.
- Errors at the engine layer require either panicking (breaking the advance loop) or
  silently discarding agent data. Neither is acceptable for an explicit reservation whose
  purpose is to surface the collision clearly to the agent.

### Option C: Both layers

**Pros:**
- Defense-in-depth. The CLI catches the common case and gives a clear error. The engine
  layer enforces the invariant for any source that writes events directly.
- Matches the TODO comment in `advance.rs` line 356, which anticipates Feature 2
  converting the current silent overwrite into a hard invariant.
- Consistent with the codebase's approach elsewhere (e.g., `accepts` block validation
  exists both at compile time in `validate_evidence_routing` and at runtime in
  `handle_next`).

**Cons:**
- Slightly more code. The engine-layer check needs to decide: return an `AdvanceError`
  and surface it as an error in `koto next`, or silently drop the key. If it returns an
  error, the agent can't advance the workflow at all (worse than the CLI rejection, which
  tells them what to fix). If it silently drops, it provides no additional protection.

**Assessment:** The engine layer's role in this invariant is narrower than it first
appears. The engine's `current_evidence` is populated from `EvidenceSubmitted` events in
the state file, not from the CLI payload directly. If the CLI layer reliably blocks
`"gates"` keys from ever being written to `EvidenceSubmitted` events, the engine layer
never encounters a collision in practice. The TODO comment in `advance.rs` confirms this
framing: Feature 2 is meant to guarantee the invariant at submission time, converting the
engine's defensive overwrite from a fallback into dead code.

### Option D: CLI layer for `koto next`, not for `koto context set`

**Analysis:**
This is not a meaningful distinction to make for the implementation decision, but it is
the correct characterization of scope. Context and evidence are structurally separate:

- `koto context set myflow gates somevalue` writes to `~/.tsuku/.../ctx/gates` (a file).
- Evidence lives in `EvidenceSubmitted` events in the state JSONL file.
- The advance loop never reads context keys into the evidence merge map.

The `gates` string appearing as a context key has no semantic overlap with the
`gates.*` evidence namespace. There is no collision, no reservation needed, and adding
one would be confusing (why can't agents store artifacts under a key named "gates"?).

This option is not a standalone implementation choice -- it is a clarification that the
reservation applies to evidence submission only.

## Chosen approach

**Option C**, with the emphasis of the decision falling on the CLI layer as the primary
enforcement point and the engine layer as a lightweight invariant check.

Specifically:

1. **CLI layer (`handle_next`)**: After parsing the JSON payload and before calling
   `validate_evidence`, check whether the top-level object contains a key equal to
   `GATES_EVIDENCE_NAMESPACE` ("gates"). If so, return `InvalidSubmission` with a
   message such as `"\"gates\" is a reserved field; agent submissions must not include
   this key"`. This check is unconditional -- it runs regardless of what the template's
   `accepts` block declares.

2. **Engine layer (`advance_until_stop`)**: Convert the current silent overwrite to an
   explicit assertion. If `current_evidence` contains a top-level `"gates"` key when
   building the merge map, log a warning (or in debug builds, panic). This will never
   trigger in normal operation after the CLI check lands, but it documents the invariant
   in the code and protects against non-CLI writes to the state file.

3. **Context store**: No change. `koto context set` operates on a separate namespace.
   The reservation does not apply there.

## Why not A alone?

Option A is sufficient for production operation, and Option C's engine check is close to
dead code once A is in place. But the TODO comment in `advance.rs:356` was written with
exactly this in mind -- it expects Feature 2 to make the overwrite an invariant rather
than a fallback. Honoring that expectation keeps the code self-documenting. The engine
check is a one-line assertion, not a complex code path.

## Implementation note

The check should be added to the CLI layer's evidence submission path, immediately after
JSON parse and before `validate_evidence`. The `GATES_EVIDENCE_NAMESPACE` constant
already exists in `src/template/types.rs` and is already imported by `advance.rs`.
Adding an import in `cli/mod.rs` and a three-line check is the full implementation cost
for the CLI layer. The engine assertion is similarly minimal.

The `validate_evidence` function in `src/engine/evidence.rs` already rejects unknown
fields, so for templates that do not declare `gates` in `accepts`, the reservation is
already enforced via that path. R7 adds the guarantee for templates that mistakenly
declare it. The CLI pre-check runs before `validate_evidence` and is unconditional.
