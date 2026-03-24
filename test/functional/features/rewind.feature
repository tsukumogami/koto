Feature: Rewind

  Scenario: Rewind returns to previous state
    Given a clean koto environment
    And the template "multi-state" exists
    And I run "koto init test-wf --template .koto/templates/multi-state.md"
    And I run:
      """
      koto next test-wf --with-data '{"route": "setup"}'
      """
    When I run "koto rewind test-wf"
    Then the exit code is 0
    And the JSON output field "state" equals "entry"

  Scenario: Decisions cleared after rewind
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init test-wf --template .koto/templates/decisions.md"
    And I run:
      """
      koto decisions record test-wf --with-data '{"choice": "A", "rationale": "because"}'
      """
    And I run:
      """
      koto decisions record test-wf --with-data '{"choice": "B", "rationale": "also"}'
      """
    And I run:
      """
      koto next test-wf --with-data '{"status": "completed"}'
      """
    And I run "koto rewind test-wf"
    When I run "koto decisions list test-wf"
    Then the exit code is 0
    And the JSON output field "decisions.count" equals 0

  Scenario: Rewind at initial state fails
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init test-wf --template .koto/templates/decisions.md"
    When I run "koto rewind test-wf"
    Then the exit code is not 0
    And the output contains "initial state"
