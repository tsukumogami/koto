# Lead: L4 v0.12.0-forensics — reconstruct the exact evidence for all five instances of the defect class, so the check we build can be proven against them

All evidence below is read-only against `v0.12.0` (commit `3b06b0cb`, release commit `79e8036`),
`origin/main`, and the worktree at `.claude/worktrees/doc-code-drift`. No tag was checked out and
the working tree was not modified. Where a binary was needed, the already-installed
`/home/dgazineu/.tsuku/tools/current/koto` was used — it self-reports
`koto 0.12.0 (79e8036 2026-08-23T22:07:22Z)`, i.e. it *is* the v0.12.0 release binary, which makes
every runtime observation below first-party evidence rather than a reconstruction.

---

## Findings

### Instance 1 — `koto session rebind` (the primary acceptance test)

#### 1a. The defect, demonstrated end to end on the v0.12.0 binary

The released binary emits a repair instruction naming a subcommand that the same binary rejects:

```
$ koto session rebind demo --to /tmp
error: unrecognized subcommand 'rebind'

Usage: koto session <COMMAND>

For more information, try '--help'.
```

```
$ koto session --help
Session management subcommands

Usage: koto session <COMMAND>

Commands:
  start    Start a child session under `--parent`
  dir      Print the absolute session directory path
  list     List all sessions as JSON
  cleanup  Remove a session directory (idempotent)
  resolve  Resolve a session version conflict
  update   Update session metadata fields
  recover  List, and optionally restore, sessions the old-layout migration set aside because their name was already taken
  help     Print this message or the help of the given subcommand(s)
```

This is the acceptance fixture in its purest form: one process, two contradictory statements.

#### 1b. The three Rust string literals that reach a user

These are the only occurrences at v0.12.0 where the *binary* names the verb. Everything else is
prose about the binary.

**(i) `src/cli/mod.rs:3469-3482` — `execution_anchor_unresolvable`, exit 3.** Full context:

```rust
            ExecutionAnchorCheck::Unresolvable { status } => {
                let machine = match &status.machine_id {
                    Some(id) => format!(" ({})", id),
                    None => String::new(),
                };
                let err = NextError {
                    code: NextErrorCode::ExecutionAnchorUnresolvable,
                    message: format!(
                        "workflow '{}' is bound to {}, which does not resolve on this machine{}; \
                         run `koto session rebind {} --to <dir>` if the checkout moved",
                        name,
                        status.path.display(),
                        machine,
                        name,
                    ),
                    details: vec![],
                };
                let json = serde_json::json!({"error": err});
                exit_with_error_code(json, err.code.exit_code());
            }
```

The literal is split across a Rust line continuation (`\` at end of line 3472). The token
`rebind` sits on **line 3473**; the words `koto session` sit on the *same* physical line
(`run \`koto session rebind {} --to <dir>\``), so a naive line-oriented extractor still sees the
whole phrase — but only because of where the author happened to break the string. See Surprises.

**(ii) `src/cli/mod.rs:3484-3499` — `execution_anchor_mismatch`, exit 2.** Full context:

```rust
            ExecutionAnchorCheck::Outside { anchor, cwd } => {
                let err = NextError {
                    code: NextErrorCode::ExecutionAnchorMismatch,
                    message: format!(
                        "workflow '{}' is bound to {}; `koto next` must run from that directory \
                         or one beneath it, not {}. Run `koto session rebind {} --to <dir>` if \
                         the checkout moved",
                        name,
                        anchor.display(),
                        cwd.display(),
                        name,
                    ),
                    details: vec![],
                };
                let json = serde_json::json!({"error": err});
                exit_with_error_code(json, err.code.exit_code());
            }
```

Here the phrase `koto session rebind {} --to <dir>` is intact on **line 3489**, but the sentence
continues onto 3490 across a continuation. Note the capitalised `Run` here vs lowercase `run` in
(i) — the two messages are not textually uniform.

**(iii) `src/cli/next_types.rs:173-185` — the anchor-adoption notice**, spliced into the
`directive` field of a *successful* tick (not an error path):

```rust
/// One-time notice that a session which recorded no execution anchor
/// has adopted the directory this tick ran from (R14).
///
/// Spliced into `directive` via [`NextResponse::with_directive_prefix`],
/// the same mechanism the recovery pointer and the leg-abandonment
/// notice use. It says what koto bound and how to move it, and claims
/// nothing about what a command can reach once running -- anchoring
/// binds where commands start, not where they can go (R17).
pub fn execution_anchor_adopted_notice(name: &str, anchor: &std::path::Path) -> String {
    format!(
        "[koto] Session '{}' had no recorded directory; it is now bound to {}. \
         Later ticks must run there or below it -- `koto session rebind {}` moves it.\n\n",
        name,
        anchor.display(),
        name,
    )
}
```

`koto session rebind {}` on **line 179**. This one has **no `--to` flag** — a third distinct
surface form. It also reaches the user on the *happy path*, so a user who never hits an error
still gets told to run a command that does not exist.

Additionally `src/cli/next_types.rs:725` is a doc comment on the `ExecutionAnchorMismatch` variant
("...or rebind the session to where the checkout now is") — developer-facing, not user-facing.

#### 1c. Surface classification of every `rebind` occurrence at v0.12.0

`git grep -n -i "rebind" v0.12.0` returns 43 hits across 20 files. Classified:

| Surface | Files / lines | Names the verb? | Notes |
|---|---|---|---|
| **Rust literal reaching a user** | `src/cli/mod.rs:3473`, `src/cli/mod.rs:3489`, `src/cli/next_types.rs:179` | **Yes, all three** | The defect. Backticked *inside* the string literal. |
| Rust doc comment | `src/cli/next_types.rs:725`, `src/cli/request.rs:148,561`, `src/engine/request_store/mod.rs:238,239,1144,1177,1341,1343`, `src/engine/template_source_status.rs:161,219` | No — all are the *unrelated* request-leg rebinding concept, or prose | False-positive class for a bare `rebind` matcher |
| Rust runtime literal (unrelated) | `src/engine/request_store/mod.rs:242` (`"cannot rebind to {requested_child}"`) | No | Request-leg concept, legitimate |
| Rust test | `src/engine/request_store/tests.rs:612,626,640,668`; `tests/request_cli.rs:814,816,820,857,871,872,962,1066`; `tests/discovery_scan.rs:723` | No | Request-leg concept |
| **Rust test asserting the defect** | `tests/execution_anchor_test.rs:400-404` | **Yes** | See 1e — this is the reason the suite stayed green |
| Guide under `docs/` | `docs/guides/default-action-authoring.md:515,519,569,571,574,588` | Yes at 515, 574, 588 | 574 is inside a bare ``` fence |
| Reference under `docs/` | `docs/reference/error-codes.md:71,72,94,102,105,107,352`; `docs/reference/session-feed.md:941` | Yes at 94, 102, 107 | 94/102 inside ```json fences |
| Design doc | `docs/designs/current/DESIGN-koto-runs-commands.md:385,417,584,679,691,736`; `DESIGN-request-lifecycle.md:38,68,193,796,797,961` | Yes at 584, 691, 736 | Forward-looking design; 584 is a markdown table cell |
| PRD | `docs/prds/PRD-koto-runs-commands.md:172,273,274,285,294,427,428,437,483,564,654`; `PRD-request-lifecycle.md:249,525,526` | No — all prose ("rebind", "rebinds") | Requirement text, never the literal command |
| Brief | `docs/briefs/BRIEF-koto-runs-commands.md:165,201` | No | Prose |
| Skill (`plugins/koto-skills/`) | `koto-user/SKILL.md:201,206,369,542`; `koto-user/references/command-reference.md:445,460,696,698`; `koto-user/references/error-handling.md:116,120,124,126,254` | Yes at SKILL.md 201/206/542, command-reference 696/698, error-handling 116/120/126 | |
| **CHANGELOG** | *(none)* | — | `git grep -i rebind v0.12.0 -- CHANGELOG.md` returns nothing |
| Test fixture | *(none)* | — | No fixture names the verb |

#### 1d. The exact form the reference takes — what an extractor must match

This is the operative section for building a check. **There are five textually distinct forms**,
and no single naive pattern catches all of them:

1. **Backticked, with binary name and flag, inside a Rust string literal**:
   `` `koto session rebind {} --to <dir>` `` — `src/cli/mod.rs:3473, 3489`. Note the `{}`
   *interpolation placeholder sits between the verb and the flag*. A matcher keyed on
   `koto session rebind <session>` or on `koto session rebind --to` fails here; a matcher keyed on
   the prefix `koto session rebind` succeeds.
2. **Backticked, binary name, no flag, with placeholder**:
   `` `koto session rebind {}` `` — `src/cli/next_types.rs:179`.
3. **Bare inside a fenced code block, no language tag** — `docs/guides/default-action-authoring.md:571-575`:
   ````
   A developer whose checkout genuinely moved rebinds the session with one deliberate command:

   ```
   koto session rebind <session> --to <dir>
   ```
   ````
   Not backticked (it is the fence content), placeholder is `<session>` not `{}`.
4. **Backticked inside a JSON string inside a ```json fence** — `docs/reference/error-codes.md:94`:
   ```json
   {"error":{"code":"execution_anchor_mismatch","message":"workflow 'my-workflow' is bound to /home/dev/repo; `koto next` must run from that directory or one beneath it, not /tmp/elsewhere. Run `koto session rebind my-workflow --to <dir>` if the checkout moved","details":[]}}
   ```
   Here the placeholder is *resolved* to a concrete example value (`my-workflow`). An extractor
   that skips fenced blocks misses this; one that does not skip them must cope with JSON escaping.
5. **As a markdown heading** — `plugins/koto-skills/skills/koto-user/references/command-reference.md:696`:
   `## koto session rebind — not implemented` (unbackticked, em-dash).

Consequence for the check design: **matching `koto <verb> <subverb>` as a backticked token is
sufficient to catch the Rust literals (forms 1 and 2), which is where the defect actually lives.**
Forms 3-5 are documentation, and in this instance the documentation was *already correct* (it says
the verb does not exist). So a check scoped to Rust string literals alone would have caught
instance 1 — and would have had a far lower false-positive rate than one scoped to all markdown.

#### 1e. The CLI's actual verb set at v0.12.0, from source

Top-level `Command` enum, `src/cli/mod.rs:88-288`, 16 variants (line numbers at v0.12.0):

`Version` (90), `Init` (97), `Next` (141), `Cancel` (183), `Rewind` (196), `Workflows` (206),
`Template` (225), `Session` (231), `Context` (237), `Status` (243), `Decisions` (249),
`Overrides` (255), `Config` (261), `Workspace` (267), `Request` (276), `Dashboard` (285).

`SessionCommand` enum, `src/cli/mod.rs:387-499`, **exactly 7 variants**:

| Variant | Line | Surface name |
|---|---|---|
| `Start` | 402 | `koto session start` |
| `Dir` | 445 | `koto session dir` |
| `List` | 450 | `koto session list` |
| `Cleanup` | 452 | `koto session cleanup` |
| `Resolve` | 457 | `koto session resolve` |
| `Update` | 472 | `koto session update` |
| `Recover` | 487 | `koto session recover` |

**`Rebind` is absent.** Confirmed three ways: the enum body has no such variant; `clap`'s generated
`koto session --help` from the v0.12.0 binary lists the seven above and nothing else; and the
binary answers `error: unrecognized subcommand 'rebind'`.

(Care needed reading this enum by eye: `Auto`/`Skip`/`AcceptRemote`/`AcceptLocal` at lines 506-514
are the `ChildrenPolicy` **ValueEnum**, and `Publish` at 522 is `WorkflowsAction::Publish`
(`koto workflows publish`). Neither belongs to `SessionCommand`.)

**Bonus drift found while enumerating.** Three documents at v0.12.0 each state the session verb
list, and they *disagree with each other*:

- `docs/reference/error-codes.md:107` — "`koto session` currently offers `start`, `dir`, `list`,
  `cleanup`, and `resolve`." **Wrong**: omits `update` and `recover`.
- `plugins/koto-skills/skills/koto-user/SKILL.md:201` — "`start`, `dir`, `list`, `cleanup`,
  `resolve`, and `update`." **Wrong**: omits `recover`.
- `docs/guides/default-action-authoring.md:576-577` — "`start`, `dir`, `list`, `cleanup`,
  `resolve`, and `update`." **Wrong**: omits `recover`.
- `plugins/koto-skills/skills/koto-user/references/command-reference.md:698` — "`koto session`
  offers `start`, `dir`, `list`, `cleanup`, `recover`, `resolve`, and `update`, and nothing else."
  **Correct** — the only one of the four that is.

Three of the four hand-maintained verb lists had already rotted, in three different ways. This is
a second, cheaper acceptance target: a check that diffs a documented verb list against
`clap`'s enum would fire on three files at v0.12.0 independent of the rebind question.

#### 1f. The test that made the defect green

`tests/execution_anchor_test.rs:381-408` at v0.12.0:

```rust
#[test]
fn an_anchor_that_no_longer_resolves_is_a_distinct_refusal() {
    ...
    let message = run.json["error"]["message"].as_str().unwrap();
    assert!(message.contains(anchor.to_str().unwrap()));
    assert!(
        message.contains("rebind"),
        "the refusal must point at the rebind verb, got {}",
        message
    );
    ...
}
```

This is the crux of the whole exploration. The suite did not merely *fail to notice* the defect —
it **required** it. A contributor who removed the phantom verb from the error message would have
broken CI. The assertion's own failure message calls it "the rebind verb", as though it existed.
Any check we build has to be able to fire in the presence of a passing test that says the opposite.

#### 1g. State on the current tree

**The defect is still live and completely unrepaired.**

```
$ git diff --stat v0.12.0 origin/main -- src/cli/mod.rs
(empty)
```

`src/cli/mod.rs` is **byte-identical** between `v0.12.0` and `origin/main`. `SessionCommand` on
`origin/main` still has the same seven variants at the same line numbers (402, 445, 450, 452, 457,
472, 487). Nobody has landed `rebind`. Every hit listed in 1c reproduces verbatim on `origin/main`,
including all three Rust literals and the `contains("rebind")` assertion.

This is good news for the acceptance criterion: the check can be proven against `v0.12.0` *and*
against `HEAD` with the same expected result, and the fixture will not silently stop being a
fixture. It also means that if `rebind` ever does land, the check must go green on its own — so it
should be written against the live `clap` verb set, not against a hardcoded denylist containing
the string "rebind".

---

### Instance 3 — koto's `CLAUDE.local.md` describes a Go repository

`CLAUDE.local.md` is **not** in the worktree and is **not tracked by git** (`git ls-files | grep -i claude`
returns only `.claude-plugin/marketplace.json`, `.claude/settings.json`, `CLAUDE.md`, and
`plugins/koto-skills/.claude-plugin/plugin.json`). It is a niwa-generated file installed at
`/home/dgazineu/dev/niwaw/tsuku/.../public/koto/CLAUDE.local.md`, whose source lives in `dot-niwa`.
This matters: **no CI check in koto can ever see this file**, because it is not in koto's repo.

Every false claim, with the real thing:

| Quoted claim | Why it is false | The real thing |
|---|---|---|
| `├── cmd/koto/        # CLI entry point` | No `cmd/` directory exists (`ls -d cmd` → No such file or directory). Go layout convention. | `src/main.rs` |
| `├── internal/        # Internal packages` | No `internal/` directory exists. Go visibility convention with no Rust analogue. | Rust uses `pub(crate)`; modules live under `src/` |
| `├── pkg/             # Public Go library` | No `pkg/` directory exists. | `src/lib.rs` is the library root |
| `│   ├── cache/       # Cache layer` | Not a directory. | `src/cache.rs` (a file) |
| `│   ├── controller/  # Workflow controller` | Does not exist under any name. | No such module; the advance loop is `src/engine/advance.rs` |
| `│   ├── discover/    # Template discovery` | Not a directory. | `src/discover.rs` (a file) |
| `│   ├── engine/      # Core state machine engine` | Path is `pkg/engine/`, which does not exist | `src/engine/` — the only structurally-correct row, and it is still under the wrong parent |
| `│   └── template/    # Template parsing and compilation` | Path is `pkg/template/`, which does not exist | `src/template/` |
| `go build -o koto ./cmd/koto` | koto is a Rust crate; there is no Go toolchain, no `go.mod`, and no `cmd/koto`. | `cargo build --release` |
| `go test ./...` | Same. | `cargo test` |
| `go install ./cmd/koto` | Same. | `cargo install --path .` |
| `go vet ./...` | Same. | `cargo clippy` |
| `` `koto transition <state>` `` | **No such subcommand.** Not in the top-level `Command` enum, not in `koto --help`. The word "transition" appears in koto only as an internal concept (`src/cli/mod.rs:149`, "Directed transition to a named state"). | `koto next --to <state>` |
| `` `koto query` `` — "Inspect full workflow state as JSON" | **No such subcommand.** Not in `Command`, not in `koto --help`. Only unrelated hit is a dashboard salient-key string (`src/cli/dashboard_data.rs:152`). | `koto status <name>` |
| `- All Go code must pass `gofmt` formatting` | No Go code. | `cargo fmt --check` |

Claims in the same file that happen to be **true**: `koto init`, `koto next`, `koto status`,
`koto rewind`, `koto template compile`, "State files are written atomically". So the file is not
uniformly wrong — it is a Go-repo template with a handful of accidentally-correct rows, which is
precisely the hard case for a reviewer skimming it.

**The same rot has partially survived into koto's committed `CLAUDE.md`**, which was migrated to
Rust but incompletely (`CLAUDE.md:9-22`):

```
koto/
├── cmd/koto/        # CLI entry point
├── src/             # Core library (engine, template, gate, CLI)
│   ├── engine/      # State machine and advance loop
│   ├── template/    # Template parsing and compilation
│   ├── gate/        # Gate evaluators (command, context-exists, context-matches)
│   └── cli/         # CLI subcommands and JSON output types
```

Two falsehoods survive in the tracked file: `cmd/koto/` (does not exist) and `src/gate/` shown as
a **directory** when the real thing is `src/gate.rs`, a 1002-line **file**. That second one is the
direct upstream of instance 5. The build commands in `CLAUDE.md` are correctly Rust
(`cargo build --release`, `cargo test`, `cargo clippy && cargo fmt --check`), and
`cargo test --test integration_test` is valid (`tests/integration_test.rs` exists).

Also still tracked in this Rust repo: **`.golangci.yaml`** (`git ls-files | grep -i golangci`), a
758-byte Go linter config that nothing can consume.

---

### Instance 4 — shirabe's `CLAUDE.md` claims a repo-root `koto-templates/`

The claim, `public/shirabe/CLAUDE.md:257-264`:

```
## Directory Structure

```
shirabe/
├── skills/              # Claude Code workflow skills
├── koto-templates/      # Koto YAML workflow templates
├── .github/workflows/   # Reusable CI validation workflows
├── .claude-plugin/      # Plugin manifest and marketplace entry
└── docs/                # Documentation and guides
```
```

There is **no repo-root `koto-templates/`**. `find . -type d -name koto-templates` returns three
directories, all nested one level below `skills/`:

```
public/shirabe/skills/scope/koto-templates
public/shirabe/skills/work-on/koto-templates
public/shirabe/skills/execute/koto-templates
```

The templates are **co-located with the skill that owns them**, which is a materially different
architecture from a shared root-level template pool — an agent reading this CLAUDE.md would look
for a central registry that does not exist, and would miss that each skill's templates are its own.

Note that 20 other files in shirabe reference `koto-templates` and appear to use the *correct*
nested paths (e.g. `skills/execute/koto-templates/execute.md` is a real file). So this is a single
stale line in the one document most likely to be read first.

---

### Instance 5 — `src/gate/mod.rs` and `docs/template-format.md` citations

Both paths are dead, and **both references still exist on the current tree**.

| Reference | Location on HEAD | Status |
|---|---|---|
| `` `src/gate/mod.rs:206-230` `` | `docs/prds/PRD-koto-runs-commands.md:62` | **Still broken.** No `src/gate/` directory. Live path: **`src/gate.rs`** (1002 lines). |
| `` `docs/template-format.md:142` `` | `docs/prds/PRD-koto-runs-commands.md:109` | **Still broken.** No `docs/template-format.md`. Live path: **`plugins/koto-skills/skills/koto-author/references/template-format.md`**. |
| Both, quoted as errata | `docs/designs/current/DESIGN-koto-runs-commands.md:113-115` | Correct — see below |

The PRD line 62 in full:

> a command gate's evidence keeps only `exit_code` and `error` (`src/gate/mod.rs:206-230`) and never the command's stdout

The PRD line 109 in full:

> table (`docs/template-format.md:142`) and a single Rust integration test

The remarkable part: **the drift was found, written down, and then left in place.**
`DESIGN-koto-runs-commands.md:113-116` says:

> Two details in the tree correct the upstream research. The PRD cites
> `src/gate/mod.rs` and `docs/template-format.md`; the live paths are
> `src/gate.rs` and
> `plugins/koto-skills/skills/koto-author/references/template-format.md`.
> Neither changes any conclusion.

A human caught this by hand, recorded the correction in a *different document*, judged it
immaterial, and never fixed the source. So the PRD still ships two dangling citations, and a reader
who starts at the PRD (the more likely entry point) gets the wrong paths with no errata in sight.

One mitigating detail worth recording for fixture purposes: the *line range* `206-230` is still
roughly accurate — `src/gate.rs:206` is `fn evaluate_command_gate`, and `command_gate_result`
(which builds the `exit_code`/`error` evidence) runs 218-245. So the citation is wrong only in its
path, not its content. A path-existence check catches it; a content check would not need to.

Separately, `docs/designs/current/DESIGN-koto-next-output-contract.md` (lines 65, 72, 175, 230,
320, 348) and `DESIGN-koto-user-skill.md` (17, 53, 125, 127, 244, 279, 310) refer to
`template-format.md` by **bare filename**, without a directory. Those are ambiguous rather than
false — the file does exist, just not at a path any of them state.

---

### Instance 2 — koto-author's `batch-authoring.md` and #213

**The commit.** `2a29fed` — `fix(template): accept every children-complete field in when clauses (#213)`,
dated 2026-08-23, and it **is an ancestor of `v0.12.0`** (`git merge-base --is-ancestor 2a29fed v0.12.0`
succeeds). So this instance was already repaired by the time v0.12.0 shipped; it is a historical
fixture, not a live one.

**What kind of claim was wrong.** Two claims, of two different kinds, in the same skill:

1. **A template snippet the compiler rejected.** `batch-authoring.md` taught authors to route a
   `children-complete` gate on `all_success: true` / `needs_attention: true`, and the shipped
   example `references/examples/batch-coordinator.md` did exactly that. It did not compile. The
   compile-time gate schema (`gate_type_schema` in `src/template/types.rs:388`) listed only five
   fields for `children-complete` while the gate emitted sixteen, so `when` clauses on
   `all_success` and `needs_attention` were rejected. **Warning W4's own remedy could not be
   satisfied** — the compiler told authors to route on fields the compiler then refused.
2. **A template snippet that compiled and failed at runtime.** `template-format.md` taught the
   other pattern: `all_complete: true/false` with a self-loop for the waiting case. That compiled,
   then produced `koto next` exit 3, `template_error`, "cycle detected", on every poll while
   children ran — because a same-state transition re-visits a state already seen this tick.

So the skill shipped two mutually contradicting recipes, and neither one worked. That is a strictly
harder failure than instance 1: instance 1 is a *name* that does not resolve; this is *semantics*
that a name-existence check would sail straight past.

**Reproduced.** I extracted the pre-fix example to a scratch directory and compiled it with the
v0.12.0 binary:

```
$ koto template compile <2a29fed^:.../examples/batch-coordinator.md>
{"command":"template compile","error":"validation error: state \"analyze_failures\": transitions to \"plan_and_await\" and \"summarize\" are not mutually exclusive: transitions share no fields, so both could match the same evidence"}
```

**Are the shipped examples compilable now?** Yes — all four non-generated example templates
compile clean against the v0.12.0 binary:

| Example | `koto template compile` |
|---|---|
| `references/examples/batch-coordinator.md` | exit 0 (15 `never referenced` warnings + one W3) |
| `references/examples/batch-worker.md` | exit 0 |
| `references/examples/complex-workflow.md` | exit 0 |
| `references/examples/evidence-routing-workflow.md` | exit 0 |

**How one would check them — and the gap that let this ship.** koto already has the exact check
needed, and it is pointed at the wrong directory. `.github/workflows/validate-plugins.yml:38`:

```bash
done < <(find plugins/koto-skills/skills/ -path '*/koto-templates/*.md' -type f)
```

That glob matches exactly **two** files in the whole repo:

```
plugins/koto-skills/skills/koto-author/koto-templates/koto-author.md
plugins/koto-skills/skills/koto-author/koto-templates/koto-author.mermaid.md
```

(and the second is skipped by the `*.mermaid.md` case guard, so **one** file is actually compiled).
The shipped examples live at `plugins/koto-skills/skills/koto-author/references/examples/*.md` —
**outside the glob**. The template-compilation CI job has existed all along; it simply never looked
at the files that broke. Widening that `find` to include `references/examples/` is a one-line change
that would have caught instance 2 at PR time.

A second gap: the workflow triggers only on `paths: ['plugins/**', '.claude-plugin/**']`. The #213
root cause was in `src/template/types.rs` — a **compiler-side** change could break a shipped
template without the template-compilation job running at all.

The fix commit did add a durable guard for the schema half: a regression test pinning
`gate_type_schema` against `gate_type_builtin_default` so the two cannot drift silently again, and
`src/template/types.rs:394-399` now carries a comment naming issue #207 as the cautionary tale.
Both functions now list the same sixteen `children-complete` fields.

*(One thing that looks like residual drift but is not: `batch-authoring.md:119` says the gate
"surfaces fifteen output fields: eight counts, six aggregate booleans, and the per-child
`children[]` array" — 8+6+1 = 15 — while the schema has sixteen entries. The sixteenth is `error`,
which is not an output field. Consistent.)*

---

### Cross-cutting: the minimal mechanical check per instance

| # | Instance | Minimal check that would have caught it | Cheap? |
|---|---|---|---|
| 1 | `koto session rebind` in shipped binary strings | Extract every `` `koto <verb> [<subverb>]` `` token from Rust string literals under `src/`; assert each resolves against the `clap` command tree (obtainable by walking `koto help` recursively, or from the `Subcommand` enums). Fires on `src/cli/mod.rs:3473`, `:3489`, `src/cli/next_types.rs:179`. | **Yes.** Small, deterministic, no false positives on this tree, and self-retiring if `rebind` ever lands. |
| 1b | Three hand-written session-verb lists, three different wrong answers | Same extractor applied to backticked command tokens in `docs/**` and `plugins/**`; or a generated-block marker in the docs that CI regenerates from `--help` and diffs. | **Yes**, and it is nearly free once the instance-1 extractor exists. |
| 2 | koto-author examples that did not compile | Widen `validate-plugins.yml`'s `find` from `*/koto-templates/*.md` to also cover `*/references/examples/*.md`, and add `src/template/**` to the workflow's `paths` trigger. | **Yes — one line plus a trigger path.** The runner already exists. Note this catches only the *compile* half; the "compiles then cycle-detects at runtime" half needs an execution smoke test, which is not cheap. |
| 3 | `CLAUDE.local.md` describes a Go repo | Two checks, both cheap: (a) path-existence for every `path/`-shaped token in a fenced directory tree; (b) the instance-1 command extractor, which fires on `koto transition` and `koto query`. **But neither can run in koto's CI** — the file is generated by niwa from `dot-niwa` and is untracked here. The check has to live where the source lives. | **Cheap to write, hard to place.** This is the honest gap. |
| 3b | `cmd/koto/` and `src/gate/` in the tracked `CLAUDE.md`; orphan `.golangci.yaml` | Path-existence check over fenced directory-tree blocks in tracked markdown, distinguishing `foo/` (must be a dir) from `foo.rs` (must be a file). Would fire on `CLAUDE.md:11` and `:15`. | **Yes.** |
| 4 | shirabe's root-level `koto-templates/` | Same path-existence check over fenced directory trees, run in shirabe. Fires on `CLAUDE.md:261`. | **Yes** — one check covers 3b and 4 identically. |
| 5 | `src/gate/mod.rs`, `docs/template-format.md` | Path-existence check over backticked `path:line` citations in `docs/**`. Fires on `PRD-koto-runs-commands.md:62` and `:109`. The `:206-230` suffix must be stripped before the stat. | **Yes.** Highest-value-per-line check in the set — dangling citations are dense in PRDs and trivially decidable. |

**Where I would be honest about no cheap check existing:**

- **Instance 2's runtime half.** "This template compiles and then cycle-detects on the second poll"
  is not statically decidable. Only executing the workflow finds it. Compile coverage is cheap;
  execution coverage is a different order of investment.
- **Instance 3's placement.** The check is trivial; getting it to run on a file that koto's CI
  cannot see is the real problem, and it is organisational, not technical.
- **Semantic staleness generally.** None of these checks would catch a doc that names a real
  command and describes its behaviour incorrectly. Every instance here is a **naming** failure —
  a token that should resolve to something and does not. That is the boundary of what is cheap.

---

## Implications

**The acceptance test is not just reproducible — it is currently reproducible.** `src/cli/mod.rs`
is byte-identical between `v0.12.0` and `origin/main`, so a check can be developed against HEAD and
proven against the tag with no divergence to reason about. Any check that fires on HEAD fires on
v0.12.0 for the same reason.

**Scope the instance-1 check to Rust string literals, not to all markdown.** The documentation at
v0.12.0 was *right* about `rebind` — `error-codes.md:107`, `command-reference.md:696-698`,
`SKILL.md:201`, `error-handling.md:126`, and `default-action-authoring.md:576` all explicitly say
the subcommand does not exist. The binary was the liar. A check that scanned prose would have
produced a pile of hits on files that were already telling the truth, and the signal would have
drowned. Scanning `src/**/*.rs` string literals gives three hits, all true positives.

**The check must survive a green test suite that asserts the defect.** `tests/execution_anchor_test.rs:400`
requires the message to contain `"rebind"`. This is not an oversight to be embarrassed about — it
is what a conscientious test author writes when the design doc says the verb is coming. It means the
check cannot be a test; it has to be a separate CI stage that reads the clap tree as ground truth.

**Two checks cover four of the five instances.** A path-existence check over fenced directory-tree
blocks and backticked `path:line` citations covers 3b, 4, and 5. A command-token-vs-clap-tree check
covers 1, 1b, and 3's `koto transition` / `koto query`. Both are stat-and-compare against a set the
compiler already knows. Neither needs a model, a heuristic, or a maintained allowlist.

**Instance 2 argues for auditing existing checks before adding new ones.** koto already had a
template-compilation CI job. It was pointed at a glob that matched one file. Before building a new
gate, it is worth asking of each existing gate: what does its glob actually match? The cheapest
win in this whole investigation is a one-line `find` change.

---

## Surprises

**The docs were right and the binary was wrong.** I expected the opposite. Five separate
documentation surfaces at v0.12.0 explicitly flag `koto session rebind` as unimplemented, one of
them with a dedicated section heading (`## koto session rebind — not implemented`). The team knew.
The knowledge was written down in five places. It just never propagated back to the three Rust
string literals that users actually see. Documentation discipline was not the failure mode here —
the failure was that nothing connected the prose to the binary.

**A passing test actively required the defect.** `tests/execution_anchor_test.rs:400-404` asserts
`message.contains("rebind")` with the rationale "the refusal must point at the rebind verb". The
test suite was not silent about the phantom verb; it enforced it. This reframes the core question
of the exploration: the gates were not merely blind, one of them was pointed the wrong way.

**Three documents each enumerate the session verbs, and three of the four lists are wrong** — in
three *different* ways (missing `update`+`recover`, missing `recover`, missing `recover`). Only
`command-reference.md:698`, which is also the file that says "and nothing else", gets it right. A
hand-maintained list of a machine-knowable set rots the moment anyone adds a verb, and nothing
noticed across at least two additions.

**Instance 5's drift was found by a human, documented, and then abandoned.**
`DESIGN-koto-runs-commands.md:113-116` records the correction — "the PRD cites `src/gate/mod.rs`
and `docs/template-format.md`; the live paths are ... Neither changes any conclusion" — and the PRD
was never touched. The judgment that it did not change any conclusion was correct and also beside
the point: the dangling citations still ship, in the document a reader is more likely to open first.
Manual detection without a mechanical fix does not durably fix anything.

**koto's CI already contained the check for instance 2, aimed at one file.** The glob
`plugins/koto-skills/skills/*/koto-templates/*.md` matches two paths in the entire repository, one
of which the script's own `*.mermaid.md` guard then skips. A job named "Compile all templates"
compiled exactly one template while the broken examples sat two directories away.

**The v0.12.0 release binary was already installed on this machine**, which turned a documentary
exercise into a live demonstration. `koto session rebind demo --to /tmp` →
`error: unrecognized subcommand 'rebind'`, from the same binary that prints "run
`koto session rebind ...`". No reconstruction needed.

**`.golangci.yaml` is still tracked in this Rust repo.** A stale Go linter config that no tool in
the build can consume. It is harmless, but it is the same rot as instance 3 sitting in tracked
files where a check *could* see it.

**The Rust line-continuation breaks are load-bearing luck.** In `src/cli/mod.rs:3472-3473`, the
string breaks mid-sentence and `run \`koto session rebind {} --to <dir>\`` happens to land intact
on 3473. Had the author broken after `` `koto session `` instead, a line-oriented extractor would
miss it entirely. Any extractor we build should join Rust string-literal continuations before
matching, rather than relying on where a formatter happened to wrap.

---

## Open Questions

1. **Where does the instance-3 check live?** `CLAUDE.local.md` is generated by niwa from `dot-niwa`
   and is untracked in koto. koto's CI cannot see it. Does the check run in `dot-niwa` against its
   `repos/koto.md` source, in niwa at apply time, or is instance 3 simply out of scope for a
   koto-repo check? This is a scoping decision, not a technical one, and it changes what "catches
   all five instances" means.

2. **Ground truth for the clap tree: parse the enums, or shell out to `--help`?** Walking
   `koto help` recursively requires a built binary in CI (already true for `validate-plugins.yml`)
   and gives exactly what users see. Parsing `Subcommand` enums out of `src/cli/mod.rs` needs no
   build but must handle `#[clap(name = "...")]` renames (`ChildrenPolicy::AcceptRemote` already
   uses one) and must not confuse `ValueEnum`s with `Subcommand`s — a trap I hit by eye while doing
   this analysis.

3. **Do we check `plugins/**` and `docs/**` for command tokens at all, given the docs were right?**
   Scanning them would have produced ~20 hits at v0.12.0, nearly all on lines whose purpose is to
   say the verb does not exist. Any prose-scanning check needs an escape hatch, and escape hatches
   are exactly what rots. Recommendation: start with Rust literals only, and add the docs surface
   only if a second instance argues for it.

4. **What happens when `rebind` lands?** A check keyed on the live clap tree goes green
   automatically, which is right. But the three *documentation* passages saying "this does not
   exist" would then become false, and nothing would catch that — the inverse drift. Is the check
   one-directional by design, or should a documented "not implemented" marker also be verified?

5. **Instance 2's runtime half.** Compile coverage is a one-line fix. Is an execution smoke test for
   shipped example templates in scope for this effort, or is "it compiles" the honest ceiling?

6. **`.golangci.yaml`, `cmd/koto/` in the tracked `CLAUDE.md`** — are these in scope as instance 3b,
   or noise? They are the same defect class, they *are* mechanically checkable, and unlike
   `CLAUDE.local.md` they live where koto's CI can reach them.

---

## Summary

All five instances are reproducible, and instance 1 is not merely reproducible but still live: `src/cli/mod.rs` is byte-identical between `v0.12.0` and `origin/main`, three Rust string literals (`src/cli/mod.rs:3473`, `:3489`, `src/cli/next_types.rs:179`) tell users to run `koto session rebind`, and the installed v0.12.0 binary answers that exact command with `error: unrecognized subcommand 'rebind'` — `SessionCommand` has exactly seven variants and never had an eighth.

The two things that make this fixture sharp: the documentation was already *correct* (five separate files at v0.12.0 explicitly say the subcommand does not exist), so the check must be scoped to Rust string literals rather than prose; and `tests/execution_anchor_test.rs:400` actively asserts `message.contains("rebind")`, meaning the suite did not miss the defect but required it — so the check cannot be a test, it has to be a CI stage reading the clap tree as ground truth.

Two mechanical checks cover four of the five instances — a path-existence check over fenced directory trees and backticked `path:line` citations (catches 3b, 4, 5) and a command-token-versus-clap-tree check (catches 1, the three disagreeing session-verb lists, and instance 3's `koto transition`/`koto query`) — while instance 2 needs no new check at all, only widening `validate-plugins.yml`'s `find` glob, which today matches exactly one template file and misses every shipped example.
