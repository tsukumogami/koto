# Lead: What's the simplest way to distribute a single skill via Claude Code?

## Findings

### Three distribution methods

Claude Code offers three methods ranked by simplicity:

1. **Standalone SKILL.md files**: simplest, unnamespaced. A single SKILL.md can be loaded directly. No marketplace infrastructure needed, but limited discoverability and no versioning.

2. **Single plugins with manifests**: moderate complexity, namespaced. A plugin directory with a manifest and one or more skills. Gives you namespacing (`plugin-name:skill-name`) without full marketplace overhead.

3. **Marketplaces**: comprehensive, multi-plugin. A marketplace.json lists multiple plugins, each with multiple skills. Supports versioning, auto-updates, and installation via `/plugin` or `/install`.

### Koto's current setup

Koto already uses a marketplace to distribute its plugins. The infrastructure is in place. Adding a new skill as a plugin within koto's existing `marketplace.json` is straightforward.

### Recommendation

Since koto already has marketplace infrastructure, adding the template-authoring skill as a new plugin within the existing marketplace avoids creating parallel distribution mechanisms. It gains marketplace features (versioning, auto-updates) without additional setup overhead.

## Implications

The distribution question is largely answered: use koto's existing marketplace. The remaining question is organizational: extend an existing plugin or create a new one.

## Surprises

Standalone SKILL.md files exist as a simpler option, but they lack namespacing and versioning. For something meant to be reusable and maintained, the marketplace format is worth the small overhead.

## Open Questions

- Should the template-authoring skill be a new plugin in koto's marketplace, or extend an existing koto-skills plugin?
- How tightly coupled should the skill be to koto's version? Should it version independently?
- Does the marketplace support installing individual skills from a plugin, or is it all-or-nothing per plugin?

## Summary

Claude Code offers standalone SKILL.md files (simplest), single plugins (moderate), and marketplaces (full-featured). Since koto already has marketplace infrastructure, adding the template-authoring skill as a new plugin within the existing marketplace is the most practical approach -- it avoids parallel distribution mechanisms while gaining versioning and auto-updates. The main open question is whether to create a new plugin or extend an existing one.
