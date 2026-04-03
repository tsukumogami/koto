# Exploration Decisions: koto-user-skill

## Round 1

- Parallel workstreams: koto-author update and koto-user creation are independent — neither blocks the other. User chose parallel over sequential.
- Plugin placement: koto-user lives in `plugins/koto-skills/skills/koto-user/` (same plugin as koto-author). One directory + one `plugin.json` line. Separate plugin ruled out — adds overhead, no benefit.
- AGENTS.md at plugin root is misplaced: `plugins/koto-skills/AGENTS.md` should not serve as skill reference content via AGENTS.md mechanism. Content should move to skill `references/` directories where it's explicitly linked from SKILL.md.
- Root-level `koto/AGENTS.md` is needed: for repo-wide context loading by any Claude Code session in koto. Lighter than skill content — overview/orientation level.
- Skills use custom `references/` files: not AGENTS.md for skill-specific content.
- Three workstreams identified: (A) koto-author update, (B) koto-user creation, (C) root AGENTS.md.
- Eval cases are the highest-leverage freshness mechanism: existing harness in place, zero infrastructure cost. 5–8 cases targeting gate-blocked handling, evidence submission, override two-step flow, rewind.
- File-change heuristics ruled out: high false-positive rate, adds process noise without reliable signal.
