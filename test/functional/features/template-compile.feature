Feature: Template compilation

  Scenario: Valid template compiles
    Given a clean koto environment
    And the template "hello-koto" exists
    When I run "koto template compile .koto/templates/hello-koto.md"
    Then the exit code is 0

  Scenario: Invalid template rejected
    Given a clean koto environment
    And the file ".koto/templates/bad.md" contains "not valid yaml frontmatter"
    When I run "koto template compile .koto/templates/bad.md"
    Then the exit code is not 0
    And the output contains "error"
