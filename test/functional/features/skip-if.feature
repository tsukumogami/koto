Feature: skip_if auto-advance conditions

  # Scenario 1: single skip_if fires — state advances past skip_if state
  Scenario: skip_if fires and advances state
    Given a clean koto environment
    And the template "skip-if-chain" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-chain.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "advanced" equals "true"

  # Scenario 2: consecutive chaining — A→B→C in one koto next call
  Scenario: consecutive skip_if conditions chain through multiple states
    Given a clean koto environment
    And the template "skip-if-chain" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-chain.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "state" equals "c"
    And the JSON output field "advanced" equals "true"

  # Scenario 3: unmet condition blocks — no --var means skip_if doesn't fire
  Scenario: unmet vars condition blocks at evidence_required
    Given a clean koto environment
    And the template "skip-if-vars" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-vars.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "evidence_required"
    And the JSON output field "state" equals "start"

  # Scenario 4: gate-backed skip_if fires when gate passes
  Scenario: gate-backed skip_if fires when gate passes
    Given a clean koto environment
    And the template "skip-if-gate" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-gate.md"
    And the file "wip/flag.txt" contains "present"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "advanced" equals "true"
    And the JSON output field "state" equals "done"

  # Scenario 5: gate-backed skip_if does not fire when gate fails
  Scenario: gate-backed skip_if does not fire when gate fails
    Given a clean koto environment
    And the template "skip-if-gate" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-gate.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "gate_blocked"
    And the JSON output field "state" equals "check"

  # Scenario 6: skip_if bypasses accepts when vars condition is met
  Scenario: skip_if bypasses evidence requirement when var is set
    Given a clean koto environment
    And the template "skip-if-vars" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-vars.md --var SHARED_BRANCH=main"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "advanced" equals "true"
    And the JSON output field "state" equals "done"

  # Scenario 7: vars skip_if does not fire without var set
  Scenario: vars skip_if does not fire without var
    Given a clean koto environment
    And the template "skip-if-vars" exists
    And I run "koto init test-wf --template .koto/templates/skip-if-vars.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "evidence_required"
