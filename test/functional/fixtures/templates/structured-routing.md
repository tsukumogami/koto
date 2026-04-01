---
name: structured-routing
version: "1.0"
description: Routes based on gates.* exit_code in when clauses
initial_state: check

states:
  check:
    gates:
      ci_check:
        type: command
        command: "test -f wip/flag.txt"
    transitions:
      - target: pass
        when:
          gates.ci_check.exit_code: 0
      - target: fix
        when:
          gates.ci_check.exit_code: 1
  pass:
    terminal: true
  fix:
    terminal: true
---

## check

Run ci_check gate (test -f wip/flag.txt) and route based on exit code.
If wip/flag.txt exists, the gate exits 0 and routes to pass.
If wip/flag.txt is absent, the gate exits 1 and routes to fix.

## pass

Gate passed.

## fix

Gate failed, fix required.
