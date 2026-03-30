# Lead: How do `koto transition` and `koto submit` interact with auto-advancement state?

## Findings

### There is no `koto transition` or `koto submit` command

The commands `koto transition` and `koto submit` don't exist in the current codebase. They were removed as part of the unified `koto next` redesign (DESIGN-unified-koto-next.md, implemented in the Go-to-Rust migration per DESIGN-migrate-koto-go-to-rust.md). The old `koto transition <target>` was replaced by `koto next --to <target>`. Evidence submission (the conceptual "submit") was never a standalone command -- it's `koto next --with-data <json>`.

The CLI enum in `src/cli/mod.rs` (lines 54-139) defines exactly these subcommands: `Version`, `Init`, `Next`, `Cancel`, `Rewind`, `Workflows`, `Template`, `Session`, `Context`, `Decisions`, `Config`. The `Next` subcommand has `--with-data` and `--to` as optional flags.

### How `--to` (directed transition) works

Path: `src/cli/mod.rs` lines 1279-1365.

`--to` is a **single-shot path that bypasses the advancement loop entirely**. It:

1. Validates the target is a legal transition from the current state
2. Appends a `directed_transition` event
3. Calls `dispatch_next()` (the pure classifier from `src/cli/next.rs`) on the target state
4. Skips gate evaluation (empty gate_results map)
5. Hard-codes `advanced: true` in the dispatch call (line 1344)
6. Outputs the `NextResponse` and exits immediately

It does NOT call `advance_until_stop`. This means if the directed transition lands on an auto-advanceable state (no accepts block, no gates, not terminal), the caller gets an `EvidenceRequired` response with empty `expects` but **no further advancement happens**. The agent would need to call `koto next` again to trigger the advancement loop.

### How `--with-data` (evidence submission) works

Path: `src/cli/mod.rs` lines 1382-1466 (evidence handling), then lines 1506-1651 (advancement loop).

`--with-data` **does trigger the advancement loop**. It:

1. Validates the evidence against the state's accepts schema
2. Appends an `evidence_submitted` event
3. Falls through to the shared advancement path (steps 6-10 in the handler doc comment)
4. Re-reads events to include the just-appended evidence
5. Calls `merge_epoch_evidence` and then `advance_until_stop`
6. Maps the `AdvanceResult` (with its `advanced` field and `StopReason`) to a `NextResponse`

The `advanced` field here comes from `AdvanceResult.advanced`, which is set to `true` by `advance_until_stop` only when at least one state transition actually occurs during the loop.

### Bare `koto next` (no flags)

Also triggers the advancement loop, identical to `--with-data` except no evidence event is appended first. The advancement loop uses whatever evidence already exists in the current epoch.

### Output shape comparison

All three paths produce the same `NextResponse` types:

| Path | Auto-advancement? | `advanced` source | Output type |
|------|-------------------|-------------------|-------------|
| `--to` | No (single-shot) | Hard-coded `true` | `dispatch_next()` result |
| `--with-data` | Yes (loop) | `AdvanceResult.advanced` | `StopReason` -> `NextResponse` mapping |
| Bare `koto next` | Yes (loop) | `AdvanceResult.advanced` | `StopReason` -> `NextResponse` mapping |

The JSON serialization is identical across all paths -- same `NextResponse` enum, same `Serialize` impl in `next_types.rs`. The fields present depend on the variant, not the calling path.

### The `advanced` field semantics differ by path

This is the key contract gap:

- **`--to`**: `advanced` is always `true` because the directed transition itself is considered an advancement. But no auto-advancement chain runs, so `advanced: true` means "we moved to the state you asked for" not "the engine auto-advanced through states."
- **`--with-data` / bare**: `advanced` reflects whether the advancement loop made at least one transition. If evidence was submitted but didn't match any transition condition, `advanced` is `false`. If it matched and the engine auto-advanced through one or more states, `advanced` is `true`.

A caller that interprets `advanced: true` as "auto-advancement happened" would misinterpret `--to` results. A caller that interprets it as "state changed" would be correct for `--to` but would miss the distinction between "advanced to the next stop" and "stayed at the same state" for the loop path.

### Stale documentation referencing `koto transition`

Several docs still reference the removed `koto transition` command:

- `docs/guides/custom-skill-authoring.md` (lines 193, 220, 231, 244, 417, 547) -- instructs skill authors to use `koto transition`
- `docs/testing/MANUAL-TEST-agent-flow.md` (lines 65, 76, 87) -- test steps use `koto transition`
- `docs/designs/current/DESIGN-koto-template-format.md` (lines 584, 586, 599) -- references `koto transition` with `--evidence` flag
- `docs/designs/current/DESIGN-koto-engine.md` (lines 184, 254, 276, 579, 606) -- references `koto transition`
- `docs/designs/current/DESIGN-koto-agent-integration.md` (lines 92, 172, 399) -- references `koto transition`
- `docs/designs/current/DESIGN-koto-installation.md` (line 65) -- references `koto transition`
- `plugins/koto-skills/skills/hello-koto/SKILL.md` (line 82) -- instructs agent to run `koto transition eternal`
- `plugins/koto-skills/.cursor/rules/koto.mdc` (lines 90, 124) -- references `koto transition`

## Implications

1. **The `advanced` field has two different meanings depending on the code path**, and neither is documented. This is the contract gap the exploration predicted. A caller spec must clarify: for `--to`, `advanced` means "transition happened"; for the loop path, `advanced` means "at least one auto-advancement occurred." Or the semantics should be unified.

2. **`--to` doesn't chain auto-advancement**, which is a design choice but could surprise callers. If a directed transition lands on a passthrough state, the agent must call `koto next` again. This should be explicitly documented.

3. **`--to` skips gate evaluation on the target state** (line 1342: empty gate_results map). This is intentional (the doc says "skipping gate evaluation") but means directed transitions can land on states whose gates would otherwise block. This is a separate contract question worth noting.

4. **Stale docs are a real problem**. The custom-skill-authoring guide and several design docs tell agents to use `koto transition`, which doesn't exist. Any agent following those instructions would fail.

## Surprises

1. **`koto transition` and `koto submit` don't exist at all.** The exploration scope document lists them as "adjacent commands" but they were removed in the unified redesign. The functionality lives as flags on `koto next`.

2. **`--to` hard-codes `advanced: true`** rather than computing it. This makes sense (a directed transition is an advancement) but creates a semantic split with the loop path where `advanced` tracks auto-advancement specifically.

3. **`--to` uses `dispatch_next()` (the pure classifier) while the other paths use `advance_until_stop()` (the loop engine).** These are completely different code paths that happen to produce the same output types. The dispatcher in `src/cli/next.rs` was written for a pre-loop world; the loop engine in `src/engine/advance.rs` supersedes it for the normal path. The `--to` path is the only remaining caller of `dispatch_next()` in production.

4. **The volume of stale `koto transition` references** is larger than expected -- over a dozen files including active guides.

## Open Questions

1. Should `--to` trigger auto-advancement after landing on the target state? Currently it doesn't, but if it landed on a passthrough state, the caller would need an extra `koto next` call.

2. Should the `advanced` field have a single documented semantic, or should the spec explicitly note the per-path difference?

3. Is the gate-skipping behavior of `--to` intentional and permanent, or a simplification that should be revisited?

4. Should the stale `koto transition` references be cleaned up as part of this work or tracked separately?

## Summary

`koto transition` and `koto submit` don't exist as standalone commands -- they were folded into `koto next --to` and `koto next --with-data` during the unified redesign. The critical contract gap is that `--to` bypasses the auto-advancement loop entirely and hard-codes `advanced: true`, while `--with-data` and bare `koto next` run the full advancement loop where `advanced` reflects actual auto-advancement -- giving `advanced` two different meanings depending on the code path. Over a dozen documentation files still reference the removed `koto transition` command, which would cause agent failures if followed.
