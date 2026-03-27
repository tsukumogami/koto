Feature: Variable substitution

  Scenario: Variable substituted in gate command
    Given a clean koto environment
    And the template "var-substitution" exists
    And I run "koto init test-wf --template .koto/templates/var-substitution.md --var MY_VAR=expected_value"
    And the file "wip/expected_value.txt" contains "present"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "state" equals "done"
    And the JSON output field "advanced" equals "true"

  Scenario: Missing required variable rejected
    Given a clean koto environment
    And the template "var-substitution" exists
    When I run "koto init test-wf --template .koto/templates/var-substitution.md"
    Then the exit code is not 0
    And the output contains "missing required variable"
