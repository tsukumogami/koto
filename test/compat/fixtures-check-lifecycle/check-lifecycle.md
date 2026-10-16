---
name: check-lifecycle-compat
version: "1.0"
description: A gate that passes while reporting a warning, then a state waiting on evidence
initial_state: lint
states:
  lint:
    gates:
      style:
        type: command
        command: 'sh ./style.sh'
    transitions:
      - target: work
  work:
    accepts:
      verdict:
        type: enum
        required: true
        values: [finish]
    transitions:
      - target: done
        when:
          verdict: finish
  done:
    terminal: true
---

## lint

Run the style check.

## work

Do the work, then submit a verdict.

## done

Done.
