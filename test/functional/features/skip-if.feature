Feature: skip_if auto-advance conditions

  # Scenario 1: consecutive chaining — A→B→C in one koto next call
  Scenario: consecutive skip_if conditions chain through multiple states
    Given a clean koto environment
    And the template "skip-if-chain" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-chain.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "state" equals "c"
    And the JSON output field "advanced" is true

  # Scenario 2: unmet condition blocks — no --var means skip_if doesn't fire
  Scenario: unmet vars condition blocks at evidence_required
    Given a clean koto environment
    And the template "skip-if-vars" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-vars.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "evidence_required"
    And the JSON output field "state" equals "start"

  # Scenario 3: gate-backed skip_if fires when context-exists gate passes
  Scenario: gate-backed skip_if fires when gate passes
    Given a clean koto environment
    And the template "skip-if-gate" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-gate.md"
    And the file "ctx_data.txt" contains "present"
    And I run "koto context add test-wf ctx_flag --from-file ctx_data.txt"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "advanced" is true
    And the JSON output field "state" equals "done"

  # Scenario 4: gate-backed skip_if does not fire when context key is absent
  Scenario: gate-backed skip_if does not fire when gate fails
    Given a clean koto environment
    And the template "skip-if-gate" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-gate.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "gate_blocked"
    And the JSON output field "state" equals "check"

  # Scenario 5: skip_if bypasses accepts when vars condition is met
  Scenario: skip_if bypasses evidence requirement when var is set
    Given a clean koto environment
    And the template "skip-if-vars" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-vars.md --var SHARED_BRANCH=main"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "advanced" is true
    And the JSON output field "state" equals "done"

  # Scenario 6: correct conditional branch selected when skip_if fires with multi-branch transitions
  Scenario: skip_if selects correct conditional branch when multiple transitions exist
    Given a clean koto environment
    And the template "skip-if-branch" exists
    And I run "koto init branch-wf --template .koto/templates/skip-if-branch.md --var ROUTE=main"
    When I run "koto next branch-wf"
    Then the exit code is 0
    And the JSON output field "state" equals "main_track"
    And the JSON output field "advanced" is true
