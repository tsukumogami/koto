Feature: koto workspace prune reclaims terminal session trees

  Operator-facing verb that reclaims a workspace tree rooted at a terminal
  session. The terminal-state gate, the symlink-root refusal, and the
  --force / --yes escape hatches are the safety surface; --dry-run lets
  operators preview without touching the filesystem.

  Scenario: Dry-run on terminal session reports descendants and exits cleanly
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init wf --template .koto/templates/decisions.md"
    And I run:
      """
      koto next wf --with-data '{"status": "completed"}' --no-cleanup
      """
    When I run "koto workspace prune --root wf --dry-run"
    Then the exit code is 0
    And the output contains "root: wf"
    And the output contains "completed"

  Scenario: Pruning a symlinked session refuses by name
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init real --template .koto/templates/decisions.md"
    And I run:
      """
      koto next real --with-data '{"status": "completed"}' --no-cleanup
      """
    And the session directory "shadow" is a symlink to "real"
    When I run "koto workspace prune --root shadow --dry-run"
    Then the exit code is 2
    And the JSON output field "command" equals "workspace prune"
    And the output contains "symlink"

  Scenario: Pruning a non-terminal session without --force is gated
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init wip-wf --template .koto/templates/decisions.md"
    When I run "koto workspace prune --root wip-wf --dry-run"
    Then the exit code is 2
    And the JSON output field "command" equals "workspace prune"
    And the output contains "not terminal"

  Scenario: Pruning a terminal session with --yes reclaims it non-interactively
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init wf --template .koto/templates/decisions.md"
    And I run:
      """
      koto next wf --with-data '{"status": "completed"}' --no-cleanup
      """
    When I run "koto workspace prune --root wf --yes"
    Then the exit code is 0
    And the JSON output field "pruned" is true
    And the JSON output field "name" equals "wf"

  Scenario: --force --yes overrides the terminal gate on a non-terminal session
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init wip-wf --template .koto/templates/decisions.md"
    And the command stdin is "force-prune"
    When I run "koto workspace prune --root wip-wf --force --yes"
    Then the exit code is 0
    And the JSON output field "pruned" is true
    And the output contains "WARNING"
