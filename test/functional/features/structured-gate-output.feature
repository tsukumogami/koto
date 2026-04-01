Feature: Structured gate output in blocking_conditions and routing

  # Scenario 13: gate_blocked response includes structured output
  Scenario: Failing command gate returns structured output in blocking_conditions
    Given a clean koto environment
    And the template "structured-gates" exists
    And I run "koto init test-wf --template .koto/templates/structured-gates.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "gate_blocked"
    And the JSON output field "state" equals "check"
    And the JSON output has field "blocking_conditions"
    And the JSON output field "blocking_conditions.0.name" equals "ci_check"
    And the JSON output field "blocking_conditions.0.type" equals "command"
    And the JSON output field "blocking_conditions.0.status" equals "failed"
    And the JSON output field "blocking_conditions.0.output.exit_code" equals 1
    And the JSON output field "blocking_conditions.0.output.error" equals ""

  # Scenario 14: gate passes and auto-advances based on gates.* when clause
  Scenario: Gate passes and auto-advances via gates.* routing
    Given a clean koto environment
    And the template "structured-routing" exists
    And I run "koto init test-wf --template .koto/templates/structured-routing.md"
    And the file "wip/flag.txt" contains "present"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "pass"
    And the JSON output field "advanced" equals "true"

  # Scenario 15: gate fails and routes to different state based on gates.* exit_code
  Scenario: Gate fails and routes to fix state via gates.* routing
    Given a clean koto environment
    And the template "structured-routing" exists
    And I run "koto init test-wf --template .koto/templates/structured-routing.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "fix"
    And the JSON output field "advanced" equals "true"
