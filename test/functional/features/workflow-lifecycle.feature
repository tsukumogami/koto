Feature: Workflow lifecycle

  Scenario: Init creates state file
    Given a clean koto environment
    And the template "hello-koto" exists
    When I run "koto init hello --template .koto/templates/hello-koto.md --var SPIRIT_NAME=test"
    Then the exit code is 0
    And the state file for "hello" exists
    And the JSON output field "state" equals "awakening"

  Scenario: Next returns directive when gate fails
    Given a clean koto environment
    And the template "hello-koto" exists
    And I run "koto init hello --template .koto/templates/hello-koto.md --var SPIRIT_NAME=test"
    When I run "koto next hello"
    Then the exit code is 0
    And the JSON output has field "directive"
    And the JSON output field "action" equals "execute"
    And the JSON output field "state" equals "awakening"

  Scenario: Next advances when gate passes
    Given a clean koto environment
    And the template "hello-koto" exists
    And I run "koto init hello --template .koto/templates/hello-koto.md --var SPIRIT_NAME=test"
    And the file "wip/spirit-greeting.txt" contains "hello world"
    When I run "koto next hello"
    Then the exit code is 0
    And the JSON output field "action" equals "done"
    And the JSON output field "state" equals "eternal"
    And the JSON output field "advanced" equals "true"

  Scenario: Duplicate init rejected
    Given a clean koto environment
    And the template "hello-koto" exists
    And I run "koto init hello --template .koto/templates/hello-koto.md --var SPIRIT_NAME=test"
    When I run "koto init hello --template .koto/templates/hello-koto.md --var SPIRIT_NAME=test"
    Then the exit code is not 0
    And the output contains "already exists"
