<!-- decision:start id="skill-path-discovery" status="assumed" -->
### Decision: How skills discover the session path

**Context**

Feature 1 (local session storage) moves koto's session state from `wip/` in the git
working tree to `~/.koto/sessions/<repo-id>/<name>/`. Feature 4 (git backend) restores
`wip/` as an opt-in backend, but it depends on Feature 2 (config system for backend
selection). This creates a gap: after Feature 1 ships, skills that hardcode `wip/` paths
break, and there's no way to configure koto to use `wip/` again until Features 2 and 4
land.

The gap affects shirabe (529 `wip/` references across 85 files) and koto's own plugin
skills. These references are in skill phase definitions, agent prompts, eval fixtures,
and scripts -- they construct paths like `wip/issue_<N>_plan.md` or
`wip/research/<command>_<phase>_<role>.md` directly.

The design already specifies `koto session dir <name>` as a CLI command (Phase 3 of
implementation). The question is whether this is enough, or whether koto needs an
additional compatibility mechanism during the gap.

**Assumptions**

- Skills are updated by the same team that ships Feature 1. There's no third-party
  ecosystem of skills that would break without notice. If this changes, the env var
  escape hatch (Alternative 2) becomes more valuable.
- `koto session dir` can be implemented in Feature 1's Phase 3 (session subcommands),
  meaning it ships in the same release as the storage move. If Phase 3 slips to a
  separate release, skills break with no workaround.
- The ~150 hardcoded path references in shirabe are mechanically replaceable. They follow
  predictable patterns (`wip/<prefix>_<artifact>.md`, `wip/research/...`). If some paths
  are dynamically constructed in ways that resist find-and-replace, the migration effort
  grows.

**Chosen: `koto session dir` as the sole contract, with coordinated skill migration**

Skills call `koto session dir <name>` (or capture its output in a variable) to get the
session path. The "migration" is a skill-side change: replace hardcoded `wip/` path
construction with calls to `koto session dir`. This ships alongside Feature 1, not after
it. There is no compatibility layer, env var override, or detect-and-warn mechanism in
koto itself.

The concrete migration:
1. Feature 1 ships with all three implementation phases (including `koto session dir`).
2. Before or simultaneously, shirabe and koto-skills are updated to call
   `koto session dir` instead of hardcoding `wip/`. Skills that run shell commands use
   `SESSION_DIR=$(koto session dir "$name")` and construct paths from that variable.
   Skills that use file tools compute the path from the `koto session dir` output.
3. The two changes (koto Feature 1 + skill migration) are released together. Neither
   ships without the other.

This is the approach the design doc already implies. The `koto session dir` command exists
specifically so skills don't need to know where sessions live. The "gap" only exists if
skills aren't updated, and since they're controlled by the same team, coordinating the
release eliminates the gap entirely.

**Rationale**

The migration problem is a coordination problem, not a technical one. Every proposed
technical mechanism (env var, detect-and-warn, bundling Features 1+4) adds complexity
to solve a problem that disappears with coordinated releases.

The env var approach (Alternative 2) is the closest competitor, but it has a fundamental
flaw: it creates two ways to discover the session path (`KOTO_SESSION_DIR` and
`koto session dir`), which means skills must check for both. It also leaks an
implementation detail (the env var) into the skill contract, making it harder to remove
later. The `koto session dir` command is already the designed API surface -- adding an
env var alongside it creates redundancy.

Shipping Features 1+4 together (Alternative 3) defeats the purpose of the roadmap's
incremental sequencing. Feature 4 depends on Feature 2 (config system), so bundling them
means Feature 1 can't ship until the config system exists. This blocks the highest-value
change (removing `wip/` from git) behind lower-priority work.

Making skill migration a prerequisite (Alternative 4) is what the chosen approach does,
just stated as a constraint rather than a mechanism. The difference is framing: the
chosen approach says "coordinate the release" rather than "gate Feature 1 on skill
updates." The result is the same, but the framing keeps Feature 1's scope clean.

**Alternatives Considered**

- **KOTO_SESSION_DIR env var override**: Skills read `$KOTO_SESSION_DIR` if set, fall
  back to `wip/`. Cheap escape hatch. Rejected because it creates a parallel discovery
  mechanism that competes with `koto session dir`, adds a contract that's hard to
  deprecate, and solves a coordination problem with a technical mechanism. If someone
  sets the env var and forgets to unset it, sessions go to the wrong place silently.

- **Detect-and-warn**: koto detects `wip/` artifacts and warns the user that sessions
  moved. Rejected because it's reactive (warns after breakage, doesn't prevent it),
  requires koto to understand skill artifact patterns (coupling), and adds code that
  exists only for the transition period.

- **Ship Features 1+4 together**: Don't ship Feature 1 until the git backend exists
  so users can opt back into `wip/`. Rejected because Feature 4 depends on Feature 2
  (config system), so this bundles three features into one release. Blocks the
  highest-value change behind lower-priority work. Contradicts the roadmap's
  "each feature is independently shippable" principle.

- **Skill migration as a hard prerequisite**: Gate Feature 1's release on all skills
  being updated. This is functionally identical to the chosen approach but frames the
  skill update as a blocker rather than a coordinated release. Rejected as a separate
  alternative because the distinction is only in framing, and "coordinated release" is
  more accurate -- both changes ship together, neither blocks the other.

**Consequences**

The skill migration becomes part of Feature 1's release scope, not a follow-up. This
means:
- Feature 1's effective scope grows to include shirabe and koto-skills updates (the ~150
  path references). The koto side stays clean, but the coordinated release has more
  moving parts.
- There's no backward-compatible transition period. Old skill versions and new koto
  versions are incompatible. This is acceptable because skills and koto are co-versioned,
  but it means partial upgrades break.
- The git backend (Feature 4) becomes purely an opt-in preference, not a compatibility
  bridge. Users who want `wip/` in their repo can configure it later; it's not needed
  for migration.
- No temporary code in koto (env vars, detection logic, deprecation warnings) that needs
  removal later.
<!-- decision:end -->
