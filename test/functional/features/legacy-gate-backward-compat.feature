Feature: Legacy gate backward compatibility

  # Templates with gates that have no gates.* when-clause references are
  # "legacy mode" templates. koto init accepts them with a warning.
  # koto template compile rejects them unless --allow-legacy-gates is passed.

  Scenario: koto init with a legacy-gate template exits 0 and emits warning
    Given a clean koto environment
    And the template "legacy-gates" exists
    When I run "koto init test-wf --template .koto/templates/legacy-gates.md"
    Then the exit code is 0
    And the error output contains "legacy behavior"

  Scenario: koto template compile on a legacy-gate template without flag exits nonzero
    Given a clean koto environment
    And the template "legacy-gates" exists
    When I run "koto template compile .koto/templates/legacy-gates.md"
    Then the exit code is not 0
    And the output contains "gates."
    And the output contains "--allow-legacy-gates"

  Scenario: koto template compile --allow-legacy-gates exits 0
    Given a clean koto environment
    And the template "legacy-gates" exists
    When I run "koto template compile --allow-legacy-gates .koto/templates/legacy-gates.md"
    Then the exit code is 0

  Scenario: Legacy state gate passes and workflow advances
    Given a clean koto environment
    And the template "legacy-gates" exists
    And I run "koto init test-wf --template .koto/templates/legacy-gates.md"
    When I run "koto next test-wf"
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "complete"
    And the JSON output field "advanced" is true
