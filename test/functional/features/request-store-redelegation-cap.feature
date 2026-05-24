Feature: koto request-store redelegation cap configuration surface

  PRD R29's redelegation cap is resolved through a 5-level cascade
  (CLI flag > env-var > project > user > built-in default). This feature
  exercises the externally observable surface: the koto next
  --redelegation-cap flag parses, koto config get exposes the resolved
  value, and the built-in default is observable from a fresh workspace.

  Scope notes:
  1. The cap-exceeded exit code (RedelegationCapExceeded → 75) fires
     inside the substrate recovery path (claim.rs / RecoveryAction::
     Abandon) and is not currently reachable through a single CLI
     invocation. Coverage for the typed-error exit code lives in the
     Rust unit tests
     (src/engine/types.rs::engine_error_redelegation_cap_exceeded_exit_code_is_75).
  2. `koto config set request_store.redelegation_cap` is not yet wired
     through the TOML writer (set_value_in_toml in src/config/mod.rs
     handles only `session.*` keys), so config-mutation scenarios are
     not included here. See team-lead handoff notes for both gaps.

  Scenario: Built-in default is 3
    Given a clean koto environment
    When I run "koto config get request_store.redelegation_cap"
    Then the exit code is 0
    And the output contains "3"

  Scenario: --redelegation-cap flag is accepted on koto next (default cap)
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init wf --template .koto/templates/decisions.md"
    When I run "koto next wf --redelegation-cap 5"
    Then the exit code is 0
    And the JSON output has field "action"

  Scenario: --redelegation-cap zero is accepted by koto next
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init wf --template .koto/templates/decisions.md"
    When I run "koto next wf --redelegation-cap 0"
    Then the exit code is 0
    And the JSON output has field "action"

  Scenario: --redelegation-cap accepts a large value without error
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init wf --template .koto/templates/decisions.md"
    When I run "koto next wf --redelegation-cap 100"
    Then the exit code is 0
    And the JSON output has field "action"

  Scenario: koto config list surfaces the request_store table
    Given a clean koto environment
    When I run "koto config list --json"
    Then the exit code is 0
    And the output contains "request_store"
    And the output contains "redelegation_cap"
