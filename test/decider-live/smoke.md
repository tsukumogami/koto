---
# The template the live decider smoke check runs its fixtures against
# (scripts/decider-live-smoke.sh). Its inputs are synthetic sentences, so
# nothing from any repository is sent to the provider. One enum question and
# one boolean question cover both of the provider's answer types.
name: decider-live-smoke
version: "1.0"
description: Synthetic questions for the live decider smoke check.
initial_state: pick
variables:
  ITEM:
    description: A synthetic sentence.
    required: false
    default: fixture
states:
  pick:
    accepts:
      color:
        type: enum
        values: [red, blue]
        required: true
        description: Which color does the text name?
        decider:
          answers:
            red: {description: "The text names the color red."}
            blue: {description: "The text names the color blue."}
          escape: {value: unclear, description: "The text names no color, or both."}
          inputs:
            - {var: ITEM, label: text}
    transitions:
      - target: check
        when:
          color: red
      - target: check
        when:
          color: blue
  check:
    accepts:
      is_even:
        type: boolean
        required: true
        description: Is the number in the text even?
        decider:
          answers:
            true:  {description: "The number written in the text is even."}
            false: {description: "The number written in the text is odd."}
          inputs:
            - {var: ITEM, label: text}
    transitions:
      - target: done
        when:
          is_even: true
      - target: done
        when:
          is_even: false
  done:
    terminal: true
---

## pick

Which color does {{ITEM}} name?

## check

Is the number in {{ITEM}} even?

## done

Done.
