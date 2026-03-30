# Lead: How do other workflow engines handle gate bypass audit trails?

## Findings

### GitHub Actions & Environment Protection Rules

GitHub provides a concrete model for gate override audit trails:
- **Bypass capability**: Administrators can bypass environment protection rules via "break glass" functionality, but only on public repositories for most plans
- **Audit logging**: All bypasses are logged in the organization audit trail, creating an immutable record
- **Queryability**: The audit log captures deployment protection rule events (creation, update, deletion, bypass)
- **Pattern**: Bypass is logged as a distinct event type separate from normal deployments, making it queryable as a first-class event

This establishes that enterprise systems treat override as an event worth tracking separately from the normal execution path.

### CI/CD and SOX Compliance Standards

Compliance frameworks define what override audit trails must capture:
- **Required data**: Approver ID and role, timestamp, bypassed controls, risk classification, exception type
- **Rationale**: Exception records must include the reason for override
- **Time-boxing**: Exceptions must have owner, scope, and closure criteria, or they become "bypass debt"
- **Segregation of Duty**: Approvals must come from authorized personnel separate from those performing the action
- **Immutability**: Audit trails must be tamper-evident and append-only

Key insight: Compliance doesn't just require logging that an override happened—it requires capturing *why*, with enough specificity to allow later audits to assess whether the override was justified.

### Workflow Pattern Standards

Multiple enterprise workflow engines converge on a common approval override pattern:
- **Management Approval Pattern** (SAP Signavio, Camunda): A user task captures the approval decision, followed by an exclusive gateway that routes based on approval/rejection
- **Override placement**: Override logic is explicitly placed in the rejection path, making it a second-level decision with different rules/approvers than the initial gate
- **Decision trail**: When a request is approved (or overridden), the decision, decider, and rationale are all logged together
- **Queryability goal**: "If someone asks why this was approved six months later, you should answer in 30 seconds by pulling up the decision record"

This shows that established patterns make override approvals explicit steps, not implicit consequences of having evidence.

### Event Sourcing as Infrastructure

Event sourcing systems provide a model for making audit trails queryable and actionable:
- **Append-only events**: All state changes are recorded as immutable events in a sequence
- **Complete history**: The event log is the source of truth; you can reconstruct any past state by replaying events
- **Queryable events**: CQRS (Command Query Responsibility Segregation) pattern uses read models/projections—views that consume events and make them queryable by business logic (e.g., SQL queries against a "decisions" table)
- **Redo/replay**: Event sourcing enables undo/redo and test-time replay, allowing you to "rewind and replay just like a debugger"
- **Temporal queries**: You can determine the state of a system at any point in history

Temporal and Cadence workflows implement this model: they maintain persistent logs of every decision step as queryable events, with audit trails that can be reviewed or handed to regulators.

### AI Agent Audit Trails (Emerging Pattern)

A new challenge is emerging with agentic AI systems, directly relevant to koto's use case:
- **The transparency gap**: "Agentic AI often does not offer human-readable reasoning unless explicitly programmed"
- **Why matters more**: "It is no longer sufficient to answer 'Who did what?'—one must also answer why"
- **Rationale capture**: Effective audit logs capture the agent's reasoning, the prompt that led to action, and the knowledge sources used
- **Step-by-step logic**: Demonstrating step-by-step reasoning proves the agent followed a defensible process, not arbitrary decisions
- **Minimum capture**: User identity, timestamp, full prompt, knowledge sources with references, response, tool calls executed, and agent rationale
- **GDPR/regulatory**: Under GDPR and similar regulations, organizations must demonstrate lawful basis and how automated decisions were made

This directly applies to koto's override scenario: recording "the AI agent bypassed gate X because..." requires capturing the agent's reasoning, not just the fact of override.

### Argo Workflows & Approval Gates

Argo Workflows uses a suspend template pattern for approval gates:
- **Pause mechanism**: Workflows pause at a Suspend node, allowing human input
- **Parameter capture**: Users can provide input text, choose from dropdowns, or update workflow parameters before resuming
- **Logging approach**: Comprehensive logging via ELK (Elasticsearch, Logstash, Kibana) or Fluentd for centralized analysis
- **Limitation**: While Argo supports logging, the search results do not show a built-in "capture override rationale" feature—this appears to be a gap or requires custom implementation

### Jenkins Audit Plugins

Jenkins provides audit trail plugins that log who performed operations:
- **Audit Trail Plugin**: Logs Jenkins operations (configure, create, delete jobs) with configurable destinations (disk, syslog, Elasticsearch, console)
- **Audit Log Plugin**: Covers build lifecycle, node lifecycle, login/logout, item lifecycle events
- **Limitation**: Both plugins log system operations rather than pipeline-specific override rationale. No built-in "gate override" event type exists; you'd need custom logic.

### Spinnaker Deployment Pipeline

Spinnaker uses Manual Judgments as gates:
- **Gate pattern**: Manual Judgment stages pause the pipeline and wait for approval to proceed or stop
- **Execution history**: Provides execution history as an audit log of deployment operations and enforced policies
- **Limitation**: Like Argo, Spinnaker can view execution history but doesn't explicitly capture override rationale as a first-class feature

## Implications

### Koto Should Treat Override as a First-Class Event

All surveyed systems either:
1. Log override/bypass as a distinct event type (GitHub, compliance standards), OR
2. Model override as an explicit secondary decision step with its own approval flow (workflow pattern standards)

Koto's current approach (implicit override via evidence presence) is an outlier. Making overrides explicit events will enable:
- **Auditability**: Every override is queryable and attributable
- **Compliance**: Supports SOX, GDPR, and other regulatory requirements
- **Accountability**: Clear "who, when, why" for every gate bypass

### Rationale Must Be Captured at Override Time

The AI agent audit trail research highlights that "why" is non-obvious for autonomous systems. Koto should require the agent to provide reasoning at override time (similar to `koto decisions record`, but mandatory and structured).

Pattern from compliance: Store {approver_id, override_reason, timestamp, gate_id, gate_outcome_if_not_overridden}.

### Event Sourcing Infrastructure Enables Future Features

All three future capabilities koto wants (verification, visualization, redo):
- **Verification**: Query the event log to confirm gate state and override decision
- **Visualization**: Build read models/projections from events (e.g., "show me all overrides by gate type over time")
- **Redo**: Replay from override event to explore "what if we hadn't overridden?" or to replay with corrected logic

Event sourcing is not required today (Koto already has JSONL event persistence), but the pattern suggests structuring override as a replayable event rather than implicit state.

### Approval Gates Should Preserve Negative Outcome

Standards converge on: Store the gate's actual outcome (pass/fail) separately from the override decision. This enables:
- Analyzing gate accuracy over time ("how many times was this gate overridden?")
- Distinguishing "gate was wrong" from "gate was right but we had a good reason to bypass anyway"

Koto currently loses the "gate would have failed" information once evidence is provided.

## Surprises

### Argo and Spinnaker Don't Have Built-In Override Rationale

Both are mature, CI/CD-focused tools, yet neither has a built-in "capture override rationale" stage. This suggests:
- The problem is recognized but not yet standardized in the open-source ecosystem
- Organizations using these tools either implement custom logging or accept the gap
- Koto has an opportunity to pioneer a cleaner pattern

### GDPR and AI Transparency Are Reshaping Audit Requirements

The AI agent audit trail research reveals a regulatory shift: compliance now requires explaining *why* an agent (or human) made a decision, not just logging that it happened. This goes beyond SOX/CI-CD thinking, which predates agentic AI.

For koto (an AI orchestration engine), this is a stronger compliance driver than traditional deployment pipeline auditing.

### Event Sourcing Is the Dominant Pattern for Complex Workflows

Temporal, Cadence, and event-sourced systems all treat the audit trail as the source of truth, not a derivative log. This differs from traditional databases where you log changes *to* state. For workflow engines handling redo/replay/verification, the event log *is* the system.

## Open Questions

1. **Who should provide override rationale?** The AI agent (code), a human reviewer, or both? Should different gates require different sources?

2. **How structured should rationale be?** Free-form text (like comments), structured fields (gate reason: "pre-release check overridden", code_owner: user), or both?

3. **Should override require explicit approval or just logging?** Current pattern: if evidence is provided, gate is implicitly overridden. Compliance pattern: override should be a separate decision step. Is implicit override acceptable if logged?

4. **How should gate override be queryable?** Should Koto provide a built-in query API (e.g., "list all overrides for gate X in workflow Y"), or just ensure the event structure makes ad-hoc queries possible?

5. **What happens if redo is attempted?** If you replay a workflow from an override event, should the gate be re-evaluated or the override replayed as-is?

6. **Should override be reversible?** Can a user retract an override and re-fail the gate? Or is override immutable once recorded?

## Summary

Established workflow engines (GitHub, compliance standards, Temporal) treat gate overrides as first-class auditable events with structured capture of who, when, and why; event sourcing provides the infrastructure to make overrides queryable and replayable for future verification, visualization, and redo. The emerging AI agent audit trail research reveals that regulatory frameworks now require capturing not just what happened but why an autonomous system made its decision—a requirement koto should front-load in its override design.

---

## Sources
- [GitHub Actions environment protection rules bypass](https://docs.github.com/en/organizations/keeping-your-organization-secure/managing-security-settings-for-your-organization/audit-log-events-for-your-organization)
- [GitHub Actions admins can bypass environment protection rules](https://github.blog/changelog/2023-03-01-github-actions-admins-can-now-bypass-environment-protection-rules/)
- [SOX Compliance for Software Delivery](https://www.harness.io/harness-devops-academy/sox-compliance-for-software-delivery-explained)
- [Building SOX Compliance into CI/CD Pipelines](https://hoop.dev/blog/building-sox-compliance-into-your-ci-cd-pipelines/)
- [Audit Trails in CI/CD: Best Practices for AI Agents](https://prefactor.tech/blog/audit-trails-in-ci-cd-best-practices-for-ai-agents)
- [Temporal Workflow documentation](https://docs.temporal.io/workflows)
- [Mastering Temporal Workflows — Auditing](https://medium.com/@ry84155/mastering-temporal-workflows-reliable-systems-with-composition-auditing-and-internal-magic-fdb5d4be9f1e)
- [Argo Workflows documentation](https://argoproj.github.io/workflows/)
- [Approval workflow modeling patterns - SAP Signavio](https://www.signavio.com/post/approval-workflow-modelling-patterns/)
- [Workflow patterns - Dapr Docs](https://docs.dapr.io/developing-applications/building-blocks/workflow/workflow-patterns/)
- [Workflow patterns - Camunda 8 Docs](https://docs.camunda.io/docs/components/concepts/workflow-patterns/)
- [Agentic workflows for software development](https://medium.com/quantumblack/agentic-workflows-for-software-development-dc8e64f4a79d)
- [Enterprise AI Approval Workflow for Risk-Based Decisions](https://qorsync.online/blog/ai-approval-workflow)
- [Event Sourcing Pattern - Microsoft Learn](https://learn.microsoft.com/en-us/azure/architecture/patterns/event-sourcing)
- [Audit log with event sourcing - Arkency](https://blog.arkency.com/audit-log-with-event-sourcing/)
- [Event Sourcing - Martin Fowler](https://martinfowler.com/eaaDev/EventSourcing.html)
- [Audit Trail and Event Sourcing in Chiron](https://www.yields.io/blog/audit-trail-and-event-sourcing-in-chiron/)
- [Akka Event-Sourced Applications](https://akka.io/app-types/event-sourced/)
- [The Growing Challenge of Auditing Agentic AI](https://www.isaca.org/resources/news-and-trends/industry-news/2025/the-growing-challenge-of-auditing-agentic-ai)
- [The AI Audit Trail: Compliance and Transparency with LLM Observability](https://medium.com/@kuldeep.paul08/the-ai-audit-trail-how-to-ensure-compliance-and-transparency-with-llm-observability-74fd5f1968ef)
- [Building trustworthy AI agents for compliance - IBM](https://www.ibm.com/think/insights/building-trustworthy-ai-agents-compliance-auditability-explainability)
- [Your AI Agent Needs an Audit Trail, Not Just a Guardrail](https://medium.com/@ianloe/your-ai-agent-needs-an-audit-trail-not-just-a-guardrail-6a41de67ae75)
- [Your AI Agents and the Audit Trail: What Compliance Actually Needs](https://dev.to/waxell/your-ai-agents-and-the-audit-trail-what-compliance-actually-needs-33i5)
- [Jenkins Audit Trail Plugin](https://plugins.jenkins.io/audit-trail/)
- [GitHub Configuration Checklist for SOC 2 Compliance](https://delve.co/blog/github-configuration-checklist-for-soc-2-compliance)
- [Spinnaker Safe Deployments Guide](https://spinnaker.io/docs/guides/tutorials/codelabs/safe-deployments/)
