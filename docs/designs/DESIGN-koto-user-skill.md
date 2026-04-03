---
status: Proposed
upstream: docs/prds/PRD-koto-user-skill.md
problem: |
  The koto-skills plugin has no skill covering the agent runtime loop. koto-author
  covers template authoring, but an agent running a koto-backed workflow has no
  installable guidance for init, next-dispatch, evidence submission, override flow,
  or rewind. Separately, koto-author has accumulated four documentation gaps since
  structured gate output shipped, and two phantom CLI references that send agents
  to commands that don't exist. A root AGENTS.md for the koto repo is also absent,
  leaving Claude Code sessions without basic orientation.
decision: |
  Create a koto-user skill in the existing koto-skills plugin with a SKILL.md
  covering the full runtime loop and three reference files (command-reference.md,
  response-shapes.md, error-handling.md). Migrate the existing plugins/koto-skills/
  AGENTS.md content into these reference files and delete the original. Update
  koto-author in place by extending template-format.md and the existing example
  files. Create AGENTS.md at the koto repo root for session orientation.
rationale: |
  Placing koto-user in the existing plugin avoids a split install step with no
  benefit. Reference files linked from SKILL.md are the correct mechanism for
  deep skill content -- not AGENTS.md, which Claude Code auto-loads by directory
  rather than on demand. Updating koto-author in existing files preserves its
  structure. The root AGENTS.md solves the directory-scoping problem that makes
  the current plugin-buried AGENTS.md invisible to most sessions.
---

# DESIGN: koto-user skill and koto-skills plugin update

## Status

Proposed

## Context and Problem Statement

The `koto-skills` plugin provides Claude Code skills for agents working with koto
workflows, but it only covers one of two agent personas. An agent *authoring* a
template can install `koto-author` and get structured guidance. An agent *running*
a workflow has nothing: no guidance on interpreting `koto next` output, dispatching
on `action` values, submitting evidence, handling blocked gates, or rewinding states.

This gap compounded when koto shipped structured gate output across PRs #120-#125.
`koto-author` was not updated, so it now documents a legacy gate pattern that fails
`koto template compile` in strict mode, omits the override mechanism entirely, and
references two commands (`koto status`, `koto query`) that don't exist in the CLI.

Three implementation problems need solving:

1. **New skill directory**: `plugins/koto-skills/skills/koto-user/` must be wired
   into `plugin.json` and contain a `SKILL.md` plus three reference files with
   precise content boundaries.

2. **koto-author in-place updates**: Four content gaps across `template-format.md`
   and the example files must be filled without restructuring the skill.

3. **Root orientation file**: `AGENTS.md` at the koto repo root is the only file
   Claude Code reliably auto-loads for any session in this directory. The existing
   `plugins/koto-skills/AGENTS.md` is scoped to the plugin directory and invisible
   to most sessions; it must be deleted after its content migrates to koto-user's
   reference files.

## Decision Drivers

- **Content accuracy**: all CLI commands, flags, and response schemas documented
  must match the current Rust source (`src/cli/`, `src/engine/`, `src/gate/`)
- **Navigation cost**: agents should reach what they need in one or two file reads,
  not by chasing a chain of references
- **Bounded scope**: root AGENTS.md is orientation only (≤80 lines); depth lives
  in skill reference files
- **No plugin split**: koto-user and koto-author stay in the same plugin to share
  the install step
- **koto-author structure preserved**: updates extend existing sections rather than
  adding new reference files

