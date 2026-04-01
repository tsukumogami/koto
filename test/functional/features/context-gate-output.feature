Feature: Context-exists gate blocked response with structured output

  # Scenario 18: context-exists gate blocked response includes structured output
  Scenario: Missing context key produces structured output in blocking_conditions
    Given a clean koto environment
    And the template "context-gate" exists
    And I run "koto init test-wf --template .koto/templates/context-gate.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "gate_blocked"
    And the JSON output field "state" equals "check"
    And the JSON output has field "blocking_conditions"
    And the JSON output field "blocking_conditions.0.name" equals "ctx_check"
    And the JSON output field "blocking_conditions.0.type" equals "context-exists"
    And the JSON output field "blocking_conditions.0.status" equals "failed"
    And the JSON output field "blocking_conditions.0.output.exists" equals "false"
    And the JSON output field "blocking_conditions.0.output.error" equals ""
