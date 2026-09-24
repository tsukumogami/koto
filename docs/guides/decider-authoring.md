# Declaring decisions a decider can answer

Some workflow states stop only to ask the agent a closed question: is this
issue a bug or a feature, does this change need a design first, is this plan
item clear enough to start. The agent reads a few stored inputs, picks one
value, and submits it. A typed decision model can often answer that kind of
question faster and more cheaply than an agent turn.

A **decider declaration** marks one of those questions in a template. It lives
inside an `accepts` field, says what each value means, and names the inputs
the answer depends on. For users who opt in, koto can send the question to a
decider before stopping the workflow, record what came back, and, only for
values the template has promoted to `auto`, apply the answer and move on
without asking the agent.

This guide covers writing a declaration, how users opt in, what leaves the
machine and what's recorded, and how a value earns promotion to `auto`. The
field-level syntax is also in the koto-author skill's
`plugins/koto-skills/skills/koto-author/references/template-format.md`, and
every compile error is in `docs/reference/error-codes.md`.

## Status of the Jev client

The only decider koto ships is Jev, from TypeSafe
([docs.typesafe.ai](https://docs.typesafe.ai/)). Its default endpoint is
Jev's decision URL, `https://api.typesafe.ai/v1/systemone`.

So far the Jev client has been verified only against a local stub built from
Jev's published API documentation. It hasn't been run against the live API.
Live validation is a pending follow-up. Until it's done, treat a consultation
against the real endpoint as untested, and expect the promotion data you
gather to be the first real test of the client.

## When a decision qualifies

A decision is a good candidate when all of these hold:

- The answer is **one value from a closed set**: an `enum` field or a
  `boolean` field. Free text, numbers, and task lists can't be declared.
- The answer can be judged **from inputs koto already holds**: context-store
  keys and template variables (declared ones or `capture_stdout_as` names).
  A declaration can't read files, environment variables, or runtime names
  like `SESSION_DIR`.
- The state asks **one question**. koto sends every declared field on a state
  in a single consultation and applies an answer only when every declared
  field qualifies, so a state that mixes one easy question with a hard one
  gets no help on either. Give each question its own state. A state with a
  declared field can't also have a required field without a declaration;
  make such a field optional (like a free-text `notes` field) or declare it
  too.

If the agent needs to do work first (run tests, read a diff, investigate),
the question isn't ready for a decider. Have an earlier state produce the
facts, store them under a context key, and ask about the stored facts.

## A complete example

The template below fetches an issue into the context store, asks what kind of
change it is (an `enum` declaration), then asks whether it needs a design
document (a `boolean` declaration). It compiles as written.

```yaml
---
name: triage
version: "1.0"
description: Classify an issue, then decide whether it needs a design first
initial_state: fetch
variables:
  ISSUE:
    description: Issue number to triage
    required: true
states:
  fetch:
    default_action:
      command: "gh issue view {{ISSUE}} --json title,body --jq '.title + \"\\n\\n\" + .body' | koto context add {{SESSION_NAME}} issue.md"
    gates:
      issue:
        type: context-exists
        key: issue.md
    transitions:
      - target: classify
        when:
          gates.issue.exists: true
  classify:
    accepts:
      kind:
        type: enum
        values: [bug, feature, chore]
        required: true
        description: What kind of change does this issue ask for?
        decider:
          answers:
            bug:     {description: "Reports behavior that differs from what's documented or intended."}
            feature: {description: "Asks for behavior that doesn't exist yet."}
            chore:   {description: "Maintenance with no user-visible change, such as dependencies, CI, or refactoring.", mode: never}
          escape: {value: unclear, description: "The text is empty, truncated, or fits none of the kinds."}
          inputs:
            - {context: issue.md, label: issue, max_bytes: 6000}
      notes:
        type: string
        required: false
    transitions:
      - target: reproduce
        when:
          kind: bug
      - target: scope
        when:
          kind: feature
      - target: scope
        when:
          kind: chore
  reproduce:
    transitions:
      - target: scope
  scope:
    accepts:
      needs_design:
        type: boolean
        required: true
        description: Does this change need a design document before anyone implements it?
        decider:
          answers:
            true:  {description: "Touches several components, changes a public interface, or leaves open questions."}
            false: {description: "A contained change whose implementation is obvious from the issue.", threshold: 0.95}
          inputs:
            - {context: issue.md, label: issue, max_bytes: 6000}
    transitions:
      - target: design
        when:
          needs_design: true
      - target: implement
        when:
          needs_design: false
  design:
    transitions:
      - target: done
  implement:
    transitions:
      - target: done
  done:
    terminal: true
---
```

The directive sections (`## fetch`, `## classify`, and so on) are left out
here; a real template needs one per state.

### The enum declaration

The field's `description` is the question. The `decider` block has three keys:

- `answers` has exactly one entry per value in `values`, keyed by value. Each
  entry has a `description` (required), a `mode`, and a `threshold`.
- `escape` is required on an enum: a `value` and a `description` for the
  answer "this can't be judged from the inputs". The escape value must not be
  in `values`, no `when` clause may route on it, and an agent can't submit
  it. It exists so the decider has somewhere to go besides a wrong guess.
- `inputs` lists at least one input. Each names exactly one of `context: <key>`
  or `var: <NAME>`, a `label` that's unique within the field, and an optional
  `max_bytes`.

A `context` input has to be a key that some `context-exists` or
`context-matches` gate in the template checks. The compiler can't see what a
`default_action` writes, so the usual pattern is the one in `fetch` above: the
state that produces the key gates on it, and a run can't reach the question
without it.

### The boolean declaration

A boolean field works the same way, with two differences. Its `answers` keys
are `true` and `false` (bare YAML `true:` and `false:` are fine), and it takes
no `escape`. The escape is implied: `true` wins when P(true) meets the `true`
threshold, `false` wins when P(false) meets the `false` threshold, and
anything else, including both or neither meeting their thresholds, counts as
the escape.

### Defaults

Defaults are resolved when the template compiles. Writing one explicitly
compiles to the same thing as leaving it out.

| Setting | Default | Allowed |
|---------|---------|---------|
| an answer's `mode` | `shadow` | `off`, `shadow`, `auto`, `never` |
| an answer's `threshold` | `0.9` | 0.5 to 1.0 inclusive |
| an input's `max_bytes` | `8192` | above 0 |

An input over its budget isn't truncated. koto skips the provider call for
that visit, records the consultation as `input_unavailable`, and the state
stops for the agent as usual. Pick a budget that fits the real content.

## Modes

Each value has its own mode:

- `off`: the value is never consulted for. If every value on a state is
  `off`, koto doesn't consult on that state at all.
- `shadow`: koto consults and records the answer but never applies it. The
  agent answers as if nothing happened.
- `auto`: when the decider picks this value at or above its threshold, and
  every other condition holds, koto applies it and advances.
- `never`: like `shadow`, but it can't be promoted by config. It says "ask,
  record, never act", and it's the mode for a value whose wrong outcome would
  cost more than an agent turn.

The mode that actually applies is the lowest of three, in the order
`off` < `shadow` < `auto`:

1. the user's global mode, from `KOTO_DECIDER` or else `decider.mode` in
   `~/.koto/config.toml` (default `off`);
2. the project's `decider.mode` in `.koto/config.toml`, when it sets one;
3. the template's mode for that value (`never` counts as `shadow` here and is
   never applied).

So a template can't turn a decider on for anyone, and a project can only turn
it down.

### When koto applies an answer

A value in `auto` is applied only when all of these hold on the visit:

- every declared field on the state has a winning value in `auto` at or above
  its threshold, and none of them is the escape;
- the combined answer matches exactly one conditional transition, and its
  target hasn't already been visited in this `koto next` call;
- that transition passes the floor (below) at run time as well as at compile
  time.

Anything short of that applies nothing, and the agent gets exactly the
response it would have got without a decider. koto consults only when the
state would otherwise stop for evidence and no gate on it failed, at most once
per visit, and at most four times per `koto next`. The agent never sees the
decider's answer, and evidence the agent submits always wins.

### The floor

Some routes should never be taken on a model's word. An answer in `auto` can't
take a transition that:

- targets a terminal state;
- targets a state whose `default_action` has `requires_confirmation: true`; or
- has a `when` clause that also tests a `gates.*` key.

The compiler checks every transition whose `when` tests the field at an `auto`
value, and refuses the template with `E-DECIDER-FLOOR` if one breaks the rule.
Nothing in the template relaxes it. Here's a declaration that promotes both
values of a duplicate check:

```yaml
dedupe:
  accepts:
    verdict:
      type: enum
      values: [duplicate, new]
      required: true
      description: Does this issue repeat one that's already open?
      decider:
        answers:
          duplicate: {description: "Asks for the same change as an open issue.", mode: auto}
          new:       {description: "Asks for something no open issue covers.", mode: auto}
        escape: {value: unclear, description: "Can't tell from the text."}
        inputs:
          - {context: issue.md, label: issue}
  transitions:
    - target: closed
      when:
        verdict: duplicate
    - target: work
      when:
        verdict: new
```

`closed` is terminal, so compiling fails:

```
E-DECIDER-FLOOR: state "dedupe" field "verdict" value "duplicate": mode auto is not allowed on the transition to "closed": the target is a terminal state
  remedy: set this value's mode to shadow or never; an auto answer can't route to a terminal state, to a state whose default_action requires confirmation, or along a when clause that tests a gate
```

Setting `duplicate` to `never` fixes it. `new` can stay in `auto`, because
`work` isn't terminal. If you want `duplicate` to be promotable later, route
it through a non-terminal state (a `close_duplicate` state that does the
closing, say) instead.

## Opting in

The decider is off by default. A user opts in with three things, and a
project can't supply any of them except a lower mode.

| Key | Env override | Default | Set in project config? |
|-----|--------------|---------|------------------------|
| `decider.mode` | `KOTO_DECIDER` | `off` | yes, and it can only lower the mode |
| `decider.api_key` | `KOTO_DECIDER_API_KEY` | none | no |
| `decider.endpoint` | `KOTO_DECIDER_ENDPOINT` | `https://api.typesafe.ai/v1/systemone` | no |
| `decider.timeout_ms` | none | `2000` (1 to 10000) | no |

```bash
koto config set --user decider.api_key "$JEV_API_KEY"
koto config set --user decider.mode shadow
```

The rules that decide whether a user is opted in:

- **A key alone opts nobody in.** The effective mode has to be `shadow` or
  `auto` as well, and with no key there's no consultation, whatever the mode.
- **Project config can only lower the mode.** A checked-in
  `.koto/config.toml` may set `decider.mode`, and the lower of it and the
  user's mode wins. koto ignores `api_key`, `endpoint`, and `timeout_ms` in
  project config and prints a warning, so a repository can't supply a key or
  point your key at its own server. `koto config set` refuses those keys
  without `--user`.
- **The key and the endpoint come from the same place.** A key from
  `KOTO_DECIDER_API_KEY` is sent only to an endpoint from
  `KOTO_DECIDER_ENDPOINT` or the default. A key from user config is sent only
  to an endpoint from user config or the default. Any other pairing leaves
  the user opted out, with a warning. That stops a command like
  `KOTO_DECIDER_ENDPOINT=<host> koto next` from carrying a stored key to
  another host.
- **Endpoints must be https.** Plain `http` is accepted only for a loopback
  host: an IP in 127.0.0.0/8, `::1`, or exactly `localhost`. koto never
  resolves a name to decide it's loopback. An endpoint with a username or
  password in it is refused.
- `decider.mode` in config takes `off`, `shadow`, or `auto`. `never` is
  template-only. An unrecognized mode means `off`, with a warning.
- An empty env variable counts as unset. `KOTO_DECIDER=off` turns the decider
  off for one command.

`koto config get decider.api_key` and `koto config list` print `<set>` instead
of the key. `koto config set` writes `~/.koto/config.toml` with mode 0600.

## What leaves the machine

For an opted-in user, one consultation is one HTTPS `POST` to the endpoint,
authenticated with the API key. It carries:

- each declared field's name and question;
- each value's name and description, and the escape's;
- each input's label and content, already within its `max_bytes`.

Nothing else from the session is sent: no other context keys, no directive
text, no environment, no file contents. Opting in applies to every template
the user runs that carries a declaration, not just yours, which is why the
authoring guidance below asks for narrow inputs.

## What's recorded

Each consultation appends a `decider_consulted` event to the session log. It
holds the state and visit, the provider and model, a SHA-256 of the assembled
inputs, each field's declaration hash, effective modes, probabilities, winning
value, confidence, threshold, and outcome, plus the overall outcome, latency,
and where the endpoint came from. An applied answer is followed by an
`evidence_submitted` event marked `source: "decider"`. The event format is in
`docs/reference/session-feed.md`.

koto also appends a `consulted` record to `~/.koto/_decider_ledger.jsonl`,
and, when the agent later answers a visit koto didn't settle, an `answered`
record with the agent's declared values. The ledger is created with mode
0600, is never synced to the cloud backend, and survives session cleanup and
`koto workspace prune`; see `docs/workspace-layout.md`. If a ledger write
fails, koto prints a warning and carries on.

Neither the event nor the ledger holds input content, the API key, or the
provider's response body. Inputs appear only as their hash.

## Promoting a value to `auto`

Every value ships in `shadow` or `never`. A value moves to `auto` only when a
maintainer edits its `mode` in the template, and only after the report says
the evidence supports it. Nothing promotes a value automatically, and
`koto decider report` never changes a mode.

1. **Ship in shadow.** Release the template with every value in `shadow` (or
   `never` for values you'll never promote). Opted-in users build up paired
   observations in their ledgers: what the decider said, and what the agent
   then chose.
2. **Write a golden fixture file.** One JSON object per line, with `inputs`
   (each declared input label mapped to its text), `expected` (a value, the
   escape, or JSON `true`/`false` for a boolean), and an optional `id`:

   ```
   {"id": "k1", "inputs": {"issue": "Crash on empty config\n\nkoto panics when config.toml is empty."}, "expected": "bug"}
   {"id": "k2", "inputs": {"issue": "Add --dry-run to sync\n\nShow what would change without writing."}, "expected": "feature"}
   ```

   Aim for real cases, labelled by someone who knows the right answer.
3. **Run the report**, opted in:

   ```bash
   koto decider report --fixtures triage.classify.kind.jsonl --template triage.md --state classify
   ```

   Add `--field <field>` when the state declares more than one field. The
   fixture run needs an opted-in decider and network access; without opt-in
   it says so, sends nothing, and exits 2.
4. **Promote what's eligible.** If the report marks a value
   promotion-eligible, change that value's `mode` to `auto`, recompile (the
   floor still applies), and send the change for review like any other.

A value is eligible when all of these hold:

- the fixture file has at least 10 cases labelled with that value (the escape
  needs none; a boolean needs 10 each for `true` and `false`) and at least 40
  cases in total;
- every fixture case got an answer;
- no fixture labelled with another value was answered with this value at or
  above its threshold, so there are no false positives for it;
- macro recall across the values beats always guessing the most frequent
  label;
- the ledger holds at least 30 paired observations under the current
  declaration hash, with at most one disagreement where the decider chose
  this value;
- the fixture run and the counted consultations used the default endpoint
  (pass `--include-custom-endpoints` to count others, for example when you're
  testing against a stub).

The full report, its JSON shape, and its exit codes are in
[cli-usage.md](cli-usage.md#decider-report).

### What resets the evidence

The ledger groups evidence by a **declaration hash**, computed per field
from:

- the question (the field's `description`);
- the values;
- every value's description;
- the escape's value and description;
- the inputs: each one's source, label, and `max_bytes`, in order.

Modes and thresholds aren't part of it. That's deliberate: promoting a value
from `shadow` to `auto`, or tightening its threshold, keeps the evidence that
justified the change. Rewording a description, adding a value, or widening an
input's budget does change the hash, and the report treats it as a new
question with no history. Reordering `values` doesn't. Settle the wording
before you start collecting, and expect a reworded question to start over.

## Authoring guidance

- **Put `auto` only on answers whose wrong outcome costs a reversible step.**
  A misrouted issue that someone moves back later is fine. Closing,
  publishing, or anything a user can't undo isn't, and the floor only catches
  the structural cases.
- **Inputs carrying externally authored text are weaker promotion
  candidates.** An issue body, a PR description, or a comment can contain
  text written to steer the decider. In `shadow` and `never` the worst case is
  misleading data; in `auto` a steered answer routes the workflow. Agreement
  with agents doesn't prove resistance, since both read the same text. Prefer
  inputs your own tooling produced, like a list of changed paths or a
  computed fact.
- **Declare the narrowest context key that answers the question.** Opted-in
  users send declared inputs to a third-party provider. If the question needs
  an issue's title and first paragraph, store those under their own key
  instead of the whole thread, and set `max_bytes` to fit.
- **Write value descriptions for both readers.** The agent sees them in
  `expects.fields.<field>.value_descriptions` and the decider receives them as
  its criteria. A description that helps one helps the other.

## Compatibility with older koto

Everything a declaration adds lives inside the field, and koto v0.12.2 drops
the `decider` block when it parses a field. A template with declarations
compiles and runs there exactly as it does with the blocks removed, so you
can add declarations without raising the koto version your users need. A
template with no `decider` block compiles byte for byte as before and keeps
its `template_hash`.
