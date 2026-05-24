Feature: koto session start --needs-agent companion-flag validation

  The request-store authoring surface enforces a companion-flag contract at
  parse time: --needs-agent set requires --role, --template, and --inputs;
  any of those four orphaned (without --needs-agent) rejects naming
  --needs-agent as the missing flag. The contract fires BEFORE any
  filesystem write so parse-time rejection has no on-disk side effects.

  Background:
    Given a clean koto environment
    And the template "multi-state" exists
    And I run "koto init parent --template .koto/templates/multi-state.md"

  Scenario: All companion flags present succeeds
    When I run:
      """
      koto session start child --parent parent --needs-agent --role reviewer --template verdict --inputs {} --coordinator-of-record parent
      """
    Then the exit code is 0
    And the JSON output field "name" equals "child"
    And the JSON output field "parent" equals "parent"
    And the JSON output field "needs_agent" is true

  Scenario: Missing --role rejected
    When I run:
      """
      koto session start child --parent parent --needs-agent --template verdict --inputs {}
      """
    Then the exit code is not 0
    And the error output contains "--needs-agent"
    And the error output contains "--role"

  Scenario: Missing --template rejected
    When I run:
      """
      koto session start child --parent parent --needs-agent --role reviewer --inputs {}
      """
    Then the exit code is not 0
    And the error output contains "--needs-agent"
    And the error output contains "--template"

  Scenario: Missing --inputs rejected
    When I run:
      """
      koto session start child --parent parent --needs-agent --role reviewer --template verdict
      """
    Then the exit code is not 0
    And the error output contains "--needs-agent"
    And the error output contains "--inputs"

  Scenario: --role without --needs-agent rejected
    When I run "koto session start child --parent parent --role reviewer"
    Then the exit code is not 0
    And the error output contains "--needs-agent"
    And the error output contains "--role"

  Scenario: --template without --needs-agent rejected
    When I run "koto session start child --parent parent --template verdict"
    Then the exit code is not 0
    And the error output contains "--needs-agent"
    And the error output contains "--template"

  Scenario: --inputs without --needs-agent rejected
    When I run:
      """
      koto session start child --parent parent --inputs {}
      """
    Then the exit code is not 0
    And the error output contains "--needs-agent"
    And the error output contains "--inputs"

  Scenario: --coordinator-of-record without --needs-agent rejected
    When I run "koto session start child --parent parent --coordinator-of-record parent"
    Then the exit code is not 0
    And the error output contains "--needs-agent"
    And the error output contains "--coordinator-of-record"

  Scenario: Plain session start without dispatch flags succeeds
    When I run "koto session start child --parent parent"
    Then the exit code is 0
    And the JSON output field "name" equals "child"
    And the JSON output field "needs_agent" is false
