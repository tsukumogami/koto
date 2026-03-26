# Product Review: Local Session Storage Design

Panel role: koto user and template author perspective

## Q1: Template author experience BEFORE and AFTER Feature 1

### Before

Template authors hardcode `wip/` paths in two places: gate commands and directive text.

```yaml
gates:
  greeting_exists:
    type: command
    command: "test -f wip/spirit-greeting.txt"
```

```markdown
Create a file at `wip/spirit-greeting.txt`
```

The path is baked into the template at authoring time. It works, but the template is permanently coupled to a specific directory layout. If a user wants artifacts elsewhere, the template breaks. More practically, the `wip/` convention creates git noise -- every PR shows workflow artifacts alongside real code changes.

Template authors don't have a way to say "put it wherever koto says." They just pick a path and hope it holds.

### After

Gate commands use `{{SESSION_DIR}}`:

```yaml
gates:
  greeting_exists:
    type: command
    command: "test -f {{SESSION_DIR}}/spirit-greeting.txt"
```

Directives reference it too:

```markdown
Create a file at `{{SESSION_DIR}}/spirit-greeting.txt`
```

The engine resolves `{{SESSION_DIR}}` at runtime in `handle_next`, so the template works regardless of which backend is configured. The template author writes one template that works for local, git, and cloud storage.

### Is the improvement clear and tangible?

Yes. The change is small in the template (swap `wip/` for `{{SESSION_DIR}}/`) but the effect is significant:

1. **Templates become backend-agnostic.** One template works for all users regardless of their storage config.
2. **Git stays clean by default.** The most common complaint (wip/ polluting PRs) goes away without the template author doing anything special.
3. **The variable syntax is already familiar.** Template authors already use `{{SPIRIT_NAME}}` for user-defined variables. `{{SESSION_DIR}}` follows the same pattern -- no new concept to learn.

The one concern: `{{SESSION_DIR}}` in directive text produces paths like `~/.koto/sessions/a1b2c3d4e5f6g7h8/my-workflow/spirit-greeting.txt`. That's a long, opaque path that agents will have to work with. It's functional but less human-readable than `wip/spirit-greeting.txt`. This is a real trade-off, not a bug -- the path being managed by koto is the whole point.

## Q2: Skill author experience BEFORE and AFTER

### Before

Skill authors (people writing SKILL.md files) hardcode `wip/` paths in their execution instructions and evidence documentation:

- "Create `wip/spirit-greeting.txt`"
- "Gate checks `test -f wip/spirit-greeting.txt`"
- "Check `wip/` for active artifacts"

The SKILL.md tells the agent exactly where to write files. Simple, direct, but brittle.

### After

Skill authors instruct agents to call `koto session dir <name>` to discover the session path:

```markdown
SESSION_DIR=$(koto session dir hello)
# Write artifacts to $SESSION_DIR/
```

The SKILL.md documents the discovery mechanism instead of hardcoding paths. The agent calls `koto session dir` once, then uses the returned path for all file operations.

### Is the improvement clear and tangible?

It's a moderate improvement. The indirection adds a step, but the benefit is real: skills become portable across storage backends. A skill written today works with local, git, or cloud storage without changes.

The concern is cognitive overhead. Today's SKILL.md says "write to `wip/foo.txt`" -- one sentence. The new version says "call `koto session dir`, capture the output, then write to `$SESSION_DIR/foo.txt`." That's three concepts instead of one. For simple skills like hello-koto, this feels like overkill. For complex multi-artifact skills (shirabe's explore, work-on), the abstraction pays for itself because you call `koto session dir` once and use it everywhere.

## Q3: Is `koto session dir` ergonomic enough?

**For agent-driven skills (the primary use case): yes.** Agents call it once, store the result, and use it for all subsequent file operations. The cost is one shell invocation. Agent tools (Read/Edit/Write) accept absolute paths, so `~/.koto/sessions/<hash>/<name>/` works fine.

**For human debugging: it's adequate but not great.** A developer troubleshooting a failed workflow needs to find where artifacts landed. `koto session dir my-workflow` gives them the path, but they need to know the workflow name first. `koto session list` fills that gap.

**Will skill authors use it or work around it?** They'll use it because there's no simpler alternative. The design correctly closes the escape hatch -- `wip/` won't exist by default, so hardcoding it doesn't work. The only way to find the session directory is to ask koto. This is the right forcing function.

**One ergonomic gap:** The design doesn't address the case where a skill author wants to reference the session directory in a SKILL.md example without knowing the workflow name at authoring time. The SKILL.md shows `koto session dir hello` but the actual workflow name is chosen by the user at `koto init` time. This is a documentation convention issue, not a design flaw -- skill authors use placeholder names in examples.

## Q4: Is `{{SESSION_DIR}}` the right variable name?

Yes. It's descriptive, follows the existing `{{VARIABLE_NAME}}` convention, and is unlikely to collide with user-defined variables (the design should probably reserve the prefix or the specific name).

### Should other engine variables exist from day one?

The design proposes only `SESSION_DIR`. Here's what I'd evaluate:

| Variable | Case for day-one | Verdict |
|----------|-------------------|---------|
| `{{SESSION_DIR}}` | Required. Templates can't reference session artifacts without it. | Ship it |
| `{{WORKFLOW_NAME}}` | Templates already know this implicitly (the author wrote the template). But skills that wrap multiple templates might find it useful for dynamic artifact naming. | Defer. Low urgency, easy to add later. |
| `{{WORKING_DIR}}` | The git repo root / working directory. Useful for templates that need to reference project files in gates. Currently gates run with CWD set to the working directory, so `test -f ./foo` already works. | Defer. Current behavior covers it. |
| `{{TEMPLATE_DIR}}` | Path to the directory containing the template source. Useful for templates that reference sibling files. | Defer. Niche use case. |

Shipping only `{{SESSION_DIR}}` is the right call. The `substitute_vars` HashMap design makes adding more variables trivial later. The risk of shipping too many day-one variables is that they become commitments before usage patterns are clear.

**One recommendation:** The design should explicitly reserve `SESSION_DIR` (and perhaps all-caps names without underscores, or a `KOTO_` prefix) so that user-defined variables via `--var` can't shadow built-in variables. The design doc mentions "built-ins refuse override" in the Decision 7 section but doesn't specify the mechanism. This should be nailed down before `--var` ships, but it's fine to defer the implementation to the `--var` feature.

## Q5: What happens to hello-koto?

hello-koto needs updating. Currently it hardcodes `wip/` in two places:

1. **Gate command:** `test -f wip/spirit-greeting.txt`
2. **Directive text:** `Create a file at wip/spirit-greeting.txt`

### Updated template

```yaml
gates:
  greeting_exists:
    type: command
    command: "test -f {{SESSION_DIR}}/spirit-greeting.txt"
```

```markdown
## awakening

You are {{SPIRIT_NAME}}, a tsukumogami spirit awakening for the first time.

Create a file at `{{SESSION_DIR}}/spirit-greeting.txt` containing a greeting
from {{SPIRIT_NAME}} to the world.
```

### Does the update make it better?

As a learning example, it's mixed:

- **Better:** It demonstrates `{{SESSION_DIR}}` alongside `{{SPIRIT_NAME}}`, showing template authors that engine-provided and user-provided variables use the same syntax. This is a strong teaching moment.
- **Slightly worse for first impressions:** The absolute path (`~/.koto/sessions/a1b2.../hello/spirit-greeting.txt`) that appears in the resolved directive is intimidating compared to `wip/spirit-greeting.txt`. A new user might wonder why their file lives in some opaque hash directory.

On balance, the update is necessary (hello-koto should demonstrate the recommended pattern) and the teaching benefit outweighs the readability cost. The SKILL.md and custom-skill-authoring.md guide should be updated at the same time to explain `{{SESSION_DIR}}` -- the guide currently uses `wip/` throughout its worked example.

### Blast radius

The skill authoring guide (`docs/guides/custom-skill-authoring.md`) references `wip/` in 7+ places across template examples, gate documentation, and the worked example. All of these need updating. The eval patterns (`evals/hello-koto/patterns.txt`) may need adjustment if the expected commands change. The SKILL.md itself needs the path discovery step added.

## Q6: User stories not satisfied by Feature 1

The PRD lists five user stories. Here's how Feature 1 maps:

| User story | Satisfied by Feature 1? | Notes |
|------------|------------------------|-------|
| **Team developer** (artifacts out of git) | Yes | Local backend stores at `~/.koto/`, zero repo footprint |
| **Machine switcher** (resume on another machine) | No | Requires cloud sync (Feature 4) |
| **Skill author** (ask koto where to write) | Yes | `koto session dir` is the discovery API |
| **Git-preferring developer** (opt-in git storage) | No | Requires git backend (Feature 3) + config (Feature 2) |
| **Multi-workflow developer** (list/cleanup sessions) | Yes | `koto session list` and `koto session cleanup` |

### Is the gap acceptable?

Yes. The two unsatisfied stories (machine transfer, git opt-in) are explicitly sequenced as later features in the roadmap. Feature 1 delivers the most impactful story (getting artifacts out of git) and the foundational infrastructure (SessionBackend trait) that makes the other stories possible.

The roadmap's sequencing is sound. The trait boundary means Features 3 and 4 are additive -- they implement SessionBackend without changing command logic. Feature 2 (config) is the only prerequisite for backend selection.

One nuance: the **git-preferring developer** story has a temporary regression. Today, `wip/` works by default. After Feature 1 ships, the git backend doesn't exist yet, so users who want the old behavior can't get it until Feature 3. The PRD acknowledges "no backward compatibility constraint" since there are no external users, which is correct. But if external users adopt koto between Feature 1 and Feature 3, they'd lose the ability to store artifacts in git. The mitigation is shipping Features 1 and 3 close together, or documenting the limitation clearly.

## Summary of findings

1. **Template author improvement is clear and tangible.** `{{SESSION_DIR}}` is a natural extension of existing variable syntax, makes templates backend-agnostic, and solves the git pollution problem.

2. **Skill author improvement is moderate but necessary.** The indirection of `koto session dir` adds a step, but the forcing function (no `wip/` by default) ensures adoption.

3. **`koto session dir` is ergonomic enough for agents**, which is the primary consumer. Human debugging is adequate via `koto session list` + `koto session dir`.

4. **`{{SESSION_DIR}}` is the right and only variable needed day one.** The HashMap design makes future additions trivial. Reserve the name against user override before `--var` ships.

5. **hello-koto must be updated** and the update improves it as a teaching example. The skill authoring guide needs a parallel update -- its `wip/` references are pervasive.

6. **Two of five user stories are deferred**, both intentionally and with clear sequencing. The gap is acceptable given no external users exist today.
