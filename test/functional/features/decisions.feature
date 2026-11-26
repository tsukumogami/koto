Feature: Decision capture

  Scenario: Record and list decisions
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init test-wf --template .koto/templates/decisions.md"
    When I run:
      """
      koto decisions record test-wf --with-data '{"choice": "A", "rationale": "because"}'
      """
    Then the exit code is 0
    And the JSON output field "decisions_recorded" equals 1
    When I run:
      """
      koto decisions record test-wf --with-data '{"choice": "B", "rationale": "also"}'
      """
    Then the exit code is 0
    And the JSON output field "decisions_recorded" equals 2
    When I run "koto decisions list test-wf"
    Then the exit code is 0
    And the JSON output field "decisions.count" equals 2

  Scenario: Invalid decision schema rejected
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init test-wf --template .koto/templates/decisions.md"
    When I run:
      """
      koto decisions record test-wf --with-data '{"not_choice": "A"}'
      """
    Then the exit code is 2
    And the output contains "missing required field"

  Scenario: A person's answer needs the question-prompt evidence
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init raiser --template .koto/templates/decisions.md"
    And I run "koto init answerer --template .koto/templates/decisions.md"
    And I run "koto decisions record raiser --kind escalate --outcome escalated --subject question=q --subject text=t --subject recipient=person:owner --by session"
    When I run "koto decisions answer raiser 000000000001 --in answerer --by person:owner --text yes"
    Then the exit code is 2
    And the output contains "a person's answer needs the question-prompt evidence"
    When I run "koto decisions answer raiser 000000000001 --in answerer --by person:owner --prompt h/toolu_x/1"
    Then the exit code is 2
    And the output contains "prompt_unverified"

  Scenario: Question content must hash to the escalate's packet
    Given a clean koto environment
    And the template "decisions" exists
    And I run "koto init raiser --template .koto/templates/decisions.md"
    When I run "koto decisions record raiser --kind escalate --outcome escalated --subject question=q --subject text=t --subject recipient=person:owner --subject packet=sha256:ABC --by session"
    Then the exit code is 2
    When I run "koto decisions record raiser --kind escalate --outcome escalated --subject question=q --subject text=t --subject recipient=person:owner --subject packet=sha256:0000000000000000000000000000000000000000000000000000000000000000 --by session"
    Then the exit code is 0
    When I run "koto decisions content write raiser 000000000001 --file .koto/templates/decisions.md"
    Then the exit code is 2
    And the output contains "packet"
