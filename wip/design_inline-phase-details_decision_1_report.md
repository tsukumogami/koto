# Decision D1: what records a delivery

## Question

R1 of `PRD-inline-phase-details` requires the include-instructions decision to be
keyed on "whether koto has already delivered that phase's instructions to a
caller during the current occupancy of that phase." Today koto keys it on
`derive_visit_counts` (`src/engine/persistence.rs:981-996`), which counts
`Transitioned` / `DirectedTransition` / `Rewound` events targeting a state, and
includes instructions when `full || count <= 1` (`src/cli/mod.rs:4010`). That
counts entries, not deliveries.

So: **what artifact records that a delivery happened, and where does it live?**
The answer determines whether R1 through R4 are implementable at all, because a
delivery is not currently observable anywhere.

### The defect, reproduced

Built binary at `target/release/koto`, scratch `HOME`, a two-phase template whose
`work` phase declares `<!-- details -->` instructions and an ungated `accepts`
block:

```
=== tick 1 ===
{"action":"evidence_required","advanced":false,...,"details":"LONG INSTRUCTIONS HERE.",...}
log after tick1:  133b4c9426fc4a34124a416cf2ca949a   4 lines
=== tick 2 (non-advancing repeat) ===
{"action":"evidence_required","advanced":false,...,"details":"LONG INSTRUCTIONS HERE.",...}
log after tick2:  133b4c9426fc4a34124a416cf2ca949a   4 lines
```

Two facts, both load-bearing for everything below:

1. The instructions are re-sent on the non-advancing repeat, exactly as the PRD
   describes. `koto init` appends `{"type":"transitioned","payload":{"from":null,"to":"work"}}`,
   so `count == 1` forever and `count <= 1` stays true forever.
2. **The session log is byte-identical before and after the second tick.** Same
   MD5, same line count. This is the single most important observation in this
   decision and it is developed under Option D.

## Decision drivers

- **R1/R2/R3/R4.** One rule, keyed on delivery within an occupancy, holding on
  the natural-advancement path and the directed-transition path alike.
- **R16 (via R9 of `PRD-koto-next-output-contract`).** Quoted verbatim from
  `docs/prds/PRD-koto-next-output-contract.md`, the "Visit tracking" bullet of R9:

  > **Visit tracking**: Uses existing JSONL state events. A state's visit count is
  > derived from the event log (counting `Transitioned`, `DirectedTransition`, and
  > `Rewound` events targeting that state). **No new state files or schema changes
  > needed.**

  Two prohibitions, and only two: no new state *files*, and no *schema* changes.
  It does not forbid new event variants. The `DESIGN-koto-next-output-contract`
  rejection is narrower still — it rejects one mechanism:

  > **Persist visit count alongside events**: Add a running counter to derived
  > state or a separate tracking file. Rejected because it violates PRD R9 ("no
  > new state files or schema changes").

  What was rejected is a *mutable persisted counter*, not an append-only record
  of a fact. That distinction is what keeps this decision open rather than
  already settled.
- **R17.** `koto-stability-tests` passes unmodified. This turns out to be
  decisive and eliminates one option outright.
- **R18.** No file read `koto next` does not already perform.
- **R10/R11.** Retrieval must not count as a delivery and must append no event.
- **The write cost.** Whether the mechanism turns a currently non-writing call
  into a writing one — the brief's sharpest cost, and it needs a more precise
  answer than "yes" or "no."

### What `koto next` writes today

Three separate questions, three different answers.

- **Does a non-advancing tick append to the *session log*?** For a **gated**
  state, yes: `append_event(&gate_evaluated_payload)` fires unconditionally for
  every non-overridden gate at `src/engine/advance.rs:389-390`, before the code
  decides whether the tick advances. For a **gateless `accepts`-only** state —
  the ordinary shape for a phase carrying long instructions — no: the
  `NeedsEvidence` arm returns `EvidenceRequired` at `src/engine/advance.rs:561-569`
  and `UnresolvableTransition` at `578-583` without any `append_event` call. The
  probe above confirms it empirically.
- **Does a non-advancing tick write to disk at all?** **Yes, always.** The
  discovery scan runs once per tick (`src/cli/mod.rs:4029`) and
  `write_cursor_post_scan` at `src/engine/discovery.rs:461` is on the main return
  path. Measured across two consecutive non-advancing ticks:

  ```
  cursor before:  d960d935ed3b3a4940772c717cbd9205   last_scan_at_unix_micros = 1786916916094671
  cursor after:   131d7c39e02959707b038404622f6340   last_scan_at_unix_micros = 1786916931052365
  session log:    133b4c9426fc4a34124a416cf2ca949a   (unchanged)
  ```

  `~/.koto/coordinators/<id>/scan_cursor.toml` is rewritten on every single
  `koto next`. This reframes the whole cost question: `koto next` is not a
  read-only call today and never has been. The objection is not "a pure reader
  becomes a writer"; it is narrower, and it is about the session log specifically.
- **Is the session log lock-protected on that write?** Only for batch-scoped
  parents. `src/cli/mod.rs:3746-3770` acquires the advisory flock for batch
  parents and states plainly that "Non-batch workflows intentionally skip the
  lock." Concurrency behavior of any new record has to survive that.

## Considered options

### Option A — a new `EventPayload` variant

**How it works.** Add `EventPayload::InstructionsDelivered { state: String }`,
serialized `type` `"instructions_delivered"`. `koto next` appends it at the
moment it includes a phase's instructions in a response. The delivery predicate
replaces the `count <= 1` check:

```rust
// src/engine/persistence.rs, alongside latest_epoch_gate_failed
pub fn instructions_delivered_this_occupancy(events: &[Event], state: &str) -> bool
```

Slice the log after the most recent state-entry event (`Transitioned` /
`DirectedTransition` / `Rewound`, any target — that is literally the PRD's
occupancy boundary), then ask whether the slice contains an
`InstructionsDelivered` naming `state`. Include instructions when
`full || !delivered`.

**Evidence.**

- The epoch-slicing shape is not new machinery. `latest_epoch_gate_failed`
  (`src/engine/persistence.rs:1006-1030`) and `derive_evidence`
  (`src/engine/persistence.rs:722-752`) both already compute "events since the
  last entry into the current state." The new predicate is the same slice with a
  different filter, in the same file, following the same `derive_*` convention.
- **No schema bump.** `CURRENT_SCHEMA_VERSION` is documented at
  `src/engine/types.rs:190-199`: bump for "a new *required* event type," a removed
  required field, or a change to the envelope keys. `docs/STABILITY.md`
  §`EventPayload` additive variants says the same and offers a worked example —
  the six `request.`-prefixed variants shipped together "with
  `CURRENT_SCHEMA_VERSION` left at 1, and a test asserts it"
  (`src/engine/types.rs:3378`).
- **Forward compatibility is mechanical, not aspirational.** `Event`'s custom
  `Deserialize` matches the `type` string against a table and drops anything
  unmatched into `EventPayload::Unknown { type_name, raw_payload }`
  (`src/engine/types.rs:782-799`); the custom `Serialize` writes the original
  strings back, so an older binary round-trips the record byte-identically.
  STABILITY.md names v0.9.0 as the floor: v0.8.4 and earlier have no catch-all
  and hard-error. `ContextRemoved`'s doc comment
  (`src/engine/types.rs:530-536`) already states this reasoning in house style
  for exactly this situation.
- **Newer koto on an older log:** absence of the record reads as "not delivered,"
  so the instructions ship once on the next tick of each occupancy. Fail-open in
  the harmless direction.
- **Blast radius is small and measurable.** `ContextRemoved` has exactly one
  reference outside `types.rs` (`src/cli/context.rs:112`) — the emitting site.
  `build_event_summary` ends in `other => other.type_name().to_string()`
  (`src/cli/dashboard_data.rs:862`), so the dashboard renders the new record as
  `instructions_delivered` with no code change. `per_state_outcomes`
  (`src/workflows_surface/project.rs:298-323`) matches only `GateEvaluated` and
  `EvidenceSubmitted` and falls into `_ => {}`, so the `/workflows` projection is
  untouched.
- **`koto-stability-tests` is unaffected.** It constructs
  `EventPayload::Unknown` and matches it with a `_ => unreachable!()` arm
  (`koto-stability-tests/src/lib.rs:113-129`); there is no exhaustive match over
  `EventPayload` anywhere in that crate.

**Write cost — the precise version.** If the marker is appended *when koto
delivers* rather than on every tick, the log grows by exactly **one line per
occupancy of an instructions-carrying phase**, not one per tick. Work through
where those writes land:

| Arrival path | Does that call already append to the log? | Marker adds a write? |
|---|---|---|
| Conditional/unconditional transition | Yes — `Transitioned` (`advance.rs:509`, `551`) | No, rides an existing writing call |
| Directed transition | Yes — `DirectedTransition` (`cli/mod.rs:3336-3349`) | No |
| Self-transition | Yes — `Transitioned` | No |
| First `koto next` after `koto init` | No | **Yes** |
| First `koto next` after `koto rewind` | No (the `Rewound` was appended by `rewind`, `cli/mod.rs:2044-2055`) | **Yes** |
| Non-advancing repeat (2nd..Nth tick) | Gated: yes. Gateless: no | No — already delivered, nothing appended |

So the honest statement of the cost: **two call shapes that append nothing to the
session log today would begin appending exactly one line — the first `koto next`
of an occupancy that a *previous command* started.** Every blocked re-tick stays
non-appending, which is the case the PRD's fourteen-iteration sweep is about.
This is a far narrower cost than "every blocked tick becomes a writer," which is
what the mechanism looks like if you record per tick instead of per delivery.

Phases declaring no instructions append nothing, ever — the marker is written
only where `details` is non-empty, so R6's byte-identity promise extends to the
log, not just the response body.

**Pros.**

- Says exactly the thing the rule needs to know. No inference, no proxy.
- Rewind works by construction: `koto rewind` appends `Rewound`, which is the
  newest entry event, so every prior marker falls outside the slice and the
  arrival response delivers. No special case.
- Multi-state auto-advance works by construction: the marker is written after the
  loop returns, so it lands after the last `Transitioned` and names
  `final_state` — the phase the loop actually stopped at. Phases crossed
  mid-chain get no marker and were never delivered, which is consistent.
- Duplicate markers are harmless. The predicate is an existence check, not a
  count, so the unlocked-append race on non-batch sessions cannot corrupt it.
- Directly auditable: `koto query --events` shows an operator when koto believes
  it delivered.

**Cons.**

- Two call shapes start writing where they did not (quantified above).
- Every append does `file.sync_data()` (`src/engine/persistence.rs:178`), so the
  added write is an fsync, not a buffered append.
- One more variant in an enum that already has ~25.
- Crash between the append and the response reaching the caller records a
  delivery that never landed. Mitigated by ordering — print first, append second —
  and ultimately by the R7 retrieval, which exists for precisely this class of
  failure.

### Option B — `EvidenceSubmitted` with a reserved dotted pseudo-state

**How it works.** No new variant. Append
`EvidenceSubmitted { state: "koto.instructions_delivered", fields: {"kind": "...", "phase": "<name>"} }`,
following the `request_store.` convention documented in `src/engine/audit.rs:1-110`.
The predicate slices the same way and looks for that reserved kind.

**Evidence.**

- The convention is real and documented: `ABANDON_NOTICE_STATE =
  "request_store.abandon_notice"` (`src/engine/audit.rs:64-74`), with
  `is_reserved_kind` (`src/engine/audit.rs:100-110`) consumed by the CLI parser
  to reject colliding `--with-data` submissions.
- **The synthetic state is mandatory here, not optional.** `derive_evidence`
  filters `EvidenceSubmitted` events *by `state == current_state` with no kind
  filter* (`src/engine/persistence.rs:750`), and the result feeds transition
  resolution. A marker written against the real phase name would be merged into
  the evidence map and could satisfy an `accepts` field or match a `when` clause,
  spuriously advancing the workflow. The `audit.rs` doc comment says this
  outright: "Every reserved audit record uses a synthetic state for exactly this
  reason." This is a correctness landmine, and Option B's viability depends
  entirely on stepping around it correctly.
- **Zero schema impact, zero new variant** — genuinely satisfies R16 and R17, and
  `koto-stability-tests` is untouched.
- The synthetic-state trick's safety rests on template state names not containing
  dots. I found **no state-name validator** enforcing that (`src/template/compile.rs`
  has no `validate_state_name`); the existing test
  `the_abandon_notice_kind_and_state_are_both_reserved`
  (`src/engine/audit.rs:367-381`) asserts the pseudo-state contains a dot and
  reasons from naming discipline, not a type-level guarantee. So the convention
  is safe by convention, and a new user of it inherits an unenforced
  precondition.

**Pros.**

- Strictly the smallest schema footprint of any recording option.
- Reuses infrastructure that already exists, including reserved-kind collision
  rejection at the CLI parser.
- Identical write-cost profile to Option A (same appends, same places).

**Cons.**

- **It lies in every projection that reads the log.** `per_state_outcomes`
  (`src/workflows_surface/project.rs:314-321`) buckets `EvidenceSubmitted` by its
  `state` with no kind filter, so `koto.instructions_delivered` materializes as a
  phantom phase in the `/workflows` projection carrying phantom evidence.
  `build_event_summary` (`src/cli/dashboard_data.rs:833`) renders it as
  "evidence: koto.instructions_delivered (2 fields)" in the dashboard timeline.
  `batch_view.rs:455` reads the family too. Each of those is a place to add a
  filter, or a place that displays something false.
- The `state` field no longer means what its name says, so the phase has to ride
  in `fields`, and the record becomes self-inconsistent — an evidence event that
  is not evidence, in a state that is not a state.
- It costs more code than Option A, not less: a reserved-kind constant, a
  synthetic-state constant, an `is_reserved_kind` extension, a reservation test,
  plus filters in the projections above. Option A's cost is a variant, a
  `type_name` arm, a deserialize arm, a payload struct, and a doc comment.
- The stated reason the convention exists does not apply here. `audit.rs`'s
  module doc says it exists because "PRD D10 requires zero new variants" — a
  constraint from `PRD-koto-request-store`, not R9. Adopting the workaround
  without the constraint that motivated it inherits the cost and none of the
  benefit.

### Option C — an additive `StateFileHeader` field

**How it works.** Add e.g. `instructions_delivered: Option<BTreeSet<String>>` to
`StateFileHeader`, rewritten in place when a delivery occurs and cleared on each
entry into a phase.

**Evidence.**

- **The header is genuinely rewritable in place; prior research was right and I
  confirmed it independently.** `append_header_line`
  (`src/engine/persistence.rs:87-116`) opens with
  `opts.create(true).write(true).truncate(false)` — no `.append(true)` — so the
  cursor starts at offset 0 and a second call overwrites the header line byte for
  byte. The file's own test `append_header_subsequent_write_is_well_formed`
  states this in its comment. `respawn_generation` and `dispatch_epoch` exercise
  it in production.
- `docs/STABILITY.md` §`StateFileHeader` additive evolution permits new
  `Option<T>` fields carrying `#[serde(default, skip_serializing_if = ...)]`
  without a schema bump, with `respawn_generation` as the worked example.

**This option is nonetheless eliminated, on R17.** `koto-stability-tests`
constructs `StateFileHeader` with **exhaustive struct literals, in two places** —
`state_file_header_resolves_and_constructs`
(`koto-stability-tests/src/lib.rs:68-92`) and
`path_buf_used_for_template_source_dir` (`koto-stability-tests/src/lib.rs:208-231`),
each listing all 22 fields. The struct is `#[derive(Debug, Clone, Serialize,
Deserialize, PartialEq)]` (`src/engine/types.rs:222`) — **not** `#[non_exhaustive]`
and **no** `Default`, so an external crate cannot write `..Default::default()`
and cannot omit a field. Adding a field is a compile error (E0063) in that crate.
The PRD's acceptance criterion is "`koto-stability-tests` passes unmodified" and
R17 pins that surface. Option C fails it as a matter of Rust language semantics,
not judgment.

Worth recording for the repo: STABILITY.md's header rules are written entirely
about *serde* compatibility and say nothing about *source* compatibility for
external constructors. The stability crate makes the header source-breaking on
any field addition. Those two documents disagree, and the disagreement is not
this feature's to resolve — but it does mean "additive header field" is more
expensive than STABILITY.md implies, for any future feature too.

**Other cons, had R17 not settled it.** The header is a whole-line rewrite with
no compare-and-swap. Non-batch sessions skip the flock entirely
(`src/cli/mod.rs:3751-3752`), so two concurrent `koto next` calls give a genuine
lost-update race — and unlike Option A, duplicate/lost writes here produce a
*wrong answer*, not a redundant one. The header also holds per-workflow scalars
today; a growing per-phase set is a different shape of thing, and it needs
explicit clearing on every entry event, which is a second place for the occupancy
semantics to drift from the log's.

**Pros.** No log growth at all. Bounded size. Fast to read — already parsed.

### Option D — record nothing; derive delivery from existing events

**How it works.** Find a predicate over today's log that answers "has koto
already delivered this phase's instructions during the current occupancy."

**This option is not viable, and the evidence is a proof rather than an
argument.**

The exact predicate the brief asked me to work out would have to distinguish
"first tick of this occupancy" from "second tick of this occupancy." For a
gateless `accepts`-only state — the ordinary shape for an instructions-carrying
phase — **the log is byte-identical in those two situations.** From the probe:
MD5 `133b4c9426fc4a34124a416cf2ca949a`, 4 lines, both after tick 1 and after
tick 2. No function of the event log can return different values for identical
inputs. There is no predicate. This is not a gap to be patched; it is a closed
question.

The code says the same thing: `resolve_transition`'s `NeedsEvidence` arm returns
`EvidenceRequired` (`src/engine/advance.rs:561-569`) and
`UnresolvableTransition` (`578-583`) with no `append_event` on either path, and
`gates_to_evaluate` is empty for a gateless state so the block at
`src/engine/advance.rs:372-393` never runs.

I verified the brief's specific sub-question — **is ANY event reliably appended
on a non-advancing tick?** No. `GateEvaluated` is appended only when the phase
declares at least one non-overridden gate. `DefaultActionExecuted`
(`src/engine/advance.rs:396-405`) only when the phase declares a default action.
`EvidenceSubmitted` only on `--with-data`. `IntentUpdated` only at init. There is
no unconditional per-tick event. The only unconditional per-tick *write* anywhere
is the discovery cursor at `~/.koto/coordinators/<id>/scan_cursor.toml`, which is
per-coordinator rather than per-phase, is shared across every workflow that
coordinator ticks, carries no phase identity, and is TTL-garbage-collected
(`gc_stale_cursors`, `src/cli/mod.rs:2950`). Keying instruction delivery on it
would be indefensible.

**The two sub-variants people reach for, and why each fails.**

- *Count `GateEvaluated` since the last entry.* Correct for gated phases, blind
  for gateless ones — and it makes the delivery rule depend on whether the
  template author happened to write a gate, which is precisely the
  shape-dependence R4 exists to eliminate. Prior research called this incomplete;
  I confirm it, and add that the incompleteness lands on the *more* common shape,
  since a phase carrying a long procedure is typically waiting on agent evidence,
  not on a gate.
- *Suppress on `advanced == false`.* Empirically broken. The probe's tick 1 —
  the genuine first delivery — reports `"advanced":false`, because `koto init`
  performed the entry transition and the first `koto next` transitions nothing.
  Suppressing on `advanced == false` would withhold the instructions on the
  feature's most basic scenario. (Note: prior research reached the right
  conclusion here via a wrong intermediate claim — it says `koto init` leaves no
  `Transitioned` event. It does: `{"seq":2,...,"type":"transitioned","payload":{"from":null,"to":"work"}}`.
  The count is 1, not 0, and the `count <= 1` check absorbs both.)

**Pros.** Zero new writes; zero schema surface; nothing to document.
**Cons.** It cannot satisfy R1 or R2. That ends it.

### Option E — variants I considered and set aside

- **A generic per-tick event (`PhaseTicked`).** Mechanically Option A, but
  written on every tick rather than on delivery, and for every phase rather than
  instruction-carrying ones. Strictly worse: it grows the log by one line per
  tick on every workflow in existence, and it changes the log for templates that
  declare no instructions, putting avoidable pressure on R6's byte-identity
  baseline. Rejected in favor of recording the specific fact.
- **Reuse `DecisionRecorded`.** Carries a `state` and is consumed by
  `derive_decisions` (`src/engine/persistence.rs:759+`) with the same
  epoch-slicing shape — so it inherits Option B's pollution problem plus a worse
  semantic mismatch. Rejected.
- **Record in the request store.** A separate file family; effectively "a new
  state file" in R16's sense, and R11 explicitly forbids the retrieval from
  writing there. Rejected.
- **Client-supplied acknowledgement** (the agent tells koto it received them).
  Would actually be *more* correct than any log-derived rule, since it answers
  the question koto is really asking. But it requires a protocol change on every
  caller, and an agent whose context was compacted cannot acknowledge anything —
  the case the PRD assigns to the retrieval. Out of scope and out of proportion.

## Recommendation

**Option A: a new `EventPayload::InstructionsDelivered { state }` variant,
appended by `koto next` at the moment it includes a phase's instructions, with
the delivery predicate computed over the events after the most recent state-entry
event.** Confidence: **high** for the recording-vs-not-recording half (Option D is
disproved, not merely disfavored, and Option C is eliminated by a compile-time
fact); **moderate-to-high** for A over B, where the argument is design quality and
total code cost rather than feasibility.

Why:

1. **Nothing else can work.** D is impossible (identical logs), C breaks R17 at
   compile time. Only A and B remain, and they have identical write-cost and
   compatibility profiles — so the choice between them turns entirely on which
   record is honest.
2. **R16 permits it, verbatim.** R9 forbids new state files and schema changes.
   A new variant is neither: `CURRENT_SCHEMA_VERSION` stays at 1 under the rule
   its own doc comment states, the same log file is used, and the precedent is
   six `request.` variants shipped in one change with a test pinning the
   constant.
3. **B pays more to say less.** B needs a reserved kind, a synthetic state, an
   `is_reserved_kind` extension, a reservation test, and filters in at least the
   `/workflows` projection and the dashboard timeline — and after all that, the
   record still claims to be evidence submitted in a state that does not exist.
   A needs a variant, a `type_name` arm, a deserialize arm, a payload struct, and
   a doc comment, and every existing projection handles it correctly through an
   arm that is already there. Fewer moving parts *and* a truthful record.
4. **The occupancy semantics fall out for free.** Rewind, self-transition,
   directed transition, and multi-hop auto-advance all work without special
   cases, because the predicate keys on position relative to the last entry
   event, and every one of those paths appends an entry event.

Concrete shape:

- `src/engine/types.rs` — variant, `type_name()` arm (`"instructions_delivered"`),
  `Deserialize` match arm, `InstructionsDeliveredPayload` struct, doc comment in
  the `ContextRemoved` house style explaining additive safety, and extend the
  existing `CURRENT_SCHEMA_VERSION == 1` assertion's coverage.
- `src/engine/persistence.rs` — `instructions_delivered_this_occupancy(events,
  state) -> bool`, next to `latest_epoch_gate_failed`, sharing its slicing idiom.
- `src/cli/mod.rs:3999-4016` — replace the `derive_visit_counts` / `count <= 1`
  check with the predicate; append the marker when instructions are included.
  Keep the existing `if final_template_state.details.is_empty()` guard, which
  keeps both the extra read and the extra write off instruction-free phases and
  preserves R6 and R18.
- `src/cli/next.rs:50-54` and `src/cli/mod.rs:3355` — the directed path (D3's
  business, but D1's record is what makes one rule possible there).
- `derive_visit_counts` **stays** — it is the shared derivation the PRD's Out of
  Scope section explicitly declines to change, and other callers may rely on it.
  This decision stops using it for the delivery question; it does not remove it.
- Ordering: print the response, then append the marker, then exit. A crash
  between them re-delivers, which is the benign direction.
- `--full` records a delivery too. It is a delivery. Recording it changes no
  observable behavior for existing `--full` callers (verified by case analysis:
  the only scenario where it could differ is `--full` on the first tick of an
  occupancy, where the rule would have delivered anyway), and it keeps the
  invariant to a single sentence: *koto records a delivery whenever a `koto next`
  response carries the instructions.*

## Case against the recommendation

The strongest case I can build against Option A, stated as forcefully as I can:

**"You are making `koto next` write to the session log on calls that today write
nothing, to fix a display bug. The very first `koto next` after `koto init` — the
single most common call in koto — becomes an fsync where it was a pure read.
Worse, you are teaching the event log to record koto's own I/O rather than the
workflow's history. Every other variant in that enum records something that
happened to the *workflow*: it transitioned, a gate ran, evidence arrived, an
action executed. `InstructionsDelivered` records something that happened to
*koto's output buffer*. That is a category error, and it is the kind of category
error that metastasizes: once the log records what koto printed, the next feature
will want to record what koto printed *last time*, and the log stops being a
state machine's history and becomes a telemetry stream. And you cannot even get
the fact right — you record 'delivered' before you know the caller received it,
and the PRD's own Known Limitations section admits the record will be wrong for
compacted and respawned agents anyway. You are adding a permanent schema-surface
commitment to a log, at a per-call fsync cost, for a fact you concede is
unreliable."**

**Does it survive? Mostly not, but one part of it does.**

The I/O half does not survive contact with measurement. `koto next` already
rewrites `scan_cursor.toml` on every single call, measured above — including the
first call after `init`. That call is not a pure read today and has not been
since the discovery cursor shipped. Adding one appended line plus one `sync_data`
to a file the process has already opened and read is a small increment on an
existing write, not the loss of a purity property. And the increment is bounded
at one line per occupancy, not per tick.

The category-error half is the serious part, and it lands a real hit. It is true
that `InstructionsDelivered` is a different *kind* of fact from `Transitioned`.
But the enum has already crossed that line: `GateEvaluated` records the outcome of
something koto ran, `DefaultActionExecuted` records koto's own subprocess with
its stdout and stderr inline, and the entire `request_store.` audit family records
coordination decisions koto made about itself. The log is already a record of
what koto did as well as where the workflow went. More decisively: the delivery
*is* workflow history under this PRD's framing. R1 makes "koto delivered these
instructions during this occupancy" a fact the state machine's behavior depends
on — which is the definition of state, and state that behavior depends on belongs
in the log rather than being re-derived from proxies. The slippery-slope version
is answered by scope: the record is written only for phases that declare
instructions, only once per occupancy, and is read by exactly one predicate.

The "fact you concede is unreliable" charge misreads what is being claimed. The
record says *koto emitted the instructions in a response*, which is true with
certainty at the moment of writing. It does not claim the caller still holds
them — nothing can — and the PRD assigns that gap to the retrieval (R7-R14),
which is the whole reason the retrieval is mandatory rather than a convenience.
A record that is precisely true about a narrower fact is not an unreliable record.

The one residual I accept: the crash-between-print-and-append window is real, and
ordering only chooses which way it fails. Print-then-append fails toward
re-delivery, which is harmless. That is the right trade and it should be stated
in a comment at the call site so a later reader does not "tidy" the order.

## Consequences

**Positive.**

- R1 through R4 become implementable with one predicate applied at one place,
  and rewind, self-transition, directed transition, and auto-advance need no
  special cases.
- The log gains an operator-visible answer to "did koto ever send me that
  procedure," which today is unanswerable after the fact.
- The R6 guarantee strengthens: instruction-free templates get byte-identical
  responses *and* byte-identical logs, because the marker is gated on non-empty
  `details`.
- `koto-stability-tests` needs no edit. `CURRENT_SCHEMA_VERSION` stays 1. Older
  binaries (>= v0.9.0) read the new logs through `Unknown` unharmed.

**Negative.**

- The first `koto next` of an occupancy begun by a previous command — after
  `init` and after `rewind` — appends one line and fsyncs where it previously
  did neither.
- One more `EventPayload` variant to maintain, and one more entry in the
  additive-variant story that STABILITY.md tells.
- A delivery is recorded at emit time, not at receipt time; a crash in the gap
  loses one delivery's worth of instructions until the caller uses the retrieval.
- Binaries older than v0.9.0 hard-error on the new event type (no `Unknown`
  catch-all). That floor is pre-existing and documented, but it is now reachable
  by a common workflow rather than only by request-store users.

**Mitigations.**

- Print before appending, and comment the ordering as deliberate.
- Gate the append on non-empty `details` so the common instruction-free path is
  untouched.
- Use an existence predicate, not a count, so the unlocked concurrent-append path
  on non-batch sessions cannot produce a wrong answer — only a redundant record.
- Pin the schema constant in the same test that covers the new variant, matching
  `adding_the_request_family_does_not_move_the_schema_version`.
- Add the R18 evidence deliberately: the predicate reads `post_events`, which
  `src/cli/mod.rs:4004` already re-reads on this path, so no new read is
  introduced.

**Test surface (R19).** Unit tests on the predicate in `persistence.rs` covering
delivered/not-delivered, marker-before-entry (rewind), self-transition, and the
never-entered case. Behavior tests at the response-construction level for the six
arrival paths R3 enumerates plus the non-advancing repeat. A schema-version pin
test. `koto-stability-tests` unmodified. `koto template compile` over
`plugins/` unaffected — no template-format change.

## Open questions for cross-validation

**For D2 (the retrieval surface) — a hard constraint.** R10 says retrieving must
not count as a delivery and R11 forbids the retrieval from appending any event.
Under this decision that is trivially satisfiable *provided the retrieval does
not route through `handle_next`*. It cannot be a flag on `koto next`: that path
appends `GateEvaluated` (`advance.rs:389`), executes default actions
(`advance.rs:396-405`), rewrites the discovery cursor (`discovery.rs:461`), and
can call `finish_terminal_tick`. **D2 must choose a surface with its own handler.**

**For D3 (call-site unification).** There are exactly two places that decide
whether `details` rides a response: `src/cli/mod.rs:3999-4016` (natural path,
applies the check) and `src/cli/next.rs:50-54` reached from `src/cli/mod.rs:3355`
(directed path, unconditional). D1 supplies one predicate; D3 has to make both
sites call it *and* both sites record the delivery. If D3 unifies by pushing the
decision into `dispatch_next`, note that `dispatch_next` is a pure function today
with no backend handle — recording would have to stay in the caller, or
`dispatch_next` would have to return "instructions were included" for the caller
to act on. I would recommend the latter: it keeps the append at one site.

**A conflict inside the PRD that D1 forces into the open.** The Definitions
section says a self-transition "ends one occupancy and begins another, which
makes it behave exactly like a loop-back: the instructions are delivered again on
arrival," and R3 lists "a directed transition" and "a self-transition" among the
occupancy starts that must carry instructions. But an acceptance criterion says
"Two consecutive directed transitions into the same phase: the first carries the
instructions, the second does not." A second directed transition into the same
phase is only reachable when the template declares a self-transition — the
directed handler validates the target against the current state's declared
transitions (`src/cli/mod.rs:3304-3322`) — and it appends
`DirectedTransition { from: X, to: X }`, which is a new occupancy by the PRD's own
definition, so it must carry. **The two cannot both hold.** A predicate keyed on
occupancy implements the Definitions and the self-transition criterion, and
contradicts that one AC. My recommendation is to follow the Definitions (they are
normative and R3 is explicit) and rewrite that AC as "a directed transition into a
phase followed by a non-advancing `koto next`: the first carries, the second does
not," which is what the AC was plainly trying to test. This needs an explicit
ruling in the DESIGN rather than a silent choice.

**For D4 (the discoverability pointer).** The pointer must appear on every
non-terminal response for an instructions-carrying phase (R14) — *including
responses where the delivery rule suppressed the instructions*, which are exactly
the responses D1's marker causes. So the pointer's presence condition is
"`details` is non-empty on this phase," not "instructions are in this response."
D4 should not reuse D1's predicate for that decision.

**Smaller, for whoever writes the acceptance evidence.** The R18 criterion says a
`koto next` call must open "no file the pre-change binary did not open." The
marker opens no *new file* — `append_event` reopens the same `.state.jsonl` the
tick already read — but under `strace` it is one additional `openat` on that
path. R18's requirement text is about *reads* ("performs no file read it does not
perform today"), which the change honors exactly; the criterion's wording is
stricter than the requirement it tests. Worth a sentence in the DESIGN so the
verification step is not run against the stricter reading by accident.

**Unresolved and out of this decision's reach.** Issue #161 (a machine-readable
reserved-kind catalog) is referenced in prior research but appears nowhere in this
repo's committed state; it would only matter if D1 went to Option B, which it
does not. And the STABILITY.md / `koto-stability-tests` disagreement about whether
header fields are additive is a genuine repo-level inconsistency this decision
surfaces but should not fix.
