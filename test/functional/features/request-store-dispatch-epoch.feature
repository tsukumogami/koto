Feature: koto next --dispatch-epoch fence on request-store child writes

  Per-write epoch fence (PRD R43). Every `--with-data` write against a
  child workflow whose header carries `needs_agent: true` must present
  `--dispatch-epoch <N>` where N equals the header's recorded
  `dispatch_epoch`. Strict equality — both stale and future presentations
  reject with `EpochFenceViolation` (exit 65). Parent-workflow ticks and
  non-request-store child ticks are NOT under the fence.

  The header rewrite step is used here to flip an existing
  `koto init --parent`-created child into the request-store
  dispatched-child shape. The session-start path produces headers
  without a compiled template path, which blocks `koto next` before the
  fence fires.

  Background:
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init parent --template .koto/templates/decisions.md"
    And I run "koto init child --template .koto/templates/decisions.md --parent parent"
    And the state file header for "child" sets:
      """
      needs_agent: true
      role: "reviewer"
      coordinator_of_record: "parent"
      requested_by: "parent"
      dispatch_epoch: 3
      """

  Scenario: --with-data without --dispatch-epoch on a fenced child rejects
    When I run:
      """
      koto next child --with-data '{"status": "completed"}'
      """
    Then the exit code is 65
    And the JSON output field "error.code" equals "epoch_fence_violation"
    And the JSON output field "error.expected_dispatch_epoch" equals 3
    And the JSON output field "command" equals "next"

  Scenario: Stale --dispatch-epoch rejects on a fenced child
    When I run:
      """
      koto next child --with-data '{"status": "completed"}' --dispatch-epoch 0
      """
    Then the exit code is 65
    And the JSON output field "error.code" equals "epoch_fence_violation"
    And the JSON output field "error.expected_dispatch_epoch" equals 3
    And the JSON output field "error.presented_dispatch_epoch" equals 0

  Scenario: Future --dispatch-epoch rejects on a fenced child
    When I run:
      """
      koto next child --with-data '{"status": "completed"}' --dispatch-epoch 99
      """
    Then the exit code is 65
    And the JSON output field "error.code" equals "epoch_fence_violation"
    And the JSON output field "error.presented_dispatch_epoch" equals 99

  Scenario: Matching --dispatch-epoch is accepted on a fenced child
    When I run:
      """
      koto next child --with-data '{"status": "completed"}' --dispatch-epoch 3
      """
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "done"

  Scenario: Parent-workflow tick is NOT under the fence
    When I run "koto next parent"
    Then the exit code is 0
    And the JSON output has field "action"
    And the JSON output field "state" equals "done"

  Scenario: Parent-workflow tick with --dispatch-epoch flag is harmless
    When I run "koto next parent --dispatch-epoch 99"
    Then the exit code is 0
    And the JSON output has field "action"
    And the JSON output field "state" equals "done"
