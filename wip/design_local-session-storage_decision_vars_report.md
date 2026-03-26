<!-- decision:start id="substitute-vars-signature" status="assumed" -->
### Decision: substitute_vars signature

**Context**

The local session storage design specifies a `substitute_vars(input: &str, vars: &HashMap<String, String>) -> String` utility that replaces `{{KEY}}` tokens in gate commands and directives. For the initial feature, the map contains exactly one entry: `SESSION_DIR`. A pragmatic reviewer questioned whether the HashMap is YAGNI when a single `str::replace("{{SESSION_DIR}}", &session_dir)` at each call site would suffice.

The question reduces to: does YAGNI apply when the abstraction costs ~8 extra lines and the known consumer (issue #67, `--var` support) is already designed and accepted? The variable infrastructure is partially built -- `VariableDecl` exists in the template types, `WorkflowInitialized` already carries a `variables: HashMap<String, Value>` field -- but the substitution function hasn't been written yet.

**Assumptions**

- Issue #67 (--var) is closed and will ship soon, not abandoned. If wrong: the HashMap still works correctly for a single entry with no performance or readability penalty.

**Chosen: HashMap<String, String> parameter (as designed)**

Keep the design as specified: a `substitute_vars` function that takes `&HashMap<String, String>` and iterates over entries to replace `{{KEY}}` tokens. The caller in `handle_next` builds the map with one entry (`SESSION_DIR -> backend.session_dir(name)`) and passes it to the function for both gate command resolution and directive text resolution.

This is ~10 lines of code for the function plus two call sites that construct and pass the map. The function lives in `src/cli/vars.rs` as a standalone utility.

**Rationale**

The YAGNI argument breaks down when three conditions hold simultaneously: the abstraction cost is near-zero, the consumer is already designed, and that consumer's acceptance criteria explicitly require the abstraction. All three apply here.

Issue #67's acceptance criteria state: "The substitution function is exposed as a reusable internal API, not inlined into gate evaluation." Writing the inline version means writing code that you know violates an accepted design, then refactoring it to match. The 8-line delta between the two approaches is smaller than the refactor diff would be.

The HashMap also matches the existing data model. `WorkflowInitialized` already stores `variables: HashMap<String, Value>`. A substitution function that takes `HashMap<String, String>` creates a natural bridge between stored variables and runtime resolution.

**Alternatives Considered**

- **Inline str::replace**: Each call site does `input.replace("{{SESSION_DIR}}", &session_dir)` directly. Rejected because the savings are ~8 lines and the cost is a guaranteed refactor when --var ships. The refactor touches gate evaluation and directive serialization across multiple files, and the inline approach contradicts issue #67's explicit requirement for a reusable substitution API.

**Consequences**

- `src/cli/vars.rs` is created with the `substitute_vars` function from the start.
- Call sites in `handle_next` construct a `HashMap` for one entry, which looks slightly over-engineered in isolation. A code comment noting --var extension prevents confusion.
- When --var ships, user variables merge into the same map with zero changes to `substitute_vars` or its call sites.
- No second substitution path is needed for any future variable source (built-in, user-provided, or otherwise).
<!-- decision:end -->
