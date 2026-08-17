# Exploration Decisions: self-loop-suppresses-details

## Round 1

- **Fork the slice rather than edit it**: the delivery predicate gets its own
  boundary rule; `occupancy_slice` keeps its current semantics for
  `latest_epoch_gate_failed`. Rationale: the shared helper also decides the
  dashboard and `/workflows` blocked badge, which has no unit-test coverage, and
  widening its window across a self-entry would silently change that badge. The
  two predicates answer different questions -- "since the machine last entered
  this state at all" versus "since the agent last arrived here from somewhere
  else" -- so they legitimately disagree, and the doc comment claiming they must
  not has to be rewritten to say why.

- **A rewind always opens a delivery occupancy, regardless of `from`**: the
  sameness test applies to `Transitioned` and `DirectedTransition` only.
  Rationale: `Rewound { from: P, to: P }` is reachable today (rewind right after
  a self-loop, because `handle_rewind` targets the second-to-last entry event's
  `to`), and the brief's own justification for rewind delivering -- "a rewind is
  a 'redo this' signal and the agent is being sent back deliberately" -- applies
  to a self-rewind exactly as much as to any other. It also decouples this work
  from koto#199, which is explicitly out of scope: whatever that fix changes
  about which events a rewind appends, a rewind still delivers.

- **`koto next --to P` while at `P` suppresses**: the brief rules this
  explicitly. The asymmetry with rewind is intended and must be argued in the
  DESIGN, not left to fall out: `--to P` says "route to P", which on a phase you
  already occupy is a lap of the same loop; a rewind says "discard what you did
  and do it again", which is a different instruction and needs the procedure.

- **Amend the PRD and DESIGN in place; do not supersede**: only the occupancy
  boundary moves. All four of the DESIGN's decisions (record delivery as an
  event, extend `koto status`, share one combinator, splice the recovery
  pointer) stay correct, so a successor DESIGN would restate them verbatim.
  Precedent exists (`70ba97c` corrected a Current design in place against the
  real code). The documented supersession tool writes a body line that fails
  FC03 on any design carrying `schema:`.

- **Rewrite, do not delete, the "A contradiction in the PRD was corrected"
  passage**: it records a decision now being reversed. Deleting it erases the
  audit trail; leaving it makes the document self-contradictory. It becomes a
  record of both rulings and which one governs.

- **Sweep the two stale "not emitted yet" claims** in
  `docs/reference/session-feed.md` and `src/engine/types.rs` in the same change.
  They describe this feature, they are wrong on `main` today, and the acceptance
  criterion is that no durable artifact is left describing the old rule.

- **Do not add a `koto phase-info` command** (AC 4 is met by `koto status`), do
  not touch `derive_visit_counts`, do not fix koto#193/#198/#199/#200, and do
  not edit `tests/fixtures/next-response-baseline/instruction-free.json`.
