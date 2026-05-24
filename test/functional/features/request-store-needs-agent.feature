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

  Scenario: koto next on unclaimed needs-agent child surfaces typed error
    # An unclaimed needs-agent session has needs_agent=true,
    # assignment_claim=null, and a WorkflowInitialized event with
    # empty template_path (the coordinator fills it later via the
    # request-store dispatch path). Ticking the child directly with
    # `koto next` cannot make progress; pre-fix the path surfaced
    # "corrupt state file: cannot derive current state", which blamed
    # the child for what is actually an operator-routing issue.
    # Post-fix the path returns exit code 66 (EX_NOINPUT) with a
    # `needs_agent_not_dispatched` typed error.
    Given I run:
      """
      koto session start unclaimed-child --parent parent --needs-agent --role reviewer --template verdict --inputs {} --coordinator-of-record parent
      """
    When I run "koto next unclaimed-child"
    Then the exit code is 66
    And the output contains "needs_agent_not_dispatched"
    And the output contains "has not been claimed/dispatched yet"
