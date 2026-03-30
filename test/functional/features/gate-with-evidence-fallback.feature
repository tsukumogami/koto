Feature: Gate with evidence fallback

  Scenario: Gate passes and auto-advances
    Given a clean koto environment
    And the template "simple-gates" exists
    And I run "koto init test-wf --template .koto/templates/simple-gates.md"
    And the file "wip/check.txt" contains "present"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "state" equals "done"
    And the JSON output field "action" equals "done"
    And the JSON output field "advanced" equals "true"

  Scenario: Gate fails and evidence is required
    Given a clean koto environment
    And the template "simple-gates" exists
    And I run "koto init test-wf --template .koto/templates/simple-gates.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "state" equals "start"
    And the JSON output field "action" equals "evidence_required"
    And the JSON output has field "expects"

  Scenario: Gate fails then evidence advances
    Given a clean koto environment
    And the template "simple-gates" exists
    And I run "koto init test-wf --template .koto/templates/simple-gates.md"
    And I run "koto next test-wf"
    When I run:
      """
      koto next test-wf --with-data '{"status": "completed", "detail": "manual"}'
      """
    Then the exit code is 0
    And the JSON output field "state" equals "done"
    And the JSON output field "action" equals "done"
    And the JSON output field "advanced" equals "true"
