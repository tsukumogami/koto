# Lead: How do other workflow engines model parent-child workflow relationships?

## Findings

### Temporal

Temporal's child workflow model is the most mature among the engines surveyed. Key characteristics:

**Relationship model: external reference, spawned inline.** A child workflow is a full Workflow Execution spawned from within a parent workflow in the same Namespace. Children use their own workflow type definitions -- they aren't defined inline within the parent template. The parent code calls `executeChildWorkflow()` with a workflow type and inputs. ([docs.temporal.io/child-workflows](https://docs.temporal.io/child-workflows))

**Result propagation: awaitable future.** The parent receives a handle to the child and can optionally `await` its result. Results are returned as typed values. Parent and child share no local state -- communication is limited to asynchronous Signals. ([docs.temporal.io/child-workflows](https://docs.temporal.io/child-workflows))

**Failure semantics: configurable Parent Close Policy.** Three options per child:
- **Terminate** (default): child is forcefully stopped when parent closes
- **Request Cancel**: child receives a cancellation request and can shut down gracefully
- **Abandon**: child continues running independently after parent closes

Each child can have its own policy. A child failure surfaces as a `ChildWorkflowFailure` in the parent with structured error info including workflow type and ID. ([docs.temporal.io/parent-close-policy](https://docs.temporal.io/parent-close-policy))

**Fan-out/fan-in:** Parents can spawn up to ~1,000 children (recommended limit). The pattern is straightforward: spawn N children, collect futures, aggregate results. Hierarchical fan-out is possible (parent -> 1k children -> 1k grandchildren each). ([community.temporal.io](https://community.temporal.io/t/long-running-workflow-with-significant-fan-out-of-child-workflows/17975))

**Parent context access:** Children cannot access parent state. All data must be passed explicitly as input parameters or exchanged via Signals.

### Apache Airflow

Airflow has gone through two generations of sub-workflow modeling, and the evolution is instructive.

**SubDagOperator (deprecated).** Launched a separate DAG as a child, monitored it externally. The child DAG's ID was conventionally prefixed with `parent.child`. This caused significant performance and edge-case issues because the child was a fully independent execution with its own scheduler interactions. Airflow deprecated this in favor of TaskGroup. ([cwiki.apache.org/confluence/display/AIRFLOW/AIP-34](https://cwiki.apache.org/confluence/display/AIRFLOW/AIP-34+TaskGroup:+A+UI+task+grouping+concept+as+an+alternative+to+SubDagOperator))

**TaskGroup (current).** A purely UI-level grouping concept. All tasks remain in the same DAG -- TaskGroup doesn't create a separate execution. It's modeled as a tree where children can be tasks or other TaskGroups. IDs are prefixed with parent group IDs for uniqueness. Dependencies can be set between TaskGroups and individual tasks. ([airflow.apache.org/docs/stable/concepts/dags.html](https://airflow.apache.org/docs/apache-airflow/stable/concepts/dags.html?highlight=taskgroup))

**TriggerDagRunOperator (cross-DAG).** For true parent-child DAG relationships, Airflow uses TriggerDagRunOperator. It can trigger another DAG and optionally wait for completion (`wait_for_completion=True`). Data passes via `conf` parameter (key-value dict) and XCom for lightweight results. ([astronomer.io/docs/learn/cross-dag-dependencies](https://www.astronomer.io/docs/learn/cross-dag-dependencies))

**Failure semantics:** When using TriggerDagRunOperator with `wait_for_completion`, the parent task fails if the triggered DAG fails. Without waiting, the parent has no visibility into child outcomes.

**Key lesson from Airflow:** The SubDag approach (separate execution tracked by parent) was abandoned due to complexity. The replacement went in two directions: flatten into same execution (TaskGroup) or explicit cross-execution triggering (TriggerDagRunOperator). There's no middle ground.

### Argo Workflows

Argo provides multiple mechanisms for workflow composition:

**WorkflowTemplate references.** Templates from one WorkflowTemplate can be referenced via `templateRef` from steps or DAG templates in another workflow. This is the inline reference model -- the child template runs within the parent workflow's execution. ([argo-workflows.readthedocs.io/en/latest/workflow-templates/](https://argo-workflows.readthedocs.io/en/latest/workflow-templates/))

**Workflow of Workflows pattern.** Uses a `resource` template to submit a new Workflow CR (Kubernetes custom resource) as a child. The parent workflow creates and monitors the child as a separate Kubernetes object. The child workflow runs independently with its own execution history. Traceability exists in both directions -- parent references child, child references parent. ([argo-workflows.readthedocs.io/en/latest/workflow-of-workflows/](https://argo-workflows.readthedocs.io/en/latest/workflow-of-workflows/))

**Result propagation:** Within a single workflow, outputs flow via `{{steps.<NAME>.outputs.result}}` or `{{tasks.<NAME>.outputs.result}}`. For workflow-of-workflows, results require reading the child Workflow CR's status after completion.

**Fan-out:** DAG templates enable parallel task execution with dependency declarations. Steps templates use a list-of-lists structure (outer = sequential, inner = parallel). `withItems` and `withParam` enable dynamic fan-out over collections.

**Failure semantics:** Within a workflow, step/task failure follows DAG dependency rules. For workflow-of-workflows, the resource template monitors the child CR's status and surfaces failure to the parent step.

### Prefect

Prefect models subflows as nested function calls:

**Relationship model: inline invocation.** A subflow is simply a flow function called from within another flow function. The parent-child connection is tracked through a special task run in the parent that represents the child flow. `state_details` contains `child_flow_run_id` in the parent and `parent_task_run_id` in the child. ([docs.prefect.io/v3/concepts/flows](https://docs.prefect.io/v3/concepts/flows))

**Result propagation: return values.** The child flow's return value is available to the parent as a normal Python return value. Task futures passed from parent to child are automatically resolved into concrete data.

**Failure semantics: exception-based.** If a child raises an exception, it fails. The parent can catch it with try/except. A parent can complete successfully even if a child failed, if it doesn't propagate the child's failed state. There's no equivalent of Temporal's Parent Close Policy. ([github.com/PrefectHQ/prefect/issues/9193](https://github.com/PrefectHQ/prefect/issues/9193))

**Fan-out:** Async subflows can run concurrently via `asyncio.gather` or AnyIO task groups. Synchronous subflows block.

**Parent context access:** Children create their own task runner and execution context. No implicit access to parent context -- data must be passed as arguments.

### Netflix Conductor

Conductor's SUB_WORKFLOW task type is the most explicit about the contract:

**Relationship model: external reference by name + version.** The parent workflow definition contains a SUB_WORKFLOW task that references a child workflow by name and optional version (defaults to latest). The child must have a pre-existing definition in Conductor. ([orkes.io/content/reference-docs/operators/sub-workflow](https://orkes.io/content/reference-docs/operators/sub-workflow))

**Result propagation: task output.** The sub-workflow task's output includes both the `subWorkflowId` (execution ID for traceability) and the child workflow's output data. The parent task completes when the child workflow completes.

**Failure semantics: optional tolerance.** The `optional` flag allows a parent to continue even when a sub-workflow fails, with the parent completing with status `COMPLETED_WITH_ERRORS`. Without this flag, child failure = parent task failure, subject to retry configuration.

**Parent context access:** Children cannot access parent context directly. All data flows through explicit `inputParameters` mapping. Inputs can reference parent workflow inputs or outputs from preceding tasks.

### AWS Step Functions

Step Functions offers two distinct fan-out models:

**Nested workflows.** A task state can start another state machine execution using `states:startExecution.sync`. The parent waits for the child to complete. Data passes via the child's input JSON. ([docs.aws.amazon.com/step-functions/latest/dg/concepts-nested-workflows.html](https://docs.aws.amazon.com/step-functions/latest/dg/concepts-nested-workflows.html))

**Map state (fan-out).** Two modes:
- **Inline mode** (default): up to 40 concurrent iterations within the same execution
- **Distributed mode**: each iteration runs as a separate child workflow execution, up to 10,000 parallel children, each with its own execution history

([docs.aws.amazon.com/step-functions/latest/dg/state-map.html](https://docs.aws.amazon.com/step-functions/latest/dg/state-map.html))

**Result aggregation:** Map state collects results from all iterations into an array, returned to the parent as a single output.

---

### Cross-Engine Pattern Comparison

| Dimension | Temporal | Airflow | Argo | Prefect | Conductor | Step Functions |
|-----------|----------|---------|------|---------|-----------|----------------|
| **Child definition** | External type | External DAG / inline TaskGroup | External template or inline ref | Inline function call | External by name+version | External state machine |
| **Execution isolation** | Separate execution, same namespace | Separate (TriggerDag) or same (TaskGroup) | Same (templateRef) or separate (resource) | Separate flow run, same process | Separate execution | Separate (nested) or same (inline Map) |
| **Result flow** | Typed future/await | XCom / conf dict | Output parameters / CR status | Python return value | Task output JSON | JSON output / Map array |
| **Parent close policy** | Terminate / Cancel / Abandon (per-child) | None (fire-and-forget or wait) | None explicit | None | None | None |
| **Failure tolerance** | ChildWorkflowFailure exception | Task failure propagation | Step/task failure | Exception-based | `optional` flag -> COMPLETED_WITH_ERRORS | Catch + retry |
| **Context sharing** | Signals only, no shared state | XCom (limited) | Parameters only | Function arguments only | inputParameters only | JSON input only |
| **Fan-out limit** | ~1,000 recommended | No built-in limit | DAG/withItems | asyncio concurrency | No documented limit | 10,000 (distributed Map) |

### Common Patterns

1. **No shared state.** Every engine requires explicit parameter passing between parent and child. No engine provides implicit context inheritance.

2. **External definition is dominant.** Most engines reference children by name/type/ID, not by inline definition. Prefect is the exception (inline function calls), but even there the child is a separately defined flow function.

3. **Two isolation levels.** Every engine that matured past v1 offers both "same execution" (lightweight grouping) and "separate execution" (full isolation) modes. Airflow learned this the hard way -- SubDag tried to be both and failed.

4. **Result propagation is always explicit.** Whether it's a typed future (Temporal), output parameter (Argo), return value (Prefect), or task output (Conductor), the parent must explicitly request and process child results.

5. **Failure defaults to propagation.** In every engine, child failure causes parent task failure by default. Tolerance is opt-in: Temporal's Parent Close Policy, Conductor's `optional` flag, Prefect's try/except, Step Functions' Catch states.

6. **Bidirectional traceability.** Parent knows child ID; child knows parent ID. This is universal.

## Implications

**For koto's design**, the prior art strongly suggests:

1. **Children should be external templates, not inline definitions.** Every successful engine uses this model. Koto's existing template system maps naturally to this -- a child workflow uses its own template file, referenced by name from the parent.

2. **No implicit context sharing.** Parent-to-child data should flow through explicit input parameters at `koto init --parent` time. Children should not be able to reach into parent state. This aligns with koto's existing design where each workflow has its own state file.

3. **The Parent Close Policy concept from Temporal is the most relevant failure model.** Koto needs to decide what happens to children when a parent completes or fails. The three options (terminate, cancel, abandon) map well to different use cases. For the "parent agent manages child agent lifecycle" scenario described in the exploration context, Abandon is likely the right default -- the parent agent decides when to stop children, koto doesn't force it.

4. **Result propagation should use the gate-transition contract.** Since koto already has structured gate output (v0.6.0), child workflow outcomes could be exposed as gate-queryable state. A parent gate condition like `child-state: {workflow: "child-1", state: "completed"}` would let the parent's state machine react to child progress without polling.

5. **Fan-out needs bounded iteration (#105), not a built-in Map state.** Koto's role as a contract layer means the parent agent handles spawning. Koto just needs to track N child workflow IDs and expose their collective state for gate evaluation.

6. **Airflow's SubDag cautionary tale applies directly.** Trying to make children run "inside" the parent's state machine would recreate SubDag's problems. Children must be fully separate workflow instances with their own state files.

## Surprises

1. **Temporal's Parent Close Policy is unique.** No other engine has a formal per-child policy for what happens when the parent closes. Most engines either propagate failure or don't -- there's no middle ground. This is the one area where Temporal's design is meaningfully more sophisticated than the rest. For koto, this matters because the parent agent may legitimately complete while children should continue (Abandon) or the parent may fail and children should be marked stale (Terminate equivalent).

2. **Conductor's `COMPLETED_WITH_ERRORS` status is underappreciated.** Most engines force a binary: either the child failure propagates or the parent catches it. Conductor's approach of completing the parent with a degraded status is a useful middle ground that could map well to koto's evidence model -- "this workflow completed but child X failed, here's what we got."

3. **Airflow's evolution is the strongest cautionary signal.** SubDagOperator tried to treat child DAGs as both "part of the parent" and "independent executions" simultaneously. This created an impossible maintenance burden and was deprecated. The lesson: pick one model (inline grouping OR separate execution) and commit to it. Don't try to be both.

4. **No engine supports bidirectional data flow during execution.** Parent-to-child data flows at spawn time only. Child-to-parent data flows at completion only. Mid-execution communication is limited to Temporal's Signals (async, event-based). This suggests koto shouldn't try to build real-time parent-child state synchronization -- periodic state queries are sufficient.

## Open Questions

1. **What should koto's equivalent of Parent Close Policy be?** The parent agent manages child lifecycle, but koto needs to know what child state means when a parent reaches a terminal state. Should koto mark abandoned children? Leave them running? Record the orphan relationship?

2. **How should child evidence flow into parent gates?** Should it be a new gate type (`child-state`), an extension of `context-exists`/`context-matches`, or something entirely different? The gate contract needs to support "all N children completed" and "child X produced evidence Y" patterns.

3. **Should child workflows inherit any parent metadata?** Every engine says "no shared state," but most do propagate a parent ID for traceability. Should koto state files include a `parent_workflow_id` field? Should `koto query` show the parent-child tree?

4. **How does this interact with #105 (bounded iteration)?** If a parent spawns 10 children for 10 issues, the parent needs to track which children map to which iteration items. Is this koto's responsibility or the agent's?

5. **What happens to `koto rewind` in a parent-child context?** If a parent rewinds past the state where children were spawned, what should happen to those children? Temporal's answer is "nothing, they're separate executions." Is that sufficient for koto?

## Summary

Every major workflow engine converges on the same core model for parent-child workflows: children are externally defined, share no implicit state with parents, propagate results explicitly at completion, and fail the parent by default with opt-in tolerance. The most relevant design decision for koto is Temporal's Parent Close Policy (terminate/cancel/abandon per child), which is the only engine that formally addresses what happens to children when a parent's lifecycle ends -- a question that maps directly to koto's "parent agent manages child agent lifecycle" requirement. The biggest open question is how child workflow outcomes should integrate with koto's existing gate-transition contract, since that's where the prior art diverges most from koto's unique position as a contract layer rather than an execution engine.
