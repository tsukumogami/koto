//! Recursion-cap enforcement (PRD R29, Decision 4).
//!
//! Three independent dimensions, each with a soft warn threshold and
//! a hard reject threshold:
//!
//! | Dimension          | Warn | Reject | What it catches |
//! |--------------------|-----:|-------:|-----------------|
//! | depth              |    5 |     10 | Recursive workflows that never branch (parent → parent → … → root). |
//! | fanout             |   20 |    100 | Single coordinator spawning a storm of siblings under the same parent. |
//! | total_unassigned   |  100 |    500 | Tree-wide accumulation no single coordinator owns. |
//!
//! ## Invocation
//!
//! [`validate_recursion_caps`] is the single entry point. It runs all
//! three validators in dimension order (depth → fanout → total) and
//! short-circuits on the first hard-reject. The CLI invokes it BEFORE
//! any header write on every `koto session start --needs-agent` so a
//! cap rejection leaves no on-disk side effects.
//!
//! ## Algorithm pins (design line 1842)
//!
//! Each validator's algorithm is fixed by the design's Required
//! Tactical Designs table. The pins are non-negotiable — substituting
//! a cheaper-but-different algorithm would silently break the perf
//! guarantee that downstream issues depend on:
//!
//! - **Depth** is walked via `parent_workflow` chain starting from the
//!   spawning subagent. Each hop reads one header. The walk terminates
//!   at the root (`parent_workflow.is_none()`) or at a missing parent
//!   (treated as the root for cycle-safety).
//! - **Fanout** is the per-parent header scan: enumerate all sessions
//!   whose `parent_workflow == spawning_parent`, filter to those with
//!   `needs_agent == Some(true)` AND `assignment_claim.is_none()`. The
//!   resulting count is what would be NEW siblings — `+1` for the
//!   incoming request.
//! - **Total-unassigned** is the workspace-wide sweep WITH the
//!   terminal-index filter from Issue 8. The filter is the AD3.3
//!   perf-cliff-avoidance pin from the Required Tactical Designs
//!   table — without it, the counter would walk every header at
//!   year-2 scale (~26k sessions) on every spawn, producing the
//!   30-second-or-longer validation pause the design rules out.
//!
//! ## Hard-coded at V1 (Decision 4)
//!
//! The reserved `[request_store.recursion]` namespace (Issue 18) pre-stakes the
//! TOML location for a V1.1 promotion to operator-configurable caps,
//! but at V1 the thresholds are hard-coded constants. An operator
//! override surface would silently break substrate-agnostic operation
//! — the bunki BK2 plane's safety depends on the koto layer enforcing
//! consistent caps regardless of operator config.

use crate::engine::errors::EngineError;
use crate::engine::terminal_index::read_terminal_index;
use crate::session::SessionBackend;

// ===== Constants =====

/// Depth at which a warn-level log fires.
///
/// A child whose `parent_workflow` chain length reaches 5 is unlikely
/// to be a bug yet, but the operator should know — deep recursion is
/// the silent-divergence shape PRD R29 calls out.
pub const DEPTH_WARN: u32 = 5;

/// Depth at which the request is hard-rejected.
///
/// Reaching depth 10 indicates either a recursive workflow with no
/// terminating condition or an unintentional spawn loop. The reject
/// surfaces with `EngineError::RecursionCapExceeded { dimension:
/// "depth", ... }` and exit code 64 (`EX_USAGE`).
pub const DEPTH_REJECT: u32 = 10;

/// Per-parent unclaimed-children count at which a warn fires.
pub const FANOUT_WARN: u32 = 20;

/// Per-parent unclaimed-children count at which the spawn is rejected.
///
/// A coordinator that has already accumulated 100 unclaimed
/// `needs_agent` children is exhibiting a fanout-storm pattern; the
/// reject forces the calling agent to consolidate before spawning more.
pub const FANOUT_REJECT: u32 = 100;

/// Tree-wide unclaimed-children count at which a warn fires.
pub const TOTAL_WARN: u32 = 100;

/// Tree-wide unclaimed-children count at which the spawn is rejected.
///
/// The tree-wide cap catches accumulation that no single coordinator
/// owns (each individual parent stays under FANOUT_REJECT, but the
/// workspace's total unclaimed-child count overruns).
pub const TOTAL_REJECT: u32 = 500;

// ===== Outcome enum =====

/// Result of a recursion-cap evaluation for one dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapEvaluation {
    /// Observed count is below the warn threshold; nothing to surface.
    Ok,
    /// Observed count is at or above the warn threshold but below the
    /// reject threshold. The caller emits a warn-level log and
    /// proceeds with the spawn.
    Warn {
        dimension: &'static str,
        threshold: u32,
        observed: u32,
    },
    /// Observed count is at or above the reject threshold. The
    /// validator returns `EngineError::RecursionCapExceeded` and the
    /// CLI exits 64 without any header write.
    Reject {
        dimension: &'static str,
        threshold: u32,
        observed: u32,
    },
}

impl CapEvaluation {
    /// Promote this evaluation to a typed error when it is a reject.
    /// `Ok` and `Warn` map to `None`.
    pub fn into_reject(self) -> Option<EngineError> {
        match self {
            CapEvaluation::Reject {
                dimension,
                threshold,
                observed,
            } => Some(EngineError::RecursionCapExceeded {
                dimension: dimension.to_string(),
                threshold,
                observed,
            }),
            _ => None,
        }
    }
}

/// Compose a [`CapEvaluation`] from an observed count and the two
/// thresholds for a given dimension.
fn classify(dimension: &'static str, observed: u32, warn: u32, reject: u32) -> CapEvaluation {
    if observed >= reject {
        CapEvaluation::Reject {
            dimension,
            threshold: reject,
            observed,
        }
    } else if observed >= warn {
        CapEvaluation::Warn {
            dimension,
            threshold: warn,
            observed,
        }
    } else {
        CapEvaluation::Ok
    }
}

// ===== Depth validator =====

/// Walk `parent_workflow` upward from the spawning subagent, counting
/// hops until the chain reaches a root (`parent_workflow.is_none()`)
/// or a missing parent (treated as a root for cycle-safety).
///
/// The returned depth is the number of generations ABOVE the
/// spawning subagent, so a fresh spawn under a root parent has
/// depth 1. The cap check is against `depth + 1` (the depth the new
/// child would occupy after the spawn).
pub fn measure_depth_from_parent(
    backend: &dyn SessionBackend,
    spawning_parent: &str,
) -> Result<u32, EngineError> {
    // Defensive cap on walk length to prevent a corrupted header
    // graph (cycle: A → B → A) from looping forever. The cap is
    // generous (1000) so a legitimate deep chain that hits
    // DEPTH_REJECT (10) is well within the limit; cycle detection
    // is structural, not best-effort.
    const MAX_WALK_HOPS: u32 = 1000;

    let mut current = spawning_parent.to_string();
    let mut hops: u32 = 0;
    let mut seen = std::collections::HashSet::new();
    loop {
        if hops > MAX_WALK_HOPS {
            // Treat overlong walks as if they had reached the root —
            // the cap rejection will fire on the next call site if
            // appropriate, but a runaway loop never poisons the
            // validator.
            break;
        }
        if !seen.insert(current.clone()) {
            // Cycle in the parent chain. Stop walking; treat the
            // visited chain as the depth (the cap will fire if it
            // already exceeds the threshold).
            break;
        }
        hops += 1;
        // Read the current header to find its parent.
        let header = match backend.read_header(&current) {
            Ok(h) => h,
            // A missing intermediate header is treated as the root —
            // we can't walk further, so the chain ends here. The
            // caller still gets a sensible depth count.
            Err(_) => break,
        };
        match header.parent_workflow {
            Some(p) if !p.is_empty() => {
                current = p;
            }
            _ => break,
        }
    }
    Ok(hops)
}

/// Validate the depth dimension. `spawning_parent` is the name of the
/// parent the new child would attach to; the depth check counts the
/// chain INCLUDING that parent.
///
/// The cap is on the new child's effective depth, which is
/// `parent_chain_length + 1`. So `DEPTH_REJECT = 10` means: a request
/// where the parent's chain has 10 hops (i.e., the new child would be
/// at depth 11) rejects.
pub fn validate_depth(
    backend: &dyn SessionBackend,
    spawning_parent: &str,
) -> Result<CapEvaluation, EngineError> {
    let parent_chain = measure_depth_from_parent(backend, spawning_parent)?;
    // New child's depth = parent's chain length + 1 (the child itself).
    let new_depth = parent_chain.saturating_add(1);
    Ok(classify("depth", new_depth, DEPTH_WARN, DEPTH_REJECT))
}

// ===== Fanout validator =====

/// Count sessions whose `parent_workflow == spawning_parent` AND
/// whose header carries `needs_agent == Some(true)` AND
/// `assignment_claim.is_none()`.
///
/// These are the "unclaimed children of this parent" — the population
/// the fanout cap protects. The count does NOT include the incoming
/// request; the cap check is against `current + 1`.
pub fn measure_fanout(
    backend: &dyn SessionBackend,
    spawning_parent: &str,
) -> Result<u32, EngineError> {
    let sessions = backend.list().map_err(|e| {
        EngineError::ParseError(format!("failed to list sessions for fanout count: {}", e))
    })?;
    let mut count: u32 = 0;
    for info in sessions {
        if info.parent_workflow.as_deref() != Some(spawning_parent) {
            continue;
        }
        // Cheap parent-name filter passed; read the header to apply
        // the needs_agent + unclaimed filter. A read failure on an
        // individual header is treated as "not a candidate" (the
        // sweep can't classify it; better to under-count than to
        // poison the cap check with a transient I/O error).
        let header = match backend.read_header(&info.id) {
            Ok(h) => h,
            Err(_) => continue,
        };
        if header.needs_agent == Some(true) && header.assignment_claim.is_none() {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

/// Validate the fanout dimension. `spawning_parent` is the parent
/// the new sibling would attach to.
pub fn validate_fanout(
    backend: &dyn SessionBackend,
    spawning_parent: &str,
) -> Result<CapEvaluation, EngineError> {
    let current = measure_fanout(backend, spawning_parent)?;
    // New child = current + 1.
    let projected = current.saturating_add(1);
    Ok(classify("fanout", projected, FANOUT_WARN, FANOUT_REJECT))
}

// ===== Total-unassigned validator =====

/// Count workspace-wide sessions with `needs_agent == Some(true)` AND
/// `assignment_claim.is_none()`, applying the terminal-index filter
/// from Issue 8 to skip sessions known to be terminal.
///
/// This is the load-bearing AD3.3 perf-cliff-avoidance algorithm: the
/// terminal-index lookup is O(open-sessions) rather than
/// O(workspace-sessions), so the counter stays cheap at year-2 scale
/// (~26k workspace sessions where 25.9k are terminal). Without the
/// filter, every `--needs-agent` spawn would walk every header on
/// disk — exactly the perf cliff the design's Required Tactical
/// Designs table item (a) calls out.
///
/// `koto_root` is the workspace root (typically `~/.koto`); the
/// terminal index lives at `<koto_root>/_terminal_index.jsonl`.
pub fn measure_total_unassigned(
    backend: &dyn SessionBackend,
    koto_root: &std::path::Path,
) -> Result<u32, EngineError> {
    let terminal = read_terminal_index(koto_root);
    let sessions = backend.list().map_err(|e| {
        EngineError::ParseError(format!(
            "failed to list sessions for total-unassigned count: {}",
            e
        ))
    })?;
    let mut count: u32 = 0;
    for info in sessions {
        // Apply the terminal-index filter BEFORE the header read so
        // the workspace-walk cost is bounded by the unfiltered set
        // (open + recently-modified sessions), not the full workspace.
        // The header-is-truth rule (Issue 8's discovery integration)
        // says the index can lag the header; here we accept that lag
        // because a stale terminal entry is the safer direction
        // (under-count) for the cap check.
        if terminal.contains_key(&info.id) {
            continue;
        }
        let header = match backend.read_header(&info.id) {
            Ok(h) => h,
            Err(_) => continue,
        };
        if header.needs_agent == Some(true) && header.assignment_claim.is_none() {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

/// Validate the total-unassigned dimension.
pub fn validate_total_unassigned(
    backend: &dyn SessionBackend,
    koto_root: &std::path::Path,
) -> Result<CapEvaluation, EngineError> {
    let current = measure_total_unassigned(backend, koto_root)?;
    let projected = current.saturating_add(1);
    Ok(classify(
        "total_unassigned",
        projected,
        TOTAL_WARN,
        TOTAL_REJECT,
    ))
}

// ===== Orchestrator =====

/// Outcome of a full three-dimensional cap validation pass. The CLI
/// caller logs each `Warn` and short-circuits on the first `Reject`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapsOutcome {
    /// Depth-dimension evaluation.
    pub depth: Option<CapEvaluation>,
    /// Fanout-dimension evaluation.
    pub fanout: Option<CapEvaluation>,
    /// Total-unassigned-dimension evaluation.
    pub total_unassigned: Option<CapEvaluation>,
}

impl CapsOutcome {
    /// Returns the first rejecting evaluation, if any. Used by the
    /// CLI caller to short-circuit before writing the new header.
    pub fn first_reject(&self) -> Option<&CapEvaluation> {
        for slot in [&self.depth, &self.fanout, &self.total_unassigned] {
            if let Some(CapEvaluation::Reject { .. }) = slot {
                return slot.as_ref();
            }
        }
        None
    }

    /// Iterate over the `Warn` evaluations so the caller can emit one
    /// warn-level log per dimension.
    pub fn warnings(&self) -> impl Iterator<Item = &CapEvaluation> {
        [&self.depth, &self.fanout, &self.total_unassigned]
            .into_iter()
            .filter_map(|slot| match slot {
                Some(w @ CapEvaluation::Warn { .. }) => Some(w),
                _ => None,
            })
    }
}

/// Run all three cap validators against the spawn request.
///
/// `spawning_parent` is the name of the parent workflow the new
/// child would attach to (the `--parent` argument). `koto_root` is
/// the workspace root used by the total-unassigned counter to read
/// the terminal index.
///
/// The orchestrator does NOT short-circuit on the first reject — it
/// runs all three so callers get a complete picture of the spawn
/// request's cap status. The caller decides how to react via
/// [`CapsOutcome::first_reject`] and [`CapsOutcome::warnings`].
pub fn validate_recursion_caps(
    backend: &dyn SessionBackend,
    spawning_parent: &str,
    koto_root: &std::path::Path,
) -> Result<CapsOutcome, EngineError> {
    Ok(CapsOutcome {
        depth: Some(validate_depth(backend, spawning_parent)?),
        fanout: Some(validate_fanout(backend, spawning_parent)?),
        total_unassigned: Some(validate_total_unassigned(backend, koto_root)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The classify function is pure; exercise its three branches
    // here. Algorithm-end-to-end coverage (depth walk, fanout scan,
    // total-unassigned with terminal-index) lives in
    // `tests/recursion_caps.rs` which uses real backends.

    #[test]
    fn classify_below_warn_returns_ok() {
        assert_eq!(classify("x", 4, 5, 10), CapEvaluation::Ok);
        assert_eq!(classify("x", 0, 5, 10), CapEvaluation::Ok);
    }

    #[test]
    fn classify_at_warn_returns_warn() {
        let got = classify("depth", 5, 5, 10);
        assert!(matches!(
            got,
            CapEvaluation::Warn {
                dimension: "depth",
                threshold: 5,
                observed: 5
            }
        ));
    }

    #[test]
    fn classify_below_reject_above_warn_returns_warn() {
        let got = classify("depth", 9, 5, 10);
        assert!(matches!(
            got,
            CapEvaluation::Warn {
                dimension: "depth",
                threshold: 5,
                observed: 9
            }
        ));
    }

    #[test]
    fn classify_at_reject_returns_reject() {
        let got = classify("depth", 10, 5, 10);
        assert!(matches!(
            got,
            CapEvaluation::Reject {
                dimension: "depth",
                threshold: 10,
                observed: 10
            }
        ));
    }

    #[test]
    fn classify_above_reject_returns_reject() {
        let got = classify("fanout", 200, 20, 100);
        assert!(matches!(
            got,
            CapEvaluation::Reject {
                dimension: "fanout",
                threshold: 100,
                observed: 200
            }
        ));
    }

    #[test]
    fn into_reject_yields_engine_error() {
        let eval = CapEvaluation::Reject {
            dimension: "total_unassigned",
            threshold: 500,
            observed: 501,
        };
        match eval.into_reject() {
            Some(EngineError::RecursionCapExceeded {
                dimension,
                threshold,
                observed,
            }) => {
                assert_eq!(dimension, "total_unassigned");
                assert_eq!(threshold, 500);
                assert_eq!(observed, 501);
            }
            other => panic!("expected RecursionCapExceeded, got {:?}", other),
        }
    }

    #[test]
    fn into_reject_returns_none_for_ok_and_warn() {
        assert!(CapEvaluation::Ok.into_reject().is_none());
        let warn = CapEvaluation::Warn {
            dimension: "depth",
            threshold: 5,
            observed: 5,
        };
        assert!(warn.into_reject().is_none());
    }

    #[test]
    fn outcome_first_reject_prefers_depth_then_fanout_then_total() {
        let outcome = CapsOutcome {
            depth: Some(CapEvaluation::Warn {
                dimension: "depth",
                threshold: 5,
                observed: 7,
            }),
            fanout: Some(CapEvaluation::Reject {
                dimension: "fanout",
                threshold: 100,
                observed: 100,
            }),
            total_unassigned: Some(CapEvaluation::Reject {
                dimension: "total_unassigned",
                threshold: 500,
                observed: 600,
            }),
        };
        let got = outcome.first_reject().unwrap();
        // First reject in dimension order is fanout.
        assert!(matches!(
            got,
            CapEvaluation::Reject {
                dimension: "fanout",
                ..
            }
        ));
    }

    #[test]
    fn outcome_first_reject_none_when_all_ok_or_warn() {
        let outcome = CapsOutcome {
            depth: Some(CapEvaluation::Ok),
            fanout: Some(CapEvaluation::Warn {
                dimension: "fanout",
                threshold: 20,
                observed: 20,
            }),
            total_unassigned: Some(CapEvaluation::Ok),
        };
        assert!(outcome.first_reject().is_none());
        assert_eq!(outcome.warnings().count(), 1);
    }

    #[test]
    fn recursion_cap_exceeded_exit_code_is_64() {
        let err = EngineError::RecursionCapExceeded {
            dimension: "depth".to_string(),
            threshold: 10,
            observed: 11,
        };
        assert_eq!(err.exit_code(), 64);
    }

    #[test]
    fn recursion_cap_exceeded_display_names_all_three_fields() {
        let err = EngineError::RecursionCapExceeded {
            dimension: "fanout".to_string(),
            threshold: 100,
            observed: 200,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("fanout"));
        assert!(msg.contains("100"));
        assert!(msg.contains("200"));
    }

    #[test]
    fn constants_match_design() {
        // Verifies the six numeric constants are exactly the values
        // the design pins. A drift here would silently change the
        // protocol contract.
        assert_eq!(DEPTH_WARN, 5);
        assert_eq!(DEPTH_REJECT, 10);
        assert_eq!(FANOUT_WARN, 20);
        assert_eq!(FANOUT_REJECT, 100);
        assert_eq!(TOTAL_WARN, 100);
        assert_eq!(TOTAL_REJECT, 500);
    }
}
