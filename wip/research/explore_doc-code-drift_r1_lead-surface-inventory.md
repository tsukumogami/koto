# Lead: L2 surface-inventory — enumerate koto's documentation surfaces and characterize each one's relationship to the code

All counts below were produced by scripts run against the worktree at
`/home/dgazineu/dev/niwaw/tsuku/tsuku+koto_doc_code_drift-a5071d6a/public/koto/.claude/worktrees/doc-code-drift`
(HEAD `363589c`, "chore(release): pin koto-version default to v0.12.0"). The scripts live in
the job scratch dir and are named inline so each number is reproducible.

## Findings

### 1. The surfaces, with sizes

`find <dir> -type f -name '*.md' | wc -l` and `... -print0 | xargs -0 cat | wc -c/-l`:

| Surface | Files | Bytes | Lines | Frontmatter `status:` |
|---|---:|---:|---:|---|
| `docs/` (all) | 79 | 2,215,972 | 43,021 | mixed, see below |
| `docs/designs/current/` | 39 | 1,359,237 | 26,747 | `Current` ×39 |
| `docs/designs/archive/` | 4 | 141,043 | 2,345 | `Superseded` ×4 |
| `docs/prds/` | 17 | 434,254 | 7,714 | `Done` ×15, `Implemented` ×1, `Complete` ×1 |
| `docs/briefs/` | 7 | 66,661 | 1,277 | `Done` ×7 |
| `docs/guides/` | 7 | 113,574 | 2,595 | **none — no frontmatter at all** |
| `docs/reference/` | 2 | 63,575 | 1,482 | none / no `status:` key |
| `docs/testing/` | 1 | 6,835 | 172 | none |
| `docs/` root (`STABILITY.md`, `workspace-layout.md`) | 2 | 30,793 | — | none |
| `plugins/koto-skills/` (`.md` + `.mdc`) | 19 | 287,537 | 5,483 | SKILL.md ×3 have skill frontmatter; references have none |
| `README.md` | 1 | 8,721 | 176 | n/a |
| `CHANGELOG.md` | 1 | 14,218 | 271 | n/a |
| `CLAUDE.md` | 1 | 4,192 | 105 | n/a |
| `install.sh` | 1 | 5,365 | 188 | n/a |
| `src/**/*.rs` | 76 | — | 78,509 | 13,546 string literals, 8,656 `///` doc-comment lines |

`CLAUDE.local.md`, `AGENTS.override.md`, and `AGENTS.md` **do not exist** in this repo.
`.claude/settings.json` (216 bytes) and `.claude-plugin/marketplace.json` are the only
other agent-facing config.

The single largest doc is `docs/designs/current/DESIGN-batch-child-spawning.md` at
216,830 bytes — 16% of `docs/designs/current/` on its own, and the source of a
disproportionate share of the phantom-verb hits below.

### 2. Load-bearing vs. record — per surface

**Load-bearing** (someone acts on it; stale = wrong action):

- `plugins/koto-skills/skills/*/SKILL.md` + `references/*.md` (19 files, 287 KB) — the
  most load-bearing surface in the repo. These are *shipped as a Claude Code plugin*;
  an agent reads them and executes what they say. `koto-user/references/command-reference.md`
  is 897 lines of "run this, expect this exit code."
- `plugins/koto-skills/.cursor/rules/koto.mdc` — same content class, Cursor-facing.
- `docs/guides/` (7 files) — user-facing how-to. `cli-usage.md` alone is 45 KB.
- `docs/reference/error-codes.md`, `docs/reference/session-feed.md` — contracts consumers
  route on.
- `README.md` — install + quick start; the commands in it are executed verbatim.
- `docs/guides/template-freshness-ci.md` — instructs *other repos* on how to wire koto CI.
  Stale here means someone else's CI breaks.
- `install.sh` and the compiled binary's strings/`--help` — the most load-bearing surface
  of all, because there is no editorial review step between a Rust string literal and a user.
- `docs/testing/MANUAL-TEST-agent-flow.md` — a script a human follows.
- `CLAUDE.md` — agents follow it.

**Record** (true when written; staleness is expected and correct):

- `docs/designs/current/` (39) and `docs/designs/archive/` (4). Design docs record decisions,
  including *rejected* ones. `DESIGN-mid-state-decision-capture.md` weighs `koto record` vs
  `koto annotate` before choosing `koto decisions record`. Both rejected names are in
  backticks. Checking these means flagging the alternatives-considered section of every
  design doc koto has ever written.
- `docs/prds/` (17) and `docs/briefs/` (7) — same argument; requirements as of writing.
- `CHANGELOG.md` — by construction a record. Entries describe past releases. A verb removed
  in 0.11 legitimately still appears in the 0.9 entry.
- `docs/STABILITY.md` — a policy document written in the future tense ("Tool is published
  as `koto migrate` or under a similar discoverable subcommand"). Neither record nor
  load-bearing in the usual sense; it describes a commitment, not an interface.

**The ambiguous middle**: `docs/designs/current/` is labeled `Current` but 39 documents
cannot all describe the shipped binary. `DESIGN-koto-runs-commands.md` is `status: Current`
and specifies `koto session rebind <name> [--to <dir>]` at line 691, in a design that has
partially shipped — the anchor check landed, the rebind verb did not. `Current` means
"not superseded", not "matches HEAD".

### 3. Raw density of code-ish backticked spans

Inline backtick spans, excluding fenced-block contents (`/home/dgazineu/.claude/jobs/9b3bab36/tmp/spans.py`):

| Surface | Inline spans | Fenced blocks | Lines inside fences |
|---|---:|---:|---:|
| `docs/` | 13,772 | 521 | 5,677 |
| `plugins/koto-skills/` | 2,773 | 169 | 1,423 |
| README + CHANGELOG + CLAUDE.md | 223 | 12 | 97 |
| **total** | **16,768** | **702** | **7,197** |

Per docs subdirectory: `designs/current` 9,509 · `prds` 2,025 · `guides` 785 · `designs/archive` 590 ·
`reference` 506 · `docs` root 201 · `briefs` 109 · `testing` 47.

**A naive extractor's real hit volume.** I ran a deliberately naive extractor over all
markdown — every inline span *and* every fenced-block line starting with `koto` — and
resolved each against the actual clap verb set parsed out of `src/cli/*.rs`
(`naive.py`, `verbs.py`, `naive2.py`). The real verb set is **52 paths** (16 top-level
commands, 36 two-level subverbs).

```
total `koto ...` candidates: 2203
  inline:        1858
  fence:bash:     201
  fence:none:     138
  fence:markdown:   4
  fence:sh:         2
unparseable/skipped: bare `koto` 37, flag-only 1, non-verb token 1
resolved-and-real:              2035  (51 distinct verb paths)
resolved-but-NOT-in-verb-set:    129  (30 distinct)   -> 6.0%
```

Bucketed by surface — **this is the number that decides scope**:

| surface | candidates | resolve | miss | miss % |
|---|---:|---:|---:|---:|
| docs/designs/archive | 87 | 60 | 27 | **31.0%** |
| docs/designs/current | 865 | 798 | 67 | 7.7% |
| docs/prds | 473 | 447 | 26 | 5.5% |
| docs (root) | 15 | 14 | 1 | 6.7% |
| docs/reference | 40 | 39 | 1 | 2.5% |
| plugins/koto-skills | 421 | 415 | 6 | 1.4% |
| docs/guides | 177 | 176 | 1 | 0.6% |
| root files (README/CHANGELOG/CLAUDE) | 45 | 45 | 0 | 0.0% |
| docs/testing | 28 | 28 | 0 | 0.0% |
| docs/briefs | 13 | 13 | 0 | 0.0% |

**Load-bearing surfaces only** (guides + reference + plugins + root files + testing + docs root):
**726 candidates, 9 misses, 1.24%.** Record surfaces (designs + prds + briefs):
**1,438 candidates, 120 misses, 8.3%.**

All nine load-bearing misses, in full:

1. `plugins/koto-skills/skills/koto-user/SKILL.md:201` — `koto session rebind`
2. `plugins/koto-skills/skills/koto-user/SKILL.md:542` — `koto session rebind`
3. `plugins/koto-skills/skills/koto-user/references/command-reference.md:698` — `koto session rebind`
4. `plugins/koto-skills/skills/koto-user/references/error-handling.md:126` — `koto session rebind`
5. `docs/guides/default-action-authoring.md:574` — `koto session rebind`
6. `docs/reference/error-codes.md:107` — `koto session rebind`
7. `plugins/koto-skills/skills/koto-user/references/command-reference.md:169` — `koto query`
8. `plugins/koto-skills/skills/koto-user/references/batch-workflows.md:330` — `koto query`
9. `docs/STABILITY.md:93` — `koto migrate`

Six of nine are the known `session rebind` bug — and **all six sites explicitly say the
verb does not exist.** `docs/reference/error-codes.md:107`: "Both messages name
`koto session rebind`, which is not implemented yet". `command-reference.md:696` has a
whole section titled `## koto session rebind — not implemented`. #9 is future tense policy.
Only #7 and #8 are new: `koto query` is not a verb, and `batch-workflows.md:330` presents
it inside a ```bash fence as something to run:

```bash
koto status parent~1.task-a     # state, outcome, evidence
koto query parent~1.task-a      # full event log
koto workflows --children parent~1  # all children of that attempt
```

That is a second live instance of the same class, shipped in the plugin, found by the
cheapest possible check.

### 4. The 40-span random sample (the false-positive budget)

Sampled with `random.Random(20260823).sample(pop, 20)` from each population — 20 of 13,772
docs spans and 20 of 2,773 plugin spans — then hand-adjudicated (the auto-classifier's
labels are in the script output; where I disagreed with it, my call is what is tabulated).

**docs/ (20 of 13,772):**

| span | file:line | class |
|---|---|---|
| `"failed"` | docs/guides/cli-usage.md:175 | d (JSON value of `GateOutcome`) |
| `occupancy_slice` | DESIGN-self-loop-suppresses-details.md:284 | d |
| `koto status` | PRD-request-lifecycle.md:435 | a |
| `koto next <synthetic-child>` | DESIGN-batch-child-spawning.md:2563 | a (+ placeholder arg) |
| `request_store` | PRD-request-store-converge.md:359 | d |
| `0700` | DESIGN-koto-ad-hoc-workflows.md:382 | d (permission literal) |
| `skip_if_matched` | DESIGN-auto-advance-transitions.md:473 | d (template field) |
| `when` | DESIGN-koto-cli-output-contract.md:162 | d (template field) |
| `--format` | DESIGN-visual-workflow-preview.md:61 | d (bare flag, not an invocation) |
| `accepts` | DESIGN-unified-koto-next.md:642 | d (template field) |
| `condition_type` | DESIGN-session-feed-data-contract.md:1100 | d |
| `key: <key>` | PRD-local-dashboard.md:235 | f |
| `src/template/types.rs` | DESIGN-hierarchical-workflows.md:600 | b |
| `when` | DESIGN-koto-cli-output-contract.md:153 | d |
| `SchedulerOutcome` | DESIGN-batch-child-spawning.md:3623 | d (type) |
| `handle_decisions_record` | DESIGN-mid-state-decision-capture.md:91 | d (fn) |
| `tests/fixtures/native-workflows/enriched-shape.json` | guides/native-workflows-verification.md:104 | b |
| `intent` | reference/session-feed.md:1000 | d |
| `koto template lint` | designs/archive/DESIGN-koto-cli-tooling.md:192 | a (phantom verb, archived) |
| `field_equals` | DESIGN-template-evidence-routing.md:400 | d |

**plugins/koto-skills/ (20 of 2,773):**

| span | file:line | class |
|---|---|---|
| `__action__` | koto-user/references/response-shapes.md:598 | d |
| `KOTO_TICK_SESSION` | koto-user/references/command-reference.md:106 | d (env var) |
| `false` | koto-user/references/response-shapes.md:547 | d |
| `koto request list [--requested-by ID \| ...] [--state open\|closed] [--unresolved-legs]` | koto-user/SKILL.md:411 | a (usage synopsis) |
| `details` | .cursor/rules/koto.mdc:170 | d |
| `predicate_impossible` | koto-user/references/error-handling.md:263 | d |
| `failure_mode` | koto-author/references/template-format.md:410 | d |
| `children` | koto-user/references/batch-workflows.md:203 | d |
| `true` | koto-user/references/command-reference.md:514 | d |
| `koto next` | koto-author/references/template-format.md:134 | a |
| `promoted` | koto-user/references/command-reference.md:444 | d |
| `concurrent_access` | koto-user/references/error-handling.md:71 | d |
| `override_default` | koto-author/references/template-format.md:523 | d |
| `--with-data '<json>'` | koto-user/references/command-reference.md:501 | f |
| `success` | koto-user/references/command-reference.md:170 | d |
| `koto status` | koto-user/references/command-reference.md:26 | a |
| `failed` | koto-author/SKILL.md:77 | d |
| `koto next {{SESSION_NAME}} --with-data @tasks.json` | koto-author/references/examples/batch-coordinator.md:53 | a (+ `{{VAR}}`) |
| `spawn_failed` | koto-user/references/error-handling.md:420 | d |
| `koto-templates/my-skill.md` | koto-author/references/template-format.md:901 | b |

**Distribution (n=40, and n=20 each):**

| class | docs | plugins | combined | % |
|---|---:|---:|---:|---:|
| (a) koto CLI invocation | 3 | 4 | 7 | 17.5% |
| (b) repo-relative path | 2 | 1 | 3 | 7.5% |
| (c) shell command for another tool | 0 | 0 | 0 | 0.0% |
| (d) code identifier / type / field | 14 | 14 | 28 | **70.0%** |
| (e) prose in backticks | 0 | 0 | 0 | 0.0% |
| (f) placeholder / template variable | 1 | 1 | 2 | 5.0% |

The whole-population auto-classifier agrees on the shape: docs 10.8% a / 7.7% b / 1.3% c /
74.5% d / 3.0% e / 2.7% f; plugins 12.0% a / 1.5% b / 1.0% c / 78.9% d / 2.0% e / 4.5% f.

**The takeaway is not what the lead assumed.** The fear is a check that "fires on prose or
placeholders" — but prose-in-backticks is ~3% of the population and did not appear once in
40 random draws, and placeholders are 5%. The actual mass is category (d) at 70%. A checker
scoped to (a) touches ~17% of spans and, on load-bearing surfaces, misfires 1.24% of the
time. A checker that tries to validate (d) is a completely different and much worse
proposition — see next.

### 5. What checking identifiers would cost

I took every inline span matching `^[A-Za-z_]\w*$` with length > 3 that is snake_case or
CamelCase, and grepped `src/**/*.rs` for it verbatim (`ident.py`):

- `docs/`: 4,372 spans, 841 distinct, **649 (77.2%) found in src/** — 192 distinct absent.
- `plugins/`: 767 spans, 214 distinct, **179 (83.6%) found** — 35 distinct absent.

Random sample of the absent ones: `child_not_fenceable`, `MoveFileEx`, `DATABASE_PASSWORD`,
`EpochBoundary`, `GateDecl`, `exclusive_with`, `needs_design`, `TEST_COMMAND`,
`ConvergeBlocked`, `mode_confirmed`, `fan_out`, `analyze_failures`, `lock_contention`.
These split into example workflow state names, illustrative env vars, Windows API names,
and proposed-but-unbuilt types. Almost none are drift. **17–23% naive false-positive rate,
an order of magnitude worse than the 1.24% for CLI verbs on load-bearing surfaces.**

### 6. The binary's own strings

`src2.py` scans every line of `src/**/*.rs` + `install.sh` + `build.rs` for `koto <word>`:

```
274 mentions; 232 resolve to a real verb (31 distinct); 42 do not (31 distinct)
```

Of the 42 non-resolving, only **10 are actually verb-shaped**:

- `koto query` ×5 — `src/cli/mod.rs:4857`, `src/engine/types.rs:{883,1039,1172,2848}`. Three
  are inside backticks in `///` doc comments, which means they reach `--help` output.
- `koto session rebind` ×3 — `src/cli/mod.rs:{3473,3489}`, `src/cli/next_types.rs:179`. The
  shipped bug.
- `koto session info` ×2 — `src/engine/respawn.rs:98` and `:807`. **New find.**
  `RESUME_CONTEXT_PROMPT` is a const handed to a respawning agent:
  `"You are resuming session <id>. Read your prior state via \`koto session info <id>\` and
  prior children via \`koto session list --parent <id>\`; advance from there."`
  `koto session info` does not exist. This is the exact same failure as `session rebind`,
  in a string that only an agent ever sees, and nothing has caught it.

The remaining 32 are `koto <english verb>` prose in `//` and `///` comments — `koto writes`,
`koto builds`, `koto renders`, `koto self-discovers`. Every one is outside backticks.
**Requiring a backtick eliminates the entire false-positive mode in src/ at once**, taking
42 misses down to 10, of which 10/10 are true drift.

`///` doc comments contain 157 `koto <verb>` mentions and feed clap's `--help` text
directly, so they are a load-bearing surface that happens to live in `.rs` files.
Repo-relative paths appear in src/ literals only 6 times (`docs/draft.md` ×4,
`docs/reference/session-feed.md` ×2).

### 7. Existing conventions an extractor can lean on

**Fenced blocks are well-tagged.** 702 fences across docs+plugins+root:

- docs: `(none)` 131, `json` 124, `rust` 106, `bash` 89, `yaml` 35, `markdown` 12, `go` 10,
  `mermaid` 5, `toml` 5, `jsonl` 2, `javascript` 1, `html` 1
- plugins: `json` 54, `bash` 47, `yaml` 30, `(none)` 29, `sh` 4, `mermaid` 3, `markdown` 2
- root: `bash` 6, `json` 3, `(none)` 2, `markdown` 1

**81% of fences carry a language tag.** ```bash / ```sh fences (142 total) are the
highest-signal surface in the repo: a line in one is unambiguously "run this."

**Placeholder conventions** (counted inside inline spans, `ph.py`, pop 16,768):

| convention | spans | files | of those, inside a `koto ...` span |
|---|---:|---:|---:|
| `<angle>` | 685 | 79 | 254 |
| `UPPER_SNAKE` | 236 | 47 | 8 |
| `{{mustache}}` | 115 | 32 | 2 |
| `ellipsis ...` | 67 | 32 | 5 |
| `$VAR` / `${VAR}` | 32 | 11 | 3 |
| `~N` (batch attempt) | 24 | 3 | 7 |
| `{brace}` | 13 | 3 | 1 |
| `$ARGUMENTS` | **0** | 0 | 0 |

`<angle>` is the dominant convention and is used consistently — it appears in 79 of the 98
markdown files scanned. `{{VAR}}` is koto's own template-variable syntax, so it is a
first-class construct, not an ad-hoc placeholder. `$ARGUMENTS` (the Claude Code slash-command
convention) is used **nowhere** in this repo.

**There is no "do not check this" marker anywhere.** Zero HTML comments of the form
`<!-- no-check -->` / `<!-- skip -->`. What exists instead is an *ad-hoc prose convention*,
written four different ways in four files:

- `docs/reference/error-codes.md:107` — "which is not implemented yet"
- `plugins/.../command-reference.md:696` — a heading: `## koto session rebind — not implemented`
- `plugins/.../error-handling.md:126` — "**`koto session rebind` does not exist yet.**"
- `plugins/.../koto-user/SKILL.md:201` — "**That subcommand has not landed yet**"
- `docs/guides/default-action-authoring.md:577` — "**This subcommand has not landed yet.**"

A grep for `not implemented|does not exist|has not landed|not shipped|hypothetical|illustrative|pseudo-code`
across docs+plugins returns 29 hits, of which roughly a third are this pattern and the rest
are unrelated prose. There is nothing machine-readable to hang an allowlist on today.

### 8. The docs lifecycle

It exists and is clean. Every design and PRD carries YAML frontmatter with a `schema:` key
(`design/v1` ×32, `prd/v1` ×12, `brief/v1` ×7; 28 files have none) and a `status:`.
`docs/designs/current/DESIGN-request-lifecycle.md`:

```yaml
---
schema: design/v1
status: Current
upstream: docs/prds/PRD-request-lifecycle.md
```

`docs/designs/archive/DESIGN-koto-engine.md`:

```yaml
---
status: Superseded
superseded_by: docs/designs/current/DESIGN-migrate-koto-go-to-rust.md
```

Observed values by directory:

- `docs/designs/current/`: `Current` ×39 (100%)
- `docs/designs/archive/`: `Superseded` ×4 (100%), each with `superseded_by:`
- `docs/prds/`: `Done` ×15, `Implemented` ×1, `Complete` ×1
- `docs/briefs/`: `Done` ×7
- `docs/guides/`, `docs/reference/`, `docs/testing/`, `docs/` root: **no frontmatter at all**

So historical status is machine-readable via both `status: Superseded` **and** the
`docs/designs/archive/` path — but only for designs. The three terminal spellings in
`docs/prds/` (`Done`/`Implemented`/`Complete`) mean a status-based filter needs a small
synonym set.

### 9. The gate that already exists, and what it skips

`.github/workflows/validate-docs.yml` delegates to
`tsukumogami/shirabe/.github/workflows/validate-docs.yml@main`. Its own header comment:

> Docs that carry a `schema:` field are validated; docs predating that convention are
> skipped with a notice.

It checks *format* — frontmatter, sections, issues-table rows — not content. And by keying
on `schema:`, it skips **every guide, every reference doc, `STABILITY.md`,
`workspace-layout.md`, and the entire `plugins/` tree** — precisely the load-bearing
surfaces. It is also `paths: docs/**`, so a plugin-only PR never triggers it.

`.github/workflows/validate-plugins.yml` does compile templates, but only these:

```bash
find plugins/koto-skills/skills/ -path '*/koto-templates/*.md' -type f
```

That glob matches exactly two files, one of which (`koto-author.mermaid.md`) the loop skips
— so **one template is compile-checked.** Meanwhile 11 files in the repo contain
`initial_state:` (i.e. template-shaped content): the four example templates under
`plugins/koto-skills/skills/koto-author/references/examples/` (batch-coordinator,
batch-worker, complex-workflow, evidence-routing-workflow), three fenced templates in
`koto-adhoc/SKILL.md`, three in `koto-author/references/template-format.md`, and one each in
`docs/guides/custom-skill-authoring.md`, `docs/guides/default-action-authoring.md`,
`docs/designs/current/DESIGN-template-evidence-routing.md`, and
`docs/designs/archive/DESIGN-koto-template-format.md`. **Ten of eleven are unchecked**, and
the four example templates are shipped in the plugin as things an agent is told to copy.

## Implications

1. **Scope the first check to `koto <verb>` inside backticks or bash fences, on load-bearing
   surfaces only.** That is 726 candidates with 9 misses — 1.24%. Seven of the nine are
   already annotated in prose as known-missing, so a one-line allowlist takes the standing
   false-positive count to **two**, both of which are real bugs (`koto query` in a bash fence
   in `batch-workflows.md`, and `koto migrate` in `STABILITY.md`'s future-tense policy).
   A gate with two findings on day one does not get disabled.

2. **Excluding `docs/designs/`, `docs/prds/`, and `docs/briefs/` is not a compromise, it is
   correct.** Those directories account for 120 of the 129 total misses. A design doc that
   weighs `koto record` against `koto annotate` before choosing `koto decisions record` is
   doing its job. The `status:` frontmatter and the `archive/` directory already give a
   machine-readable way to draw this line without a hand-maintained path list.

3. **`src/` belongs in scope on day one, and is nearly free.** The bug that motivated this
   exploration originates in a Rust string literal, not a markdown file — and the docs
   describing it are *more accurate than the binary*. Requiring a backtick collapses 42
   raw misses to 10, all 10 genuine. This surfaces `koto session info` in
   `RESUME_CONTEXT_PROMPT` (`src/engine/respawn.rs:98`), which nothing else has caught.

4. **Do not check code identifiers.** 70% of backticked spans are identifiers, and 17–23%
   of them do not appear in `src/` for entirely legitimate reasons. That is the check that
   gets disabled within a week.

5. **Compile every template-shaped block, not one.** Widening the existing
   `validate-plugins.yml` glob from `*/koto-templates/*` to include
   `references/examples/*.md` is a four-character change that puts four shipped example
   templates under `koto template compile`. Extracting ```yaml fences containing
   `initial_state:` covers the other six. This is a separate, cheaper mechanism than
   verb-checking and it reuses a command that already exists.

6. **The false-positive escape hatch should formalize what the docs already do.** Five
   places already write "not implemented yet" in prose. A structured marker — the obvious
   candidate being an HTML comment, since `$ARGUMENTS` is unused and `<angle>` is taken —
   costs nothing to introduce because there is no existing marker to conflict with.

7. **`<angle>` placeholders are inside 254 of the `koto ...` spans**, so any extractor must
   strip arguments before resolving. It only ever needs the first one or two tokens after
   `koto`; everything past that is noise for this check.

## Surprises

- **The docs are right and the binary is wrong.** All six load-bearing mentions of
  `koto session rebind` explicitly tell the reader it does not exist. Someone did the work
  by hand. The failure was that nothing propagated that knowledge back into
  `src/cli/mod.rs:3473`, where the error message still tells users to run it. A doc-only
  checker would have found nothing here.

- **A second live bug, found by the cheapest possible check.**
  `plugins/koto-skills/skills/koto-user/references/batch-workflows.md:330` puts
  `koto query parent~1.task-a` inside a ```bash fence with the comment `# full event log`.
  `koto query` is not a verb. This ships in the plugin.

- **A third, in the binary, in a prompt handed to an agent.**
  `src/engine/respawn.rs:98` — `RESUME_CONTEXT_PROMPT` tells a respawning agent to run
  `koto session info <id>`. No such verb. Same class, same silence.

- **Docs disagree with each other about the verb set.** Three files enumerate `koto session`'s
  subverbs and all three are different: `docs/reference/error-codes.md:107` says
  "start, dir, list, cleanup, and resolve" (5); `koto-user/SKILL.md:201` says
  "start, dir, list, cleanup, resolve, and update" (6); `command-reference.md:698` says
  "start, dir, list, cleanup, recover, resolve, and update" (7). Seven is correct.
  Two of the three are stale, and they are stale *in the sentence explaining the drift bug*.

- **Zero prose-in-backticks in 40 random draws.** The stated fear — a check that fires on
  prose — is not supported by the sample. This codebase uses backticks for code, and the
  discipline holds.

- **`docs/designs/archive/` has a 31% miss rate, ten times the repo average.** The archive
  is doing exactly what an archive should: preserving a CLI design (`koto transition`,
  `koto validate`, `koto template lint`, `koto cache clear`, `koto template inspect`) that
  no longer exists. Any check that reads it is broken by design.

- **`docs/library-usage.md` is 354 bytes that exist solely to say the thing it documents was
  deleted** ("The Go packages ... were removed as part of the migration to Rust"). The repo
  already has an instinct for this failure mode; it just has no mechanism.

## Open Questions

1. Do `docs/designs/current/`'s 39 `status: Current` docs get treated as record (excluded)
   or as load-bearing? They contribute 67 of 129 misses at 7.7%, and
   `DESIGN-koto-runs-commands.md:691` — a `Current` doc — is where `koto session rebind`
   was specified. Excluding them is defensible (a design is a record of a decision) but it
   means the design that *introduced* the phantom verb goes unchecked.

2. `docs/STABILITY.md:93` writes future-tense policy ("Tool is published as `koto migrate`
   or under a similar discoverable subcommand"). Is future-tense commitment language a
   category the marker needs to cover, or is it rare enough (1 instance) to allowlist?

3. Where does the verb set come from at check time — parsing `src/cli/*.rs` (what I did;
   brittle to clap refactors), or shelling out to a built `koto --help` and walking the
   subcommand tree (accurate, but requires a build in CI)? `validate-plugins.yml` already
   does `cargo build --release`, so the second is cheaper than it looks.

4. Repo-relative paths are 7.5% of the sample and trivially checkable with `test -e`, but
   I did not measure their miss rate. Worth a follow-up count — `docs/` mentions
   `pkg/engine`, `pkg/template`, `pkg/controller` in at least one guide, and those
   directories were deleted in the Go-to-Rust migration.

5. Does `--help` text (the 157 `koto <verb>` mentions in `///` comments) get checked by the
   same mechanism as string literals, or does it need clap's rendered output? Three of the
   five `koto query` instances are in `///` comments.

## Summary

koto has 79 docs (2.2 MB) and 19 plugin files (287 KB) carrying 16,768 inline backticked
spans, of which 2,203 are `koto <verb>` candidates; a naive extractor resolves 2,035 against
the real 52-verb clap set and misses 129 (6.0%), but 120 of those misses live in
`docs/designs/` and `docs/prds/` — record surfaces whose job is to preserve rejected and
superseded verbs — leaving just **9 misses in 726 candidates (1.24%) across the load-bearing
surfaces** (guides, reference, plugins, README, testing), seven of which the docs themselves
already annotate as known-missing in prose. A 40-span random sample splits 17.5% koto CLI /
7.5% path / 0% other-tool shell / 70% code identifier / 0% prose / 5% placeholder, which
says the danger is not prose-in-backticks (zero draws, ~3% of population) but category (d):
checking identifiers carries a 17–23% naive false-positive rate against `src/`, ten times
worse than CLI verbs. The repo already has clean lifecycle frontmatter (`status: Current` ×39
/ `Superseded` ×4 with `superseded_by:`, plus a `docs/designs/archive/` directory), 81%
language-tagged fences, and a consistent `<angle>` placeholder convention, but **no
machine-readable "do not check" marker** — only the same "not implemented yet" sentence
written five different ways — and the existing `validate-docs.yml` gate keys on a `schema:`
field that every load-bearing doc lacks, so it skips all of them; scanning `src/` the same
way surfaces three genuine phantom verbs (`session rebind`, `query`, and a previously
unknown `koto session info` in `RESUME_CONTEXT_PROMPT` at `src/engine/respawn.rs:98`) with
zero false positives once a backtick is required.
