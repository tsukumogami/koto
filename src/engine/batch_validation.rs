//! Pre-append runtime validation for batch submissions (Issue #9).
//!
//! [`validate_batch_submission`] is a pure function: it consumes the
//! submitted task list plus a snapshot of the siblings already on disk
//! and returns the *first* rule violation it finds, or `Ok(())` when
//! every whole-submission rule passes. No `EvidenceSubmitted` event
//! may be appended to a batch parent's log before this check succeeds
//! (Decision 11 in `docs/designs/DESIGN-batch-child-spawning.md`).
//!
//! # Scope
//!
//! This module covers the *whole-submission* rules:
//!
//! | Rule | Check |
//! |------|-------|
//! | R0 | `tasks.len() >= 1` |
//! | R3 | `waits_on` forms a DAG (no cycles) |
//! | R4 | every `waits_on` entry resolves to a submitted task |
//! | R5 | task names are unique within the submission |
//! | R6 | `tasks.len() <= 1000`, per-task `waits_on.len() <= 10`, DAG depth `<= 50` |
//! | R8 | entries whose child already exists on disk match the recorded `spawn_entry` |
//! | R9 | each task name matches `^[A-Za-z0-9_-]+$`, is 1..=64 chars, and is not reserved |
//!
//! R1 (child template compilable) and R2 (vars resolve against child
//! template) are **per-task** checks surfaced by the scheduler as
//! `BatchTaskView.outcome: spawn_failed`; they do not reject the
//! whole submission and so are not invoked here. Issue #12's
//! `run_batch_scheduler` owns the per-task path. R7 (sibling
//! collisions) is enforced inside `init_state_file` via
//! `renameat2(RENAME_NOREPLACE)` (Issue #1), so it never runs here.
//!
//! # Order of checks
//!
//! The order pins behavior readers rely on when reasoning about
//! mixed-failure submissions. `DESIGN-batch-child-spawning.md:1956`
//! spells out `R0, R3, R4, R5, R6, R8, R9`. R4-before-R8 matters in
//! particular: a typoed dependency name should surface as
//! [`InvalidBatchReason::UnknownWaitsOn`] rather than a spurious
//! [`InvalidBatchReason::SpawnedTaskMutated`] on an unrelated entry.
//!
//! # Return shape
//!
//! Returns the **first** violation encountered rather than a
//! `Vec<InvalidBatchReason>`. Agents submit batches; accumulating
//! every conceivable failure across the whole payload provides little
//! value over a single actionable rejection, and it would force the
//! caller (and Issue #10's envelope) to design a multi-error shape
//! that the design document never commits to. Fixing the first issue
//! and resubmitting is the expected loop.

use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::cli::batch_error::{BatchError, InvalidBatchReason, InvalidNameDetail, LimitKind};
use crate::engine::types::SpawnEntrySnapshot;

/// Hard limit on the number of tasks in one submission (R6).
pub const MAX_TASKS_PER_SUBMISSION: u32 = 1000;

/// Hard limit on `waits_on` entries per task (R6).
pub const MAX_WAITS_ON_PER_TASK: u32 = 10;

/// Hard limit on DAG depth, measured as node count along the longest
/// root-to-leaf path (R6). A single-node DAG has depth 1; a linear
/// chain of three tasks has depth 3.
pub const MAX_DAG_DEPTH: u32 = 50;

/// Reserved task names (R9). These are evidence-action reserved
/// words; submitting a task with one of these names would collide
/// with the scheduler's own vocabulary.
const RESERVED_NAMES: &[&str] = &["retry_failed", "cancel_tasks"];

/// Pre-append payload for one submitted task.
///
/// A future Issue (#12 when the scheduler lands) may move this type
/// behind the `tasks`-accepts parsing path; the minimal shape here
/// matches the R0-R9 rule surface and keeps the validator free of
/// template-layer dependencies. `vars` is a [`BTreeMap`] so the
/// serialized form matches [`SpawnEntrySnapshot::vars`] byte-for-byte
/// during R8 comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEntry {
    /// Short task name (R9: `^[A-Za-z0-9_-]+$`, 1..=64 chars).
    pub name: String,
    /// Optional override of the parent state's `default_template`.
    /// When `None`, R8 comparison is deferred to the caller, which
    /// resolves the canonical-form default template at spawn time
    /// (see `DESIGN-batch-child-spawning.md:1974`).
    #[serde(default)]
    pub template: Option<String>,
    /// Per-task variable bindings. Sorted by key through
    /// [`BTreeMap`] so the on-wire order matches what R8 compares
    /// against.
    #[serde(default)]
    pub vars: BTreeMap<String, serde_json::Value>,
    /// Names of other tasks in this submission that must complete
    /// before this one spawns. Whitespace/sorting is the caller's
    /// responsibility.
    #[serde(default)]
    pub waits_on: Vec<String>,
}

/// Validate a batch submission before any `EvidenceSubmitted` write.
///
/// `tasks` is the submitted payload. `existing_children` maps the
/// short task name (what the agent submitted) to the `spawn_entry`
/// recorded when that child was originally materialized, or `None`
/// when the child is mid-respawn (R8-vacuous window — see
/// `DESIGN-batch-child-spawning.md:1960`). Short names absent from
/// the map are new tasks that have never been spawned.
///
/// Returns [`BatchError::InvalidBatchDefinition`] or
/// [`BatchError::LimitExceeded`] on the first rule violation. The
/// caller is responsible for rendering the envelope (via
/// `BatchError::to_envelope`) and choosing an exit code.
///
/// # Order
///
/// R0, R3, R4, R5, R6, R8, R9. See module docs for why.
pub fn validate_batch_submission(
    tasks: &[TaskEntry],
    existing_children: &HashMap<String, Option<SpawnEntrySnapshot>>,
) -> Result<(), BatchError> {
    // R0: non-empty.
    if tasks.is_empty() {
        return Err(BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::EmptyTaskList,
        });
    }

    // R6 (tasks-count sub-rule) must run before the DAG traversals so
    // a 10,000-task payload doesn't waste time building an adjacency
    // list just to reject on size.
    if tasks.len() as u32 > MAX_TASKS_PER_SUBMISSION {
        return Err(BatchError::LimitExceeded {
            which: LimitKind::Tasks,
            limit: MAX_TASKS_PER_SUBMISSION,
            actual: tasks.len() as u32,
            task: None,
        });
    }

    // R5 must precede R3/R4 because those rules build a name→index
    // map that silently collapses duplicates. Detect duplicates first
    // so the agent sees the canonical cause.
    let mut seen: HashSet<&str> = HashSet::with_capacity(tasks.len());
    for task in tasks {
        if !seen.insert(task.name.as_str()) {
            return Err(BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::DuplicateTaskName {
                    task: task.name.clone(),
                },
            });
        }
    }

    // R6 (waits_on-length sub-rule) — cheap per-task check before
    // pointer-chasing.
    for task in tasks {
        if task.waits_on.len() as u32 > MAX_WAITS_ON_PER_TASK {
            return Err(BatchError::LimitExceeded {
                which: LimitKind::WaitsOn,
                limit: MAX_WAITS_ON_PER_TASK,
                actual: task.waits_on.len() as u32,
                task: Some(task.name.clone()),
            });
        }
    }

    // R4: every waits_on entry must name a submitted task.
    let name_set: HashSet<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    for task in tasks {
        for dep in &task.waits_on {
            if !name_set.contains(dep.as_str()) {
                return Err(BatchError::InvalidBatchDefinition {
                    reason: InvalidBatchReason::UnknownWaitsOn {
                        task: task.name.clone(),
                        unknown: dep.clone(),
                    },
                });
            }
        }
    }

    // R3: DAG-ness (no cycles). R4 has already guaranteed every edge
    // lands on a submitted task.
    if let Some(cycle) = find_cycle(tasks) {
        return Err(BatchError::InvalidBatchDefinition {
            reason: InvalidBatchReason::Cycle { cycle },
        });
    }

    // R6 (depth sub-rule) runs after cycle detection so we never
    // recurse into a cyclic subgraph.
    let depth = longest_path_node_count(tasks);
    if depth > MAX_DAG_DEPTH {
        return Err(BatchError::LimitExceeded {
            which: LimitKind::Depth,
            limit: MAX_DAG_DEPTH,
            actual: depth,
            task: None,
        });
    }

    // R8: spawn-time immutability. For each submitted task whose
    // child already exists on disk (and is not mid-respawn), the
    // submitted entry must match the recorded spawn_entry.
    for task in tasks {
        // `Some(None)` means the child is mid-respawn — R8-vacuous.
        // `None` (key absent) means the child has never been spawned.
        if let Some(Some(snapshot)) = existing_children.get(task.name.as_str()) {
            if let Some(diff) = spawn_entry_diff(task, snapshot) {
                return Err(BatchError::InvalidBatchDefinition {
                    reason: InvalidBatchReason::SpawnedTaskMutated {
                        task: task.name.clone(),
                        diff,
                    },
                });
            }
        }
    }

    // R9: name regex, length band, reserved set. Runs last so the
    // agent's first rejection is almost always structural rather than
    // cosmetic; an unrecognized name only matters once the DAG is
    // known to be legal.
    let name_re = name_regex();
    for task in tasks {
        if let Some(detail) = check_name(&task.name, &name_re) {
            return Err(BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::InvalidName {
                    task: task.name.clone(),
                    kind: detail,
                },
            });
        }
    }

    Ok(())
}

/// Build the R9 regex. Compiled once per call; the pattern is tiny
/// and the submission volume (<=1000 tasks) makes caching moot.
fn name_regex() -> Regex {
    Regex::new(r"^[A-Za-z0-9_-]+$").expect("R9 regex is a constant and always parses")
}

/// Run the R9 checks against one name. Returns `None` when the name
/// is valid, or the specific [`InvalidNameDetail`] describing why it
/// was rejected.
fn check_name(name: &str, name_re: &Regex) -> Option<InvalidNameDetail> {
    // Length band is checked first so an empty string reports
    // `LengthOutOfRange(0)` instead of `RegexMismatch`.
    if name.is_empty() || name.len() > 64 {
        return Some(InvalidNameDetail::LengthOutOfRange(name.len()));
    }
    if !name_re.is_match(name) {
        return Some(InvalidNameDetail::RegexMismatch);
    }
    if RESERVED_NAMES.contains(&name) {
        return Some(InvalidNameDetail::Reserved(name.to_string()));
    }
    None
}

/// Find a cycle in the `waits_on` graph, returning the task names
/// along the cycle in traversal order. `None` when the graph is
/// acyclic.
///
/// Iterative DFS with a three-state color marker (white/gray/black).
/// Storing the traversal stack lets us reconstruct the cycle's task
/// list when a back-edge is detected.
fn find_cycle(tasks: &[TaskEntry]) -> Option<Vec<String>> {
    let name_to_idx: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.as_str(), i))
        .collect();

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let mut color = vec![Color::White; tasks.len()];
    let mut stack_path: Vec<usize> = Vec::new();

    // Per-node iteration state: which waits_on edge we're exploring.
    let mut iter_state: Vec<usize> = vec![0; tasks.len()];

    for start in 0..tasks.len() {
        if color[start] != Color::White {
            continue;
        }
        // Kick off a DFS rooted at `start`.
        color[start] = Color::Gray;
        iter_state[start] = 0;
        stack_path.push(start);

        while let Some(&node) = stack_path.last() {
            let edges = &tasks[node].waits_on;
            if iter_state[node] >= edges.len() {
                color[node] = Color::Black;
                stack_path.pop();
                continue;
            }
            let next_name = &edges[iter_state[node]];
            iter_state[node] += 1;
            let Some(&next_idx) = name_to_idx.get(next_name.as_str()) else {
                // R4 catches this earlier, but defensively skip.
                continue;
            };
            match color[next_idx] {
                Color::White => {
                    color[next_idx] = Color::Gray;
                    iter_state[next_idx] = 0;
                    stack_path.push(next_idx);
                }
                Color::Gray => {
                    // Back-edge. Reconstruct cycle from
                    // stack_path[stack_path.position(next_idx)..] + [next_idx].
                    let cut = stack_path
                        .iter()
                        .position(|&i| i == next_idx)
                        .expect("gray node must be on the current DFS path");
                    let mut cycle: Vec<String> = stack_path[cut..]
                        .iter()
                        .map(|&i| tasks[i].name.clone())
                        .collect();
                    // Close the loop by appending the entry node again
                    // so downstream readers see `a -> b -> a`.
                    cycle.push(tasks[next_idx].name.clone());
                    return Some(cycle);
                }
                Color::Black => {
                    // Already fully explored; safe to skip.
                }
            }
        }
    }
    None
}

/// Compute the longest root-to-leaf path in the `waits_on` DAG,
/// measured in *node count* (R6 spec).
///
/// Precondition: [`find_cycle`] has returned `None`. The function
/// would infinite-loop on a cyclic graph.
///
/// "Root" is any node with no `waits_on` entries; "leaf" is any node
/// with no incoming edge. `waits_on` encodes predecessor edges, so we
/// compute depth top-down with memoization.
fn longest_path_node_count(tasks: &[TaskEntry]) -> u32 {
    let name_to_idx: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.as_str(), i))
        .collect();

    // depth[i] = longest-path node count from any root to node i.
    // u32::MAX sentinel = not yet computed.
    let mut depth: Vec<u32> = vec![u32::MAX; tasks.len()];

    fn dfs(
        idx: usize,
        tasks: &[TaskEntry],
        name_to_idx: &HashMap<&str, usize>,
        depth: &mut [u32],
    ) -> u32 {
        if depth[idx] != u32::MAX {
            return depth[idx];
        }
        let deps = &tasks[idx].waits_on;
        let max_parent = deps
            .iter()
            .filter_map(|d| name_to_idx.get(d.as_str()).copied())
            .map(|p| dfs(p, tasks, name_to_idx, depth))
            .max()
            .unwrap_or(0);
        let d = max_parent + 1;
        depth[idx] = d;
        d
    }

    let mut best = 0u32;
    for i in 0..tasks.len() {
        let d = dfs(i, tasks, &name_to_idx, &mut depth);
        if d > best {
            best = d;
        }
    }
    best
}

/// Compare a submitted [`TaskEntry`] against the `spawn_entry`
/// snapshot recorded when the child was first materialized (R8). The
/// returned string is a compact, human-readable diff of the
/// differing fields; `None` means the two match byte-for-byte.
///
/// `template` comparison uses "submitted `None` matches any
/// snapshot template" because the canonical-form template is
/// resolved from the parent state's `default_template` *at spawn
/// time* (`DESIGN-batch-child-spawning.md:1974`). The caller that
/// builds `existing_children` is responsible for upgrading
/// `TaskEntry::template = None` to the resolved value before
/// invoking this validator when strict byte-equality is needed;
/// until that caller exists (Issue #12), we treat submitted `None`
/// as a wildcard so unit tests don't get false positives on the
/// common "inherit the default" path.
///
/// TODO(#10): replace the `String` diff with a structured
/// `changed_fields: Vec<{field, spawned_value, submitted_value}>`
/// shape so the error envelope can render richer output and honor
/// the `"[REDACTED]"` sentinel described in the design.
fn spawn_entry_diff(task: &TaskEntry, snapshot: &SpawnEntrySnapshot) -> Option<String> {
    let mut diffs: Vec<String> = Vec::new();

    if let Some(submitted_tpl) = &task.template {
        if submitted_tpl != &snapshot.template {
            diffs.push(format!(
                "template: spawned={:?}, submitted={:?}",
                snapshot.template, submitted_tpl
            ));
        }
    }

    if task.vars != snapshot.vars {
        diffs.push(format!(
            "vars: spawned={}, submitted={}",
            render_vars(&snapshot.vars),
            render_vars(&task.vars),
        ));
    }

    // waits_on comparison uses canonical (sorted) order so the caller
    // doesn't need to pre-sort the submission. The snapshot is
    // already sorted by `SpawnEntrySnapshot::new`.
    let mut submitted_waits = task.waits_on.clone();
    submitted_waits.sort();
    if submitted_waits != snapshot.waits_on {
        diffs.push(format!(
            "waits_on: spawned={:?}, submitted={:?}",
            snapshot.waits_on, submitted_waits
        ));
    }

    if diffs.is_empty() {
        None
    } else {
        Some(diffs.join("; "))
    }
}

fn render_vars(vars: &BTreeMap<String, serde_json::Value>) -> String {
    // Stable serde ordering because BTreeMap iterates in key order.
    serde_json::to_string(vars).unwrap_or_else(|_| "<unserializable>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &str) -> TaskEntry {
        TaskEntry {
            name: name.to_string(),
            template: None,
            vars: BTreeMap::new(),
            waits_on: Vec::new(),
        }
    }

    fn task_with_deps(name: &str, waits_on: &[&str]) -> TaskEntry {
        TaskEntry {
            name: name.to_string(),
            template: None,
            vars: BTreeMap::new(),
            waits_on: waits_on.iter().map(|s| s.to_string()).collect(),
        }
    }

    // --- R0 ---------------------------------------------------------

    #[test]
    fn r0_empty_task_list_rejected() {
        let err = validate_batch_submission(&[], &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::EmptyTaskList
            }
        ));
    }

    #[test]
    fn r0_single_task_accepted() {
        let tasks = vec![task("a")];
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    // --- R3 ---------------------------------------------------------

    #[test]
    fn r3_simple_cycle_rejected() {
        let tasks = vec![task_with_deps("a", &["b"]), task_with_deps("b", &["a"])];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::Cycle { cycle },
            } => {
                assert!(
                    cycle.contains(&"a".to_string()) && cycle.contains(&"b".to_string()),
                    "expected cycle to name a and b, got {:?}",
                    cycle
                );
            }
            other => panic!("expected Cycle, got {:?}", other),
        }
    }

    #[test]
    fn r3_acyclic_diamond_accepted() {
        // a -> b, a -> c, b -> d, c -> d
        let tasks = vec![
            task("a"),
            task_with_deps("b", &["a"]),
            task_with_deps("c", &["a"]),
            task_with_deps("d", &["b", "c"]),
        ];
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    #[test]
    fn r3_self_loop_rejected() {
        let tasks = vec![task_with_deps("a", &["a"])];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::Cycle { .. }
            }
        ));
    }

    // --- R4 ---------------------------------------------------------

    #[test]
    fn r4_dangling_waits_on_rejected() {
        let tasks = vec![task_with_deps("a", &["ghost"])];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::UnknownWaitsOn { task, unknown },
            } => {
                assert_eq!(task, "a");
                assert_eq!(unknown, "ghost");
            }
            other => panic!("expected UnknownWaitsOn, got {:?}", other),
        }
    }

    #[test]
    fn r4_known_waits_on_accepted() {
        let tasks = vec![task("a"), task_with_deps("b", &["a"])];
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    // --- R5 ---------------------------------------------------------

    #[test]
    fn r5_duplicate_names_rejected() {
        let tasks = vec![task("a"), task("a")];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::DuplicateTaskName { task },
            } => assert_eq!(task, "a"),
            other => panic!("expected DuplicateTaskName, got {:?}", other),
        }
    }

    #[test]
    fn r5_unique_names_accepted() {
        let tasks = vec![task("a"), task("b"), task("c")];
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    // --- R6: tasks count --------------------------------------------

    #[test]
    fn r6_tasks_count_limit_at_boundary() {
        // 1000 is the ceiling and must be accepted.
        let mut tasks: Vec<TaskEntry> = Vec::with_capacity(1000);
        for i in 0..1000 {
            tasks.push(task(&format!("t{}", i)));
        }
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    #[test]
    fn r6_tasks_count_limit_exceeded() {
        let mut tasks: Vec<TaskEntry> = Vec::with_capacity(1001);
        for i in 0..1001 {
            tasks.push(task(&format!("t{}", i)));
        }
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::LimitExceeded {
                which: LimitKind::Tasks,
                limit,
                actual,
                task,
            } => {
                assert_eq!(limit, 1000);
                assert_eq!(actual, 1001);
                assert!(task.is_none());
            }
            other => panic!("expected LimitExceeded{{Tasks}}, got {:?}", other),
        }
    }

    // --- R6: waits_on per task --------------------------------------

    #[test]
    fn r6_waits_on_limit_at_boundary() {
        let mut tasks: Vec<TaskEntry> = Vec::new();
        for i in 0..10 {
            tasks.push(task(&format!("d{}", i)));
        }
        // Build a task with exactly 10 waits_on entries (the ceiling).
        let dep_names: Vec<String> = (0..10).map(|i| format!("d{}", i)).collect();
        tasks.push(TaskEntry {
            name: "head".to_string(),
            template: None,
            vars: BTreeMap::new(),
            waits_on: dep_names,
        });
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    #[test]
    fn r6_waits_on_limit_exceeded() {
        let mut tasks: Vec<TaskEntry> = Vec::new();
        for i in 0..11 {
            tasks.push(task(&format!("d{}", i)));
        }
        let dep_names: Vec<String> = (0..11).map(|i| format!("d{}", i)).collect();
        tasks.push(TaskEntry {
            name: "head".to_string(),
            template: None,
            vars: BTreeMap::new(),
            waits_on: dep_names,
        });
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::LimitExceeded {
                which: LimitKind::WaitsOn,
                limit,
                actual,
                task,
            } => {
                assert_eq!(limit, 10);
                assert_eq!(actual, 11);
                assert_eq!(task.as_deref(), Some("head"));
            }
            other => panic!("expected LimitExceeded{{WaitsOn}}, got {:?}", other),
        }
    }

    // --- R6: depth --------------------------------------------------

    #[test]
    fn r6_depth_limit_at_boundary() {
        // Chain of length 50: t0 -> t1 -> ... -> t49.
        let mut tasks: Vec<TaskEntry> = Vec::with_capacity(50);
        tasks.push(task("t0"));
        for i in 1..50 {
            tasks.push(task_with_deps(
                &format!("t{}", i),
                &[&format!("t{}", i - 1)],
            ));
        }
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    #[test]
    fn r6_depth_limit_exceeded() {
        let mut tasks: Vec<TaskEntry> = Vec::with_capacity(51);
        tasks.push(task("t0"));
        for i in 1..51 {
            tasks.push(task_with_deps(
                &format!("t{}", i),
                &[&format!("t{}", i - 1)],
            ));
        }
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::LimitExceeded {
                which: LimitKind::Depth,
                limit,
                actual,
                task,
            } => {
                assert_eq!(limit, 50);
                assert_eq!(actual, 51);
                assert!(task.is_none());
            }
            other => panic!("expected LimitExceeded{{Depth}}, got {:?}", other),
        }
    }

    #[test]
    fn depth_single_node_is_one() {
        let tasks = vec![task("solo")];
        assert_eq!(longest_path_node_count(&tasks), 1);
    }

    #[test]
    fn depth_chain_of_three_is_three() {
        let tasks = vec![
            task("a"),
            task_with_deps("b", &["a"]),
            task_with_deps("c", &["b"]),
        ];
        assert_eq!(longest_path_node_count(&tasks), 3);
    }

    // --- R8 ---------------------------------------------------------

    fn snapshot(
        template: &str,
        vars: &[(&str, serde_json::Value)],
        waits_on: &[&str],
    ) -> SpawnEntrySnapshot {
        let mut btree = BTreeMap::new();
        for (k, v) in vars {
            btree.insert((*k).to_string(), v.clone());
        }
        SpawnEntrySnapshot::new(
            template.to_string(),
            btree,
            waits_on.iter().map(|s| s.to_string()).collect(),
        )
    }

    #[test]
    fn r8_matching_snapshot_accepted() {
        let submitted = TaskEntry {
            name: "a".to_string(),
            template: Some("t.md".to_string()),
            vars: {
                let mut m = BTreeMap::new();
                m.insert("x".to_string(), serde_json::json!(1));
                m
            },
            waits_on: vec![],
        };
        let mut existing = HashMap::new();
        existing.insert(
            "a".to_string(),
            Some(snapshot("t.md", &[("x", serde_json::json!(1))], &[])),
        );
        validate_batch_submission(&[submitted], &existing).unwrap();
    }

    #[test]
    fn r8_vars_mismatch_rejected() {
        let submitted = TaskEntry {
            name: "a".to_string(),
            template: Some("t.md".to_string()),
            vars: {
                let mut m = BTreeMap::new();
                m.insert("x".to_string(), serde_json::json!(2));
                m
            },
            waits_on: vec![],
        };
        let mut existing = HashMap::new();
        existing.insert(
            "a".to_string(),
            Some(snapshot("t.md", &[("x", serde_json::json!(1))], &[])),
        );
        let err = validate_batch_submission(&[submitted], &existing).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::SpawnedTaskMutated { task, diff },
            } => {
                assert_eq!(task, "a");
                assert!(diff.contains("vars"), "expected vars in diff, got {}", diff);
            }
            other => panic!("expected SpawnedTaskMutated, got {:?}", other),
        }
    }

    #[test]
    fn r8_template_mismatch_rejected() {
        let submitted = TaskEntry {
            name: "a".to_string(),
            template: Some("other.md".to_string()),
            vars: BTreeMap::new(),
            waits_on: vec![],
        };
        let mut existing = HashMap::new();
        existing.insert("a".to_string(), Some(snapshot("t.md", &[], &[])));
        let err = validate_batch_submission(&[submitted], &existing).unwrap_err();
        assert!(matches!(
            err,
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::SpawnedTaskMutated { .. }
            }
        ));
    }

    #[test]
    fn r8_respawning_window_is_vacuous() {
        // existing child present but snapshot is None (mid-respawn).
        let submitted = TaskEntry {
            name: "a".to_string(),
            template: Some("other.md".to_string()),
            vars: BTreeMap::new(),
            waits_on: vec![],
        };
        let mut existing = HashMap::new();
        existing.insert("a".to_string(), None);
        validate_batch_submission(&[submitted], &existing).unwrap();
    }

    #[test]
    fn r8_waits_on_canonicalized_before_compare() {
        // Submitted with unsorted waits_on; snapshot stored sorted.
        let submitted = TaskEntry {
            name: "a".to_string(),
            template: Some("t.md".to_string()),
            vars: BTreeMap::new(),
            waits_on: vec!["z".to_string(), "b".to_string()],
        };
        let mut existing = HashMap::new();
        existing.insert("a".to_string(), Some(snapshot("t.md", &[], &["b", "z"])));
        // b and z also need to exist to satisfy R4.
        let tasks = vec![submitted, task("b"), task("z")];
        validate_batch_submission(&tasks, &existing).unwrap();
    }

    // --- R9 ---------------------------------------------------------

    #[test]
    fn r9_valid_name_accepted() {
        let tasks = vec![task("valid_name-1")];
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    #[test]
    fn r9_regex_mismatch_rejected() {
        let tasks = vec![task("bad name")];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason:
                    InvalidBatchReason::InvalidName {
                        task,
                        kind: InvalidNameDetail::RegexMismatch,
                    },
            } => assert_eq!(task, "bad name"),
            other => panic!("expected InvalidName(RegexMismatch), got {:?}", other),
        }
    }

    #[test]
    fn r9_empty_name_rejected_as_length() {
        // Whole-submission R5 uses HashSet of &str so empty strings
        // still hit the R9 length check (both empty names would
        // collide under R5; single empty reaches R9).
        let tasks = vec![task("")];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason:
                    InvalidBatchReason::InvalidName {
                        kind: InvalidNameDetail::LengthOutOfRange(len),
                        ..
                    },
            } => assert_eq!(len, 0),
            other => panic!("expected LengthOutOfRange(0), got {:?}", other),
        }
    }

    #[test]
    fn r9_length_65_rejected() {
        let name = "a".repeat(65);
        let tasks = vec![task(&name)];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason:
                    InvalidBatchReason::InvalidName {
                        kind: InvalidNameDetail::LengthOutOfRange(len),
                        ..
                    },
            } => assert_eq!(len, 65),
            other => panic!("expected LengthOutOfRange(65), got {:?}", other),
        }
    }

    #[test]
    fn r9_length_64_accepted() {
        let name = "a".repeat(64);
        let tasks = vec![task(&name)];
        validate_batch_submission(&tasks, &HashMap::new()).unwrap();
    }

    #[test]
    fn r9_reserved_retry_failed_rejected() {
        let tasks = vec![task("retry_failed")];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        match err {
            BatchError::InvalidBatchDefinition {
                reason:
                    InvalidBatchReason::InvalidName {
                        kind: InvalidNameDetail::Reserved(name),
                        ..
                    },
            } => assert_eq!(name, "retry_failed"),
            other => panic!("expected Reserved(retry_failed), got {:?}", other),
        }
    }

    #[test]
    fn r9_reserved_cancel_tasks_rejected() {
        let tasks = vec![task("cancel_tasks")];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::InvalidName {
                    kind: InvalidNameDetail::Reserved(_),
                    ..
                }
            }
        ));
    }

    // --- Ordering ---------------------------------------------------

    #[test]
    fn r4_reports_before_r8_on_mixed_failures() {
        // A spawned child is mutated AND another task has a dangling
        // waits_on ref. R4 must fire first.
        let tasks = vec![
            TaskEntry {
                name: "a".to_string(),
                template: Some("v2.md".to_string()),
                vars: BTreeMap::new(),
                waits_on: vec![],
            },
            task_with_deps("b", &["ghost"]),
        ];
        let mut existing = HashMap::new();
        existing.insert("a".to_string(), Some(snapshot("v1.md", &[], &[])));
        let err = validate_batch_submission(&tasks, &existing).unwrap_err();
        assert!(matches!(
            err,
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::UnknownWaitsOn { .. }
            }
        ));
    }

    #[test]
    fn r5_reports_before_r3_on_mixed_failures() {
        // Duplicate names AND what would be a cycle. Duplicate fires
        // first because the DAG walk would silently collapse duplicates.
        let tasks = vec![task_with_deps("a", &["a"]), task("a")];
        let err = validate_batch_submission(&tasks, &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            BatchError::InvalidBatchDefinition {
                reason: InvalidBatchReason::DuplicateTaskName { .. }
            }
        ));
    }
}
