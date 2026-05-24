Feature: koto next rejects reserved request-store audit-event kinds

  Per Decision 6 in DESIGN-koto-request-store, `--with-data` payloads
  carrying a `kind` discriminator that collides with the request-store
  audit family are rejected at parse time before any disk write. The
  reservation covers four literal names (ChildDispatched,
  ChildRedelegated, RequesterWoken, RequesterRespawn) and any kind
  starting with the `request_store.` prefix.

  Background:
    Given a clean koto environment
    And the template "audit-kind" exists
    And I run "koto init wf --template .koto/templates/audit-kind.md"

  Scenario: ChildDispatched is rejected as a reserved literal
    When I run:
      """
      koto next wf --with-data '{"kind": "ChildDispatched"}'
      """
    Then the exit code is 2
    And the JSON output field "error.code" equals "invalid_submission"
    And the output contains "reserved audit-event kind"
    And the output contains "ChildDispatched"

  Scenario: ChildRedelegated is rejected as a reserved literal
    When I run:
      """
      koto next wf --with-data '{"kind": "ChildRedelegated"}'
      """
    Then the exit code is 2
    And the JSON output field "error.code" equals "invalid_submission"
    And the output contains "ChildRedelegated"

  Scenario: RequesterWoken is rejected as a reserved literal
    When I run:
      """
      koto next wf --with-data '{"kind": "RequesterWoken"}'
      """
    Then the exit code is 2
    And the JSON output field "error.code" equals "invalid_submission"
    And the output contains "RequesterWoken"

  Scenario: RequesterRespawn is rejected as a reserved literal
    When I run:
      """
      koto next wf --with-data '{"kind": "RequesterRespawn"}'
      """
    Then the exit code is 2
    And the JSON output field "error.code" equals "invalid_submission"
    And the output contains "RequesterRespawn"

  Scenario: Any kind under the request_store. prefix is rejected
    When I run:
      """
      koto next wf --with-data '{"kind": "request_store.something"}'
      """
    Then the exit code is 2
    And the JSON output field "error.code" equals "invalid_submission"
    And the output contains "request_store."

  Scenario: Nested fields.kind reserved namespace is rejected
    When I run:
      """
      koto next wf --with-data '{"kind": "request_store.bar.baz"}'
      """
    Then the exit code is 2
    And the JSON output field "error.code" equals "invalid_submission"
    And the output contains "request_store.bar.baz"

  Scenario: Reservation is case-sensitive (lowercase variant accepted)
    When I run:
      """
      koto next wf --with-data '{"kind": "childdispatched"}'
      """
    Then the exit code is 0
    And the JSON output field "action" equals "done"

  Scenario: Non-reserved kind passes the reservation check
    When I run:
      """
      koto next wf --with-data '{"kind": "myapp.review"}'
      """
    Then the exit code is 0
    And the JSON output field "action" equals "done"
