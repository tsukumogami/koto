# Lead: What's the migration path from the current wip/ model?

## Findings

### Scale of the problem

- **Shirabe plugin**: 70 files reference `wip/`, concentrated in 5 work-on phases
  (3-25 refs per phase) and plan phases. Hardcoded paths: `wip/issue_<N>_baseline.md`,
  `wip/issue_<N>_plan.md`, etc.
- **Tsukumogami plugin**: 83 files reference `wip/`, including implement-doc phases,
  state management skill, and legacy skills.
- **Koto CI**: validate.yml checks wip/ is empty before merge (lines 20-27).
- **CLAUDE.md**: documents wip/ conventions, cleanup rules, naming patterns.

### Migration strategies evaluated

**Big-bang**: update all 150+ files at once. Clean break, no dual-system complexity.
But high coordination burden — all skills must release simultaneously. Testing burden
is large.

**Gradual**: new skills use koto session API, old skills keep wip/. Risk: two systems
coexist indefinitely. Agent behavior is inconsistent across skills. Resume logic gets
complicated (check both locations).

**Compatibility layer**: koto session commands write to wip/ when in git mode, to
~/.koto/sessions/ in local mode. Skills call `koto session dir` to get the path,
then use it with file tools. In git mode, the returned path IS `wip/`. Minimal skill
changes — just replace hardcoded `wip/` with a variable resolved from koto.

**Symlink bridge**: `wip/` becomes a symlink to the koto session directory at init
time. Zero skill changes needed. But fragile (platform-dependent, git doesn't track
symlinks well, confuses some tools).

### Recommended approach

The compatibility layer is the strongest option. Skills replace hardcoded `wip/`
paths with a call to `koto session dir` (or an environment variable that koto sets).
The returned path depends on the backend config. In git mode, it's literally `wip/`.
In local mode, it's `~/.koto/sessions/<id>/`. Skills don't care which.

This can be done gradually: update one skill at a time. Skills that haven't been
updated still work because `wip/` is valid in git mode (the default until users
opt into local mode).

### In-flight workflows

Workflows started with wip/ can't automatically resume through the new system.
Options: (a) complete them with the old system, (b) provide a `koto session import`
command that copies wip/ artifacts into a new session directory.

## Implications

Migration is feasible without a big-bang. The compatibility layer lets skills
migrate gradually while preserving backward compatibility. The key abstraction
is a single function/command that resolves "where does this session's state live?"
