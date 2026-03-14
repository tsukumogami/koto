# Crystallize Decision: migrate-koto-go-to-rust

## Chosen Type

No artifact

## Rationale

The exploration resolved all open questions in a single round and the user
provided clear, directive guidance on scope. The findings were immediately
actionable as direct updates to existing artifacts: GitHub issues #45-#49
were updated with new scopes and acceptance criteria, PLAN-unified-koto-next.md
was updated to reflect design+implementation issues and the intentional
state-advance gap. There is nothing left to document before implementation
begins — the work is ready to execute via `/work-on 45`.

A design doc was considered but ranked below No artifact because all
implementation paths were decided during exploration: single crate, clap v4,
serde_json/serde_yml, simple JSONL state, FormatVersion=1 compiled templates.
No competing approaches remain.

A plan was considered and scored well (+4) because DESIGN-unified-koto-next.md
exists and the work is broken into issues. But the plan already exists as
PLAN-unified-koto-next.md (Active). Running /plan would be redundant.

## Signal Evidence

### Signals Present (No artifact)

- **Simple enough to act directly**: #45 has detailed acceptance criteria, a
  clear crate layout, and an explicit table of what's in and out of scope.
- **One person can implement**: no coordination needed; single-author Rust CLI.
- **Exploration resolved all open questions**: workspace structure (single crate),
  dependencies (clap v4, serde, thiserror+anyhow, tempfile, wait-timeout),
  CI replacement (cargo test/fmt/clippy/audit, cargo-dist for releases), and
  scope boundaries all settled in Round 1.
- **Single round, high user confidence**: user gave clear directive guidance
  in converge phase; no second round needed.
- **Right next step is "just do it"**: `/work-on 45` is the correct next action.

### Anti-Signals Checked

- **Others need docs to build from**: not present — issues are self-contained
  with full acceptance criteria.
- **Multiple people working**: not present.
- **Significant uncertainty remains**: not present — scope is settled.

## Alternatives Considered

- **Plan**: Scored +4 (existing design doc, work broken into issues, scope
  confirmed). Ranked below No artifact because the plan already exists as
  PLAN-unified-koto-next.md (Active). No new plan is needed.
- **Design Doc**: Scored 0. Implementation path is decided; no competing
  approaches remain to evaluate.
- **PRD**: Demoted. Requirements were given as input to the exploration
  (user specified scope directly), not discovered during it.
