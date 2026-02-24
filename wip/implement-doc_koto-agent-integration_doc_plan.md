# Documentation Plan: koto-agent-integration

Generated from: docs/designs/DESIGN-koto-agent-integration.md
Issues analyzed: 6
Total entries: 3

---

## doc-1: README.md
**Section**: Agent Integration (new section)
**Prerequisite issues**: #35
**Update type**: modify
**Status**: pending
**Details**: Add an "Agent Integration" section after "Key concepts" that explains how to use koto with AI agents via the Claude Code plugin. Cover plugin installation (`/plugin marketplace add tsukumogami/koto`, `/plugin install koto-skills@koto`), mention the hello-koto skill as a starting point, and briefly describe the agent-driven workflow loop (init / next / execute / transition). Reference the Agent Skills standard for cross-platform support. Keep it short -- point to the authoring guide for custom skills once doc-2 exists.

---

## doc-2: docs/guides/custom-skill-authoring.md
**Section**: (new file)
**Prerequisite issues**: #35, #36, #37
**Update type**: new
**Status**: pending
**Details**: Write the custom skill authoring guide covering: SKILL.md format (YAML frontmatter with name/description, body sections for prerequisites, template setup, execution loop, evidence keys, response schemas, error handling, resume); pairing a SKILL.md with its template file; project-scoped skills (`.claude/skills/<name>/`); contributing to the koto-skills plugin (`plugins/koto-skills/skills/`); template locality constraint (absolute paths in state files, SHA-256 verification); validating templates with `koto template compile`; extracting evidence keys from compiled output; testing with the CI pipeline (#36) and eval harness (#37); security note that directive text is agent-visible; Agent Skills standard cross-platform reach. Use hello-koto as the reference example throughout.

---

## doc-3: README.md
**Section**: Documentation
**Prerequisite issues**: #35, #36, #37
**Update type**: modify
**Status**: pending
**Details**: Add a link to the custom skill authoring guide (`docs/guides/custom-skill-authoring.md`) in the Documentation section. Should appear after the existing CLI and library guide links, with a description like "creating custom workflow skills for AI agents."
