# Architecture Review: Unified koto next Design

Reviewer role: Architect Reviewer
Design: DESIGN-unified-koto-next.md
Date: 2026-03-14

---

## Summary

The design is structurally sound and implementable. The event-sourced model is the right
unifying choice — it resolves the global-evidence contamination problem structurally rather
than by policy, simplifies the atomicity story, and makes all four sub-designs coherent
through a single shared concept. There are no blocking architectural violations. There are
three issues worth resolving before tactical sub-designs begin, and one simplification
opportunity that sub-design authors should evaluate.

---

## Question 1: Is the architecture clear enough to implement without out-of-band knowledge?

Yes, with one gap.

The event taxonomy, JSONL format, data flow, and template syntax examples are all
specified clearly enough for independent implementation. A sub-design author can write
the Event Log Format sub-design from the design doc alone.

**Gap: the `koto transition` command is not mentioned in the data flow or CLI surface
changes.** Currently `koto transition` is an independent command that calls
`engine.Transition` directly (cmd/koto/main.go:275). The design adds `--to` to `koto next`,
which does the same thing as `koto transition` but through the event log path. The design
says nothing about whether `koto transition` is removed, deprecated, kept with a warning,
or rerouted to the same code path. A sub-design author implementing the CLI Output Contract
(Phase 3) will have to guess. This needs a decision before Phase 3 begins.

**Gap: the `schema_version: 2` collision.** The existing engine (engine.go:67) already
writes `schema_version: 2` to the mutable JSON object format. The design proposes JSONL
with `schema_version: 2` in the header. These are structurally different files with the
same schema version number. The migration path says "if the file is a JSON object, synthesize
an event log" — but the migration code would need to parse the content to distinguish them,
not just check the version field. This is workable but the sub-design spec must say it
explicitly. Worth clarifying before Phase 1 sub-design is written to avoid the author
silently picking a scheme.

---

## Question 2: Are there missing components or interfaces that would block a tactical sub-design?

Two interfaces are underspecified at the boundary level:

**Integration runner interface.** The design says Phase 4 implements an "integration runner
interface" but does not define its contract. The CLI Output Contract (Phase 3) must produce
an `integration` field in the `koto next` response that includes `name` and `output`. For
Phase 3 to define `output` correctly, the runner's return type must be agreed on first.
Phase 4 depends on all three prior phases, but the runner interface shape actually needs to
be established in Phase 1 or Phase 3 so Phase 3 can define the `integration` response field
correctly. Without it, Phase 3 authors will have to assume the shape of `output`.

Concretely missing: what type is `output`? String? Structured JSON? What happens when the
runner fails — is the error in `output` or in the `error` field? The design shows `"output":
"..."` as a placeholder but does not resolve it.

**Evidence replay scope rule under `directed_transition`.** The design says:
> Current evidence: union of `evidence_submitted` events whose `payload.state` matches current state

After a `directed_transition` event, the current state changes. The new current state may
have prior `evidence_submitted` events from an earlier visit (looping workflow). The replay
rule does not say whether those prior-visit events are included or excluded. The data flow
says "Evidence for a state is the union of all `evidence_submitted` events whose `state`
field matches the current state" — which would include evidence from a prior visit. This
is the contamination problem the design is explicitly solving. A looping workflow that
revisits a state would carry forward evidence from the first visit. The Event Log Format
sub-design must specify the scope rule: either "evidence from the current epoch only" (where
an epoch resets on each rewind/directed_transition) or "evidence from the most recent entry
into this state." Without this, Phase 1 and Phase 4 will make different assumptions.

---

## Question 3: Are the implementation phases correctly sequenced?

Phases 1 and 4 are correctly sequenced. The parallel placement of Phases 2 and 3 is also
correct — neither depends on the other.

One sequencing issue: the integration runner interface shape (see above) is listed as Phase 4
scope but is needed by Phase 3 to define the `integration` field in the CLI output schema.
Phase 3 can define the field shape without a full runner implementation, but requires the
runner's output contract to be specified. The dependency is: Phase 3 needs the integration
output type spec; Phase 4 needs the full Phase 3 schema. The fix is to extract the runner
output type definition into Phase 1 (as part of the event taxonomy — the
`integration_invoked` event already has `output` in its payload fields).

No other sequencing issues.

---

## Question 4: Are there simpler alternatives to any specific architectural choice that were overlooked?

**JSONL vs. JSON object with append log.** The design chose JSONL, which requires a new
line-by-line reader and breaks every existing test that parses the state file as JSON. An
alternative is a JSON object with a top-level `events` array. This is append-unfriendly
at the raw file level (you'd have to rewrite the closing `]}`), but koto already uses
temp-file-rename for atomicity — an `events` array rewrite is no worse. It would keep the
existing `json.Unmarshal` path working for the header and make migration from the current
format trivial (wrap the existing content and add an events array). The design's stated
reason for JSONL is append simplicity and sequence-number gap detection for partial writes —
both are valid, but the existing code does not use raw appends, so the JSONL append model
is a new operational pattern the codebase has not established. Worth noting in the sub-design
for the implementer to evaluate before writing the reader.

This is an advisory note, not a blocker. The JSONL approach is correct for the stated goals.

---

## Findings Summary

**Blocking (must resolve before sub-designs begin):**

1. **`koto transition` disposition is unspecified.** The design adds `koto next --to` as
   a directed transition path but does not say what happens to the existing `koto transition`
   command. The CLI Output Contract sub-design cannot be written without this decision.
   Options: deprecate and proxy to `koto next --to`; remove; keep as an alias. Pick one
   before Phase 3.

2. **`schema_version: 2` naming collision.** The existing format already uses
   `schema_version: 2` for the mutable JSON object. The Event Log Format sub-design must
   specify how the loader distinguishes old v2 from new v2 — version number alone is
   insufficient since both formats can have the same value. Specify this in Phase 1.

3. **Evidence replay scope under directed_transition and loop re-entry.** The current replay
   rule "union of `evidence_submitted` events whose `state` matches current state" includes
   evidence from prior visits to the same state in looping workflows. This is the exact
   contamination problem the design exists to prevent. The Event Log Format sub-design
   must specify the epoch boundary rule before Phase 4 can implement the advancement loop
   correctly.

**Advisory:**

4. **Integration runner output type needs to live in the event taxonomy.** The
   `integration_invoked` event's `output` field is typed as a placeholder. Phase 3's CLI
   output schema depends on this type. Moving the output type definition from Phase 4 scope
   into Phase 1 (event taxonomy) removes the implicit dependency.

5. **JSONL read path is a new pattern for this codebase.** All existing state file code
   uses `json.Unmarshal` on the full file. The implementer should evaluate whether a
   JSON-object-with-events-array alternative eliminates the line-by-line reader without
   losing the append semantics koto already achieves via temp-file-rename. This doesn't
   change the design decision but is worth an explicit evaluation note in the Phase 1
   sub-design.
