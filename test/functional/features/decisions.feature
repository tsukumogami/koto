Feature: Decision capture

  Scenario: Record and list decisions
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init test-wf --template .koto/templates/decisions.md"
    When I run:
      """
      koto decisions record test-wf --with-data '{"choice": "A", "rationale": "because"}'
      """
    Then the exit code is 0
    And the JSON output field "decisions_recorded" equals 1
    When I run:
      """
      koto decisions record test-wf --with-data '{"choice": "B", "rationale": "also"}'
      """
    Then the exit code is 0
    And the JSON output field "decisions_recorded" equals 2
    When I run "koto decisions list test-wf"
    Then the exit code is 0
    And the JSON output field "decisions.count" equals 2

  Scenario: Invalid decision schema rejected
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init test-wf --template .koto/templates/decisions.md"
    When I run:
      """
      koto decisions record test-wf --with-data '{"not_choice": "A"}'
      """
    Then the exit code is 2
    And the output contains "missing required field"
