Feature: Workflow name validation

  Scenario: Valid name accepted
    Given a clean koto environment
    And the template "hello-koto" exists
    When I run "koto init my-workflow --template .koto/templates/hello-koto.md --var SPIRIT_NAME=x"
    Then the exit code is 0

  Scenario: Path traversal rejected
    Given a clean koto environment
    And the template "hello-koto" exists
    When I run "koto init ../escape --template .koto/templates/hello-koto.md --var SPIRIT_NAME=x"
    Then the exit code is 2
    And the output contains "invalid characters"

  Scenario: Leading dot rejected
    Given a clean koto environment
    And the template "hello-koto" exists
    When I run "koto init .hidden --template .koto/templates/hello-koto.md --var SPIRIT_NAME=x"
    Then the exit code is 2
    And the output contains "invalid characters"

  Scenario: Empty name rejected
    Given a clean koto environment
    And the template "hello-koto" exists
    When I run "koto init '' --template .koto/templates/hello-koto.md --var SPIRIT_NAME=x"
    Then the exit code is not 0
