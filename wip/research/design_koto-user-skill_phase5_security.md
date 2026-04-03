# Security Review: koto-user-skill

## Dimension Analysis

### External Artifact Handling

**Applies:** No

This design creates and updates markdown files containing guidance text, JSON/YAML examples, and CLI reference information. No external artifacts are downloaded, executed, or processed. The files are authored directly — there are no fetch operations, no template rendering against untrusted input, and no processing of external feeds or registries. The JSON and YAML snippets embedded in the skill files are static examples for human and agent readers, not parsed or executed by any runtime at creation time.

### Permission Scope

**Applies:** No

Writing markdown files to paths within the repository requires only normal filesystem write access to the working directory. No elevated permissions are needed. The operation does not touch system directories, does not modify shell configuration files, does not install binaries, and does not create executable files. The one structured file being modified — `plugin.json` — is a plugin manifest that lists available skills; adding a single entry to its `skills` array carries no broader permission surface than the existing entries. Deleting `plugins/koto-skills/AGENTS.md` and creating `AGENTS.md` at the repo root are both ordinary file operations within the repository boundary.

### Supply Chain or Dependency Trust

**Applies:** No

This design introduces no new dependencies. There are no package imports, no binary downloads, no third-party registries consulted, and no build steps that pull external artifacts. The content is written by a human or agent author directly into the repository and reviewed through the normal pull-request process. The trust model is identical to any other documentation commit: the author's identity is verified by Git and the change goes through code review before merging.

### Data Exposure

**Applies:** No

The skill files contain only public information: CLI command syntax, flag descriptions, JSON/YAML schema examples, and workflow guidance drawn from the koto source code and documentation. None of the files access, read, or embed user credentials, API keys, environment variables, host identifiers, or any system-specific data. The files are checked into a public repository by design. The `CLAUDE.local.md` instructions note that secrets are stored in `.local.env`, which is gitignored and explicitly not referenced by any of the deliverables in this design.

## Recommended Outcome

**OPTION 3 - N/A with justification:** This design is documentation-only. All four security dimensions — external artifact handling, permission scope, supply chain trust, and data exposure — are inapplicable. The deliverables are markdown and JSON files authored in-repository, requiring no elevated permissions, no external dependencies, and containing no sensitive data. No mitigations are necessary. The change should proceed through standard code review.

## Summary

This design writes documentation-only files (markdown skill guides, reference pages, a plugin manifest entry) entirely within the repository boundary. None of the four security dimensions apply: there are no external inputs, no permission escalations, no new dependencies, and no sensitive data in scope. Standard pull-request review is the appropriate control.
