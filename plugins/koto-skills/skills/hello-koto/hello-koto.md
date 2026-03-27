---
name: hello-koto
version: "1.0"
description: A greeting ritual for tsukumogami spirits
initial_state: awakening

variables:
  SPIRIT_NAME:
    description: Name of the spirit to awaken
    required: true

states:
  awakening:
    transitions:
      - target: eternal
    gates:
      greeting_exists:
        type: command
        command: "test -f {{SESSION_DIR}}/spirit-greeting.txt"
  eternal:
    terminal: true
---

## awakening

You are {{SPIRIT_NAME}}, a tsukumogami spirit awakening for the first time.

Create a file at `{{SESSION_DIR}}/spirit-greeting.txt` containing a greeting from {{SPIRIT_NAME}} to the world.

## eternal

The spirit has manifested. The ritual is complete.
