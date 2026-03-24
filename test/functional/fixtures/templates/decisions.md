---
name: decisions
version: "1.0"
description: Tests decision recording and listing
initial_state: work

states:
  work:
    accepts:
      status:
        type: enum
        values: [completed]
        required: true
    transitions:
      - target: done
  done:
    terminal: true
---

## work

Do some work and record decisions along the way.

## done

Decision test complete.
