---
complexity: simple
complexity_rationale: "Pure documentation: write a format guide and two example template files with no code changes or test requirements"
---

## Goal

Create the reference material that agents read during template authoring. This includes a condensed template format guide (~200-250 lines) and two graded example templates at increasing complexity. These files are self-contained artifacts with no runtime dependencies.

## Context

Design: `docs/designs/DESIGN-koto-template-authoring-skill.md`

The koto-author skill teaches agents to write valid koto templates. Before the skill's own template or SKILL.md can be written, agents need reference material to learn from. This issue delivers that foundation: a format guide organized by three conceptual layers (structure, evidence routing, advanced features) and two example templates that demonstrate those layers in practice.

The existing `hello-koto` template (`plugins/koto-skills/skills/hello-koto/hello-koto.md`) serves as the simple example and should be referenced rather than duplicated. The `docs/guides/custom-skill-authoring.md` guide covers SKILL.md conventions and should be referenced from the format guide where relevant.

## Acceptance Criteria

- [ ] `plugins/koto-skills/skills/koto-author/references/template-format.md` exists with ~200-250 lines covering:
  - Layer 1: Structure (YAML frontmatter schema, states, transitions, terminal states, variables)
  - Layer 2: Evidence routing (`accepts`/`when` blocks, mutual exclusivity constraint, enum types)
  - Layer 3: Advanced features (gate types including `context-exists` and `command`, self-loops, split topology, integration tags)
  - A security note warning against variable interpolation in command gate strings, recommending `context-exists` gates over command gates when checking user-supplied paths
  - Minimal YAML snippet in each section
  - Reference to `hello-koto` as the simple example
  - Reference to `docs/guides/custom-skill-authoring.md` for SKILL.md conventions
- [ ] `plugins/koto-skills/skills/koto-author/references/examples/evidence-routing-workflow.md` exists as a valid koto template demonstrating:
  - Multiple states with `accepts`/`when` evidence routing
  - Enum-typed evidence fields
  - The mutual exclusivity constraint in practice
- [ ] `plugins/koto-skills/skills/koto-author/references/examples/complex-workflow.md` exists as a valid koto template demonstrating:
  - Command gates or context-exists gates
  - At least one self-loop transition
  - Split topology (a state with multiple outbound transitions)
  - Variables with interpolation in directive bodies
- [ ] Both example templates pass `koto template compile`
- [ ] No content duplicated from `hello-koto` -- reference it by path instead

## Validation

```bash
#!/usr/bin/env bash
set -euo pipefail

SKILL_DIR="plugins/koto-skills/skills/koto-author"

# Format guide exists and is in the expected size range
guide="${SKILL_DIR}/references/template-format.md"
if [[ ! -f "$guide" ]]; then
  echo "FAIL: format guide not found at $guide"
  exit 1
fi
line_count=$(wc -l < "$guide")
if (( line_count < 150 || line_count > 350 )); then
  echo "FAIL: format guide is $line_count lines (expected ~200-250, tolerance 150-350)"
  exit 1
fi
echo "PASS: format guide exists ($line_count lines)"

# Security note present
if ! grep -qi "command.*gate\|variable.*interpolation\|shell.*injection\|context-exists.*over.*command" "$guide"; then
  echo "FAIL: format guide missing security note about command gate variable interpolation"
  exit 1
fi
echo "PASS: security note present"

# Evidence routing example exists and compiles
er_example="${SKILL_DIR}/references/examples/evidence-routing-workflow.md"
if [[ ! -f "$er_example" ]]; then
  echo "FAIL: evidence-routing example not found at $er_example"
  exit 1
fi
if ! koto template compile "$er_example" 2>&1; then
  echo "FAIL: evidence-routing example does not compile"
  exit 1
fi
echo "PASS: evidence-routing example compiles"

# Complex example exists and compiles
cx_example="${SKILL_DIR}/references/examples/complex-workflow.md"
if [[ ! -f "$cx_example" ]]; then
  echo "FAIL: complex example not found at $cx_example"
  exit 1
fi
if ! koto template compile "$cx_example" 2>&1; then
  echo "FAIL: complex example does not compile"
  exit 1
fi
echo "PASS: complex example compiles"

# hello-koto referenced, not duplicated
if grep -rq "hello-koto" "$guide"; then
  echo "PASS: format guide references hello-koto"
else
  echo "FAIL: format guide should reference hello-koto as the simple example"
  exit 1
fi

echo "All checks passed"
```

## Dependencies

None.

## Downstream Dependencies

- <<ISSUE:2>> "feat(koto-author): create 8-state koto template for authoring workflow" -- needs the format guide and examples to inform directive content in each state
