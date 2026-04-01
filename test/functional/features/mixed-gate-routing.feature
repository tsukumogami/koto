Feature: Gate output and agent evidence coexist in when clause matching

  # Scenario 16: gate output and agent evidence coexist in when clause matching
  Scenario: Gate passes and agent evidence match the combined when clause
    Given a clean koto environment
    And the template "mixed-routing" exists
    And I run "koto init test-wf --template .koto/templates/mixed-routing.md"
    When I run:
      """
      koto next test-wf --with-data '{"decision": "approve"}'
      """
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "approved"
    And the JSON output field "advanced" equals "true"

  Scenario: Agent evidence alone without matching gate output does not advance
    Given a clean koto environment
    And the template "mixed-routing" exists
    And I run "koto init test-wf --template .koto/templates/mixed-routing.md"
    When I run:
      """
      koto next test-wf --with-data '{"decision": "reject"}'
      """
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "rejected"
    And the JSON output field "advanced" equals "true"
