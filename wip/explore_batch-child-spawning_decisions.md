# Exploration Decisions: batch-child-spawning

## Round 1

- Execution mode: switched to --auto on user request. Max rounds default 3.
- Scope narrowed: dynamic DAG execution with mid-flight task addition is in; cross-batch edges and distributed execution are out.
- Inter-child dependency ordering is required in v1 (user: "it's required -- these are GH issues that depend on each other").
- Failure routing: user deferred the choice; exploration must surface tradeoffs and recommend a default.
- Open interpretation: "task spawns sibling or grand-children" could mean (A) append to the same batch, or (B) start a nested batch. User declined to disambiguate -- investigate both and recommend in convergence.
- Adversarial lead skipped: source issue is labeled `needs-design`, not `needs-prd` or `bug` -- per --auto label-only rule, adversarial lead does not fire.

### Convergence-time decisions (post-Phase-2)

- **Reading A (flat declarative batch) is primary, Reading B (nested via `koto init --parent`) is complementary.**
  Rationale: GH-issue use case requires sibling-level dependencies, which pure parent-child nesting cannot express. Reading B remains available unchanged for hierarchical work.

- **Storage strategy: full derivation from on-disk state + event log.**
  Rationale: preserves append-only state file semantics; zero new cloud-sync surface; idempotency via existing `backend.exists()`; resume is the same code path as first invocation.

- **Insertion point: CLI-level scheduler tick in `handle_next`, post-`advance_until_stop`.**
  Rationale: advance loop is deliberately I/O-free; spawn needs session backend + compile cache; CLI layer already has all three. Keeps the engine pure.

- **Child naming: deterministic `<parent>.<task>`.**
  Rationale: no batch-id management; `backend.exists()` gives free idempotency; parents can't be renamed anyway.

- **Default failure policy: skip-dependents, per-batch configurable.**
  Rationale: aligns with GH-issue semantics, matches Argo/Airflow defaults, maximizes parallelism for independent branches, clean recovery via `retry_failed`.

- **CLI extensions: `--with-data @file.json` + new `json` accepts field type.**
  Rationale: both are surgical, both unlock more than batch spawn, both are prerequisites for submitting a task list.

- **Failure policy surface: per-batch only in v1; no per-task `trigger_rule` yet.**
  Rationale: ship simpler model first, validate with real use, add granularity later if needed.

- **Adversarial lead did not fire; no demand-validation block needed.**
  Rationale: #129 is labeled `needs-design`, pre-existing consumer (shirabe PR #67) is blocked on this, demand is self-evident.
