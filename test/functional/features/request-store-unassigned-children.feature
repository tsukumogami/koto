Feature: koto next reports unassigned_children directives to coordinators

  Every `koto next` response carries an `unassigned_children` array. The
  array is empty until the discovery scan finds children whose header
  has `needs_agent: true`, no `assignment_claim`, and a
  `coordinator_of_record` that matches the workflow being ticked. When
  populated, each entry exposes the dispatch contract (role, template,
  inputs, requested_by, created_at, dispatch_epoch) so the coordinator
  can pick an agent.

  Scenario: Empty workspace produces an empty unassigned_children list
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init parent --template .koto/templates/decisions.md"
    When I run "koto next parent"
    Then the exit code is 0
    And the JSON output has field "unassigned_children"
    And the JSON output field "unassigned_children" has length 0

  Scenario: A request-store child appears in the parent's unassigned_children
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init parent --template .koto/templates/decisions.md"
    And I run:
      """
      koto session start kid --parent parent --needs-agent --role reviewer --template verdict --inputs {} --coordinator-of-record parent
      """
    When I run "koto next parent"
    Then the exit code is 0
    And the JSON output field "unassigned_children" has length 1
    And the JSON output field "unassigned_children.0.child_session_id" equals "kid"
    And the JSON output field "unassigned_children.0.role" equals "reviewer"
    And the JSON output field "unassigned_children.0.template" equals "verdict"
    And the JSON output has field "unassigned_children.0.requested_by"
    And the JSON output field "unassigned_children.0.dispatch_epoch" equals 0
    And the JSON output has field "unassigned_children.0.created_at"

  Scenario: Children whose coordinator does not match are filtered out
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init parent --template .koto/templates/decisions.md"
    And I run "koto init other --template .koto/templates/decisions.md"
    And I run:
      """
      koto session start kid --parent parent --needs-agent --role reviewer --template verdict --inputs {} --coordinator-of-record other
      """
    When I run "koto next parent"
    Then the exit code is 0
    And the JSON output field "unassigned_children" has length 0

  Scenario: A second consecutive tick deduplicates an already-seen child
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init parent --template .koto/templates/decisions.md"
    And I run:
      """
      koto session start kid --parent parent --needs-agent --role reviewer --template verdict --inputs {} --coordinator-of-record parent
      """
    And I run "koto next parent --no-cleanup"
    When I run "koto next parent --no-cleanup"
    Then the exit code is 0
    And the JSON output field "unassigned_children" has length 0

  Scenario: Plain children without needs_agent do not surface in the directive
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init parent --template .koto/templates/decisions.md"
    And I run "koto session start plain --parent parent"
    When I run "koto next parent"
    Then the exit code is 0
    And the JSON output field "unassigned_children" has length 0

  Scenario: Multiple unassigned children all appear, each with full fields
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init parent --template .koto/templates/decisions.md"
    And I run:
      """
      koto session start kid-a --parent parent --needs-agent --role reviewer --template verdict --inputs {} --coordinator-of-record parent
      """
    And I run:
      """
      koto session start kid-b --parent parent --needs-agent --role planner --template plan --inputs {} --coordinator-of-record parent
      """
    When I run "koto next parent"
    Then the exit code is 0
    And the JSON output field "unassigned_children" has length 2
