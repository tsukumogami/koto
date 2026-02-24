# Test Plan: koto-agent-integration

Generated from: docs/designs/DESIGN-koto-agent-integration.md
Issues covered: 6
Total scenarios: 14

---

## Scenario 1: Marketplace manifest has required fields
**ID**: scenario-1
**Testable after**: #35
**Commands**:
- `test -f .claude-plugin/marketplace.json`
- `jq -e '.name == "koto"' .claude-plugin/marketplace.json`
- `jq -e '.owner.name == "tsukumogami"' .claude-plugin/marketplace.json`
- `jq -e '.plugins | length > 0' .claude-plugin/marketplace.json`
- `jq -e '.plugins[0].source == "./plugins/koto-skills"' .claude-plugin/marketplace.json`
**Expected**: All jq assertions exit 0. The marketplace manifest exists at the repo root and contains the name "koto", owner "tsukumogami", and a plugins array with an entry pointing to `./plugins/koto-skills`.
**Status**: passed

---

## Scenario 2: Plugin manifest has required fields
**ID**: scenario-2
**Testable after**: #35
**Commands**:
- `test -f plugins/koto-skills/.claude-plugin/plugin.json`
- `jq -e '.name == "koto-skills"' plugins/koto-skills/.claude-plugin/plugin.json`
- `jq -e '.version == "0.1.0"' plugins/koto-skills/.claude-plugin/plugin.json`
- `jq -e '.skills | length > 0' plugins/koto-skills/.claude-plugin/plugin.json`
- `jq -e '.skills[0] | contains("hello-koto")' plugins/koto-skills/.claude-plugin/plugin.json`
**Expected**: All jq assertions exit 0. The plugin manifest has name "koto-skills", version "0.1.0", and a skills array referencing the hello-koto skill directory.
**Status**: passed

---

## Scenario 3: Stop hook fires when workflow is active
**ID**: scenario-3
**Testable after**: #35
**Commands**:
- `test -f plugins/koto-skills/hooks.json`
- `jq -e '.hooks.Stop | length > 0' plugins/koto-skills/hooks.json`
- `jq -e '.hooks.Stop[0].type == "command"' plugins/koto-skills/hooks.json`
- `koto init --template plugins/koto-skills/skills/hello-koto/hello-koto.md --name hooktest --var SPIRIT_NAME=TestSpirit`
- `HOOK_OUTPUT=$(eval "$(jq -r '.hooks.Stop[0].command' plugins/koto-skills/hooks.json)" 2>&1); echo "$HOOK_OUTPUT"`
**Expected**: The hooks.json is valid JSON with a Stop hook of type "command". After starting a workflow, running the hook command produces output containing "Active koto workflow detected".
**Status**: passed

---

## Scenario 4: Stop hook is silent when no workflow is active
**ID**: scenario-4
**Testable after**: #35
**Commands**:
- `rm -f wip/koto-hooktest.state.json`
- `HOOK_OUTPUT=$(eval "$(jq -r '.hooks.Stop[0].command' plugins/koto-skills/hooks.json)" 2>&1); test -z "$HOOK_OUTPUT"`
**Expected**: With no active workflows, the Stop hook command produces no output and does not emit any error messages. The exit code of the overall hook expression may be non-zero (grep finds no match), but no text is written to stdout or stderr.
**Status**: passed

---

## Scenario 5: hello-koto template compiles cleanly
**ID**: scenario-5
**Testable after**: #35
**Commands**:
- `koto template compile plugins/koto-skills/skills/hello-koto/hello-koto.md > /tmp/hello-koto-compiled.json`
- `jq -e '.initial_state == "awakening"' /tmp/hello-koto-compiled.json`
- `jq -e '.states.awakening.transitions == ["eternal"]' /tmp/hello-koto-compiled.json`
- `jq -e '.states.eternal.terminal == true' /tmp/hello-koto-compiled.json`
- `jq -e '.variables.SPIRIT_NAME.required == true' /tmp/hello-koto-compiled.json`
- `jq -e '.states.awakening.gates.greeting_exists.type == "command"' /tmp/hello-koto-compiled.json`
- `jq -e '.states.awakening.directive | contains("{{SPIRIT_NAME}}")' /tmp/hello-koto-compiled.json`
**Expected**: The template compiles with exit code 0. The compiled JSON has initial_state "awakening", two states (awakening with transition to eternal, eternal as terminal), a required SPIRIT_NAME variable, a command gate on awakening, and the directive contains the `{{SPIRIT_NAME}}` interpolation marker.
**Status**: passed

---

## Scenario 6: hello-koto SKILL.md has valid Agent Skills frontmatter
**ID**: scenario-6
**Testable after**: #35
**Commands**:
- `test -f plugins/koto-skills/skills/hello-koto/SKILL.md`
- `head -1 plugins/koto-skills/skills/hello-koto/SKILL.md | grep -q '^---'`
- `sed -n '2,/^---$/p' plugins/koto-skills/skills/hello-koto/SKILL.md | grep -q 'name:.*hello-koto'`
- `sed -n '2,/^---$/p' plugins/koto-skills/skills/hello-koto/SKILL.md | grep -q 'description:'`
**Expected**: The SKILL.md file exists, starts with YAML frontmatter delimiters, and the frontmatter contains a `name` field with value "hello-koto" and a `description` field.
**Status**: passed

---

## Scenario 7: End-to-end hello-koto workflow loop
**ID**: scenario-7
**Testable after**: #35
**Commands**:
- `rm -f wip/koto-hello.state.json wip/spirit-greeting.txt`
- `INIT_OUT=$(koto init --template plugins/koto-skills/skills/hello-koto/hello-koto.md --name hello --var SPIRIT_NAME=Hasami); echo "$INIT_OUT" | jq -e '.state == "awakening"'`
- `NEXT_OUT=$(koto next); echo "$NEXT_OUT" | jq -e '.directive | contains("Hasami")'`
- `mkdir -p wip && echo "Greetings from Hasami" > wip/spirit-greeting.txt`
- `TRANS_OUT=$(koto transition eternal); echo "$TRANS_OUT" | jq -e '.state == "eternal"'`
- `DONE_OUT=$(koto next); echo "$DONE_OUT" | jq -e '.action == "done"'`
- `rm -f wip/koto-hello.state.json wip/spirit-greeting.txt`
**Expected**: The full init/next/execute/transition/done loop completes. Init returns state "awakening". Next returns a directive containing "Hasami" (variable interpolation works). Transition to "eternal" succeeds after the greeting file is created (command gate passes). Next on the terminal state returns action "done".
**Status**: passed

---

## Scenario 8: Command gate blocks transition when evidence is missing
**ID**: scenario-8
**Testable after**: #35
**Commands**:
- `rm -f wip/koto-gatetest.state.json wip/spirit-greeting.txt`
- `koto init --template plugins/koto-skills/skills/hello-koto/hello-koto.md --name gatetest --var SPIRIT_NAME=GateTest`
- `koto transition eternal 2>&1; echo "EXIT=$?"`
- `rm -f wip/koto-gatetest.state.json`
**Expected**: The transition command fails with a non-zero exit code because `wip/spirit-greeting.txt` does not exist and the command gate (`test -f wip/spirit-greeting.txt`) fails. This confirms gate enforcement works correctly.
**Status**: passed

---

## Scenario 9: CI workflow file validates plugin artifacts
**ID**: scenario-9
**Testable after**: #36
**Commands**:
- `test -f .github/workflows/validate-plugins.yml`
- `grep -q 'plugins/' .github/workflows/validate-plugins.yml`
- `grep -q '.claude-plugin/' .github/workflows/validate-plugins.yml`
- `grep -q 'koto template compile' .github/workflows/validate-plugins.yml`
- `grep -q 'hook' .github/workflows/validate-plugins.yml`
- `grep -q 'plugin.json' .github/workflows/validate-plugins.yml`
- `grep -q 'marketplace.json' .github/workflows/validate-plugins.yml`
- `grep -q 'go build\|setup-go' .github/workflows/validate-plugins.yml`
**Expected**: The validate-plugins.yml workflow exists and contains: path filters for `plugins/` and `.claude-plugin/`, a template compilation step using `koto template compile`, a hook smoke test, schema validation referencing both plugin.json and marketplace.json, and a Go build step (builds koto from the PR rather than using a released version).
**Status**: pending

---

## Scenario 10: CI workflow requires no external secrets
**ID**: scenario-10
**Testable after**: #36
**Commands**:
- `grep 'secrets\.' .github/workflows/validate-plugins.yml | grep -v 'GITHUB_TOKEN' | grep -c . || echo "0"`
**Expected**: The output is "0". The validate-plugins workflow does not reference any secrets other than the automatic GITHUB_TOKEN. Template compilation, hook smoke tests, and schema validation all run without API keys.
**Status**: pending

---

## Scenario 11: Eval harness exists and targets hello-koto
**ID**: scenario-11
**Testable after**: #37
**Commands**:
- `find . -name '*eval*' \( -name '*.go' -o -name '*.sh' \) -not -path '*/.git/*' | head -5`
- The harness file found above is checked for: `grep -q 'ANTHROPIC_API_KEY' <harness>`
- The harness file found above is checked for: `grep -q 'hello.koto' <harness>`
- The harness file found above is checked for: `grep -q 'koto init\|koto.init' <harness>`
**Expected**: An eval harness file (Go test or shell script) exists that references ANTHROPIC_API_KEY, the hello-koto skill, and checks for `koto init` in model output. The harness validates that SKILL.md content produces correct koto command sequences.
**Status**: pending

---

## Scenario 12: Eval GHA workflow triggers on plugin changes
**ID**: scenario-12
**Testable after**: #37
**Commands**:
- `find .github/workflows/ -name '*.yml' -o -name '*.yaml' | xargs grep -l 'eval' | head -1`
- The workflow file found above is checked for: `grep -q 'plugins/' <workflow>`
- The workflow file found above is checked for: `grep -q 'ANTHROPIC_API_KEY' <workflow>`
**Expected**: A GHA workflow exists that references evals, triggers on changes to `plugins/`, and uses the ANTHROPIC_API_KEY secret. This confirms the eval infrastructure is wired into CI.
**Status**: pending

---

## Scenario 13: Manual test checklist covers all required areas
**ID**: scenario-13
**Testable after**: #38
**Commands**:
- `find docs/ -name '*manual*' -o -name '*MANUAL*' | head -1`
- The checklist file found above is checked for: `grep -qi 'plugin install\|marketplace' <checklist>`
- The checklist file found above is checked for: `grep -qi 'skill invocation\|hello-koto\|workflow loop' <checklist>`
- The checklist file found above is checked for: `grep -qi 'stop hook\|hook.*silent\|hook.*fail' <checklist>`
- The checklist file found above is checked for: `grep -qi 'prerequisites\|when to run' <checklist>`
**Expected**: A manual test checklist document exists under docs/ and covers: plugin installation from marketplace, skill invocation and the full workflow loop, Stop hook behavior (both active reminder and silent failure), and includes prerequisites and "when to run" guidance.
**Status**: pending

---

## Scenario 14: Full agent flow with plugin-installed skill
**ID**: scenario-14
**Testable after**: #35
**Environment**: manual
**Commands**:
- Open a fresh project directory with Claude Code (version 1.0.33+)
- `/plugin marketplace add tsukumogami/koto`
- `/plugin install koto-skills@koto`
- Verify "hello-koto" appears in the skill list
- `/hello-koto Hasami`
- Observe the agent: reads SKILL.md, runs `koto init --template <path> --name hello --var SPIRIT_NAME=Hasami`
- Observe the agent: runs `koto next`, receives directive mentioning "Hasami"
- Observe the agent: creates `wip/spirit-greeting.txt`
- Observe the agent: runs `koto transition eternal`, transition succeeds
- Observe the agent: runs `koto next`, receives `{"action":"done"}`
- Observe the agent: outputs a completion message
- Stop the Claude Code session mid-workflow (before transition) and verify the Stop hook outputs "Active koto workflow detected"
- Resume the session and verify the agent can continue the workflow
**Expected**: The full agent-driven flow works end to end through the Claude Code plugin system. The plugin installs from marketplace, the skill is discoverable, the agent follows SKILL.md instructions to call koto correctly, variable interpolation works, the command gate enforces file creation, and the workflow completes. The Stop hook fires on session stop when a workflow is active. This scenario also validates the template locality question: whether the agent can resolve the template path from the plugin directory or must copy it to a project-local path.
**Status**: skipped (manual environment required)

---
