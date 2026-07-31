# Scrutiny: Justification

Issue 5 -- fix(cli): diagnose stale template_source_dir in koto init's
already-exists error. Commit reviewed: e35f42b.

## Deviations claimed and evaluation

### 1. `SpawnErrorKind::Collision` branch not covered by an integration test

**Claim**: the design's own AC ("Test confirming both paths produce the same
staleness clause for the same underlying condition") is satisfied via a unit
test of the shared `stale_template_source_dir_clause` function (called twice,
asserting identical output) plus a code comment at the Collision-handler call
site, rather than an integration test that forces the actual race.

**Evaluation**: this is a real constraint, not a shortcut. Tracing the call
graph: `handle_init`'s pre-check (`backend.exists(name)`) runs first and
unconditionally short-circuits with `exit_with_error` (which calls
`std::process::exit`) whenever the name already exists. The only way
`init_child_from_parent` reaches its own `init_state_file` call with a name
that collides is if another writer creates the session in the narrow window
*between* the pre-check's `exists()` read and the atomic rename inside
`init_state_file` -- a genuine TOCTOU race, not something a single
in-process test can force deterministically without either (a) threading
with real synchronization points inside production code (none exist -- I
verified via `grep -rn "race\|test_hook\|inject" src/cli/mod.rs
src/cli/init_child.rs`, no hooks), or (b) two real OS processes racing each
other, which would be flaky by nature. The existing
`collision_maps_to_spawn_error_kind_collision` test
(`src/cli/init_child.rs`) sidesteps this entirely by calling
`init_child_from_parent` directly, twice, bypassing `handle_init`'s pre-check
altogether -- it tests `init_child_from_parent`'s own Collision detection,
not `handle_init`'s message-formatting branch. Given that constraint, proving
the two call sites produce byte-identical clauses via "read the code, they
call the same function" plus a unit test of that function's determinism is
the correct-strength verification, not a corner cut. The alternative (skip
verification of the Collision branch entirely) was explicitly considered and
rejected in the analysis decisions.

One caveat: this means the Collision-handler branch's *integration*-level
behavior (does `exit_with_error` actually get called with the right JSON
shape from that specific match arm) is unverified end-to-end -- only the
shared clause-building logic is unit-tested, and the branch's surrounding
`exit_with_error(serde_json::json!(...))` call is structurally identical to
the pre-check's (same macro, same two keys), reducing residual risk further.
Advisory, not blocking: the residual risk is low given the structural
symmetry, but a reviewer with lower risk tolerance could reasonably ask for
this to be called out explicitly in the PR description.

### 2. `handle_init_inline` left unchanged

**Claim**: only `handle_init`'s two collision paths (pre-check ~1682,
`SpawnErrorKind::Collision` ~1707) are in scope; `handle_init_inline`'s
separate pre-check (`--from-stdin` path) is untouched.

**Evaluation**: verified against both the plan outline text (`Files:
src/cli/mod.rs`, "Update both `koto init` collision paths (the pre-check at
`~line 1682` and the `SpawnErrorKind::Collision` handler at `~line 1707`)")
and the design doc's "Implicit Decision" section, which frames the scope as
exactly these two named line ranges and never mentions
`handle_init_inline`. This is a faithful scope reading, not an
undisclosed reduction -- and it's a moot point in practice besides: per
`init_child.rs`, the `--from-stdin` inline path never populates
`template_source_dir` in the header at all (there is no source file path to
record), so even if `handle_init_inline`'s collision path called the new
helper, it would always get `None` back and never produce a clause. Extending
it would have been dead code for the one purpose this issue serves.

## Verdict

`blocking_count: 0`, `advisory_count: 1` (the Collision-handler's
`exit_with_error` call site itself is not integration-tested end-to-end,
only its shared clause-building dependency; low residual risk given
structural symmetry with the pre-check, but worth naming explicitly in the
PR body rather than leaving implicit).
