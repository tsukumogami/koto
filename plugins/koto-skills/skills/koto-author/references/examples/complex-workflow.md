---
name: deploy-pipeline
version: "1.0"
description: Deployment pipeline with gates, evidence routing, and self-loops
initial_state: preflight

variables:
  ENVIRONMENT:
    description: Target deployment environment
    required: true
  VERSION:
    description: Version to deploy
    required: true

states:
  preflight:
    gates:
      config_exists:
        type: command
        command: "test -f deploy.conf"
    transitions:
      - target: build
        when:
          gates.config_exists.exit_code: 0    # config present, advance
      - target: preflight                      # self-loop: wait for config
        when:
          gates.config_exists.exit_code: 1
  build:
    gates:
      build_output:
        type: context-exists
        key: build-output.tar.gz
    transitions:
      - target: test
        when:
          gates.build_output.exists: true     # artifact registered, advance
      - target: build                          # self-loop: wait for artifact
        when:
          gates.build_output.exists: false
  test:
    accepts:
      result:
        type: enum
        values: [pass, fail]
        required: true
    transitions:
      - target: staging
        when:
          result: pass
      - target: build
        when:
          result: fail
  staging:
    accepts:
      approval:
        type: enum
        values: [approved, rejected]
        required: true
    transitions:
      - target: production
        when:
          approval: approved
      - target: rollback
        when:
          approval: rejected
  production:
    terminal: true
  rollback:
    terminal: true
---

## preflight

Run preflight checks for deploying {{VERSION}} to {{ENVIRONMENT}}.

Verify that `deploy.conf` exists in the working directory. The gate routes to `build` when the file is present, or loops back here to wait.

## build

Build version {{VERSION}} for {{ENVIRONMENT}}.

Compile the application, package it as `build-output.tar.gz`, and add it to the context store with `koto context add`. The gate routes to `test` when the artifact is registered, or loops back here to wait.

## test

Run the test suite against the build for {{VERSION}}. Submit `pass` or `fail` when done.

<!-- details -->

Execute integration and smoke tests. Submit your result:

- **pass** -- all tests passed, advance to staging
- **fail** -- tests failed, loop back to build and fix the issues

## staging

Version {{VERSION}} is deployed to the {{ENVIRONMENT}} staging environment.

Run validation checks and verify the deployment looks correct. Submit your approval decision:

- **approved** -- promote to production
- **rejected** -- roll back the deployment

## production

Version {{VERSION}} has been deployed to {{ENVIRONMENT}} production. Deployment complete.

## rollback

Version {{VERSION}} deployment to {{ENVIRONMENT}} was rejected. Rolling back to the previous version.
