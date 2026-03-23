# Exploration Decisions: template-variable-substitution

## Round 1
- Use `{{KEY}}` syntax as-is: already established in templates and design docs, no reason to deviate
- Allowlist sanitization over escaping or env vars: escaping is fragile, env vars don't work for directive text, allowlist eliminates injection risk entirely
- `Variables` newtype over standalone function or trait: encapsulates event extraction + substitution, trait is overengineered for string replacement
- Narrow to `HashMap<String, String>`: matches template declarations, CLI input, and usage context; `serde_json::Value` was speculative and unused
- Error on undefined references (not empty string): matches parent design requirement and prevents silent bugs
- Duplicate `--var` keys should error: prevents silent override bugs
