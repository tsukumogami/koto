---
name: ci-wait-stale-keys
version: "1.0"
initial_state: review
states:
  review:
    clear_on_entry: [verdict]
    accepts:
      outcome:
        type: enum
        values: [pass, retry]
        required: true
    gates:
      verdict:
        type: context-exists
        key: verdict
    transitions:
      - target: ci
        when:
          gates.verdict.exists: true
          outcome: pass
      - target: fix
        when:
          outcome: retry
  fix:
    accepts:
      fixed:
        type: enum
        values: ["yes"]
        required: true
    transitions:
      - target: review
        when:
          fixed: "yes"
  ci:
    gates:
      checks:
        type: command
        command: ./ci-status.sh
        poll:
          interval_secs: 1
          timeout_secs: 600
    transitions:
      - target: done
        when:
          gates.checks.exit_code: 0
  done:
    terminal: true
---

## review

Review the work and record a verdict.

## fix

Fix what the review found.

## ci

Wait for the checks.

## done

Done.
