//! Native fellowship-side Orchestrator (Phase 12).
//!
//! Replaces the original LLM-driven Orchestrator member surface. The role is
//! mechanical (poll bd, count engineers, write spawn-requests for unclaimed
//! `role:engineer` beads); a tokio loop in fellowship handles it
//! deterministically without burning claude tokens or relying on an
//! external scheduler to wake an LLM PTY.
//!
//! Engineer surfaces are still LLM-driven — they read the bead they were
//! spawned for, claim it via `bd update --claim`, implement, and open a PR.
//!
//! Plan: §6 row 12. Original LLM-orchestrator design at
//! `agents/orchestrator.md` is preserved as historical context but is no
//! longer wired into fellowship's PTY pool (see Phase 12 journal entry).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::beads::{self, Bead, Status};
use crate::runtime;

/// Default poll cadence. Fast for the demo; real deployments should pull
/// this from `Config::agents.orchestrator_poll_secs` (Phase 10.5+).
pub const DEFAULT_POLL_SECS: u64 = 5;

/// Pure routing decision: given the current bead board, the set of bead ids
/// we've already enqueued spawn-requests for in this session, and the
/// current visible engineer count, return the bead ids that should produce
/// new spawn-requests this tick.
///
/// Stays pure (no I/O, no allocation beyond the result vec) so the policy
/// can be unit-tested without `bd` or filesystem state.
pub fn decide_spawns(
    beads: &[Bead],
    already_spawned: &HashSet<String>,
    current_pool: usize,
    max_engineers: usize,
) -> Vec<String> {
    if current_pool >= max_engineers {
        return Vec::new();
    }
    let headroom = max_engineers - current_pool;
    let mut out = Vec::new();
    for bead in beads {
        if out.len() >= headroom {
            break;
        }
        if !is_open_engineer_bead(bead) {
            continue;
        }
        if already_spawned.contains(&bead.id) {
            continue;
        }
        out.push(bead.id.clone());
    }
    out
}

fn is_open_engineer_bead(bead: &Bead) -> bool {
    if bead.status != Status::Open {
        return false;
    }
    if bead.assignee.is_some() {
        // Already claimed (by an engineer that hit `bd update --claim` on a
        // prior tick) or manually assigned — leave it alone.
        return false;
    }
    bead.labels.iter().any(|l| l == "role:engineer")
}

/// Run the native orchestrator loop. Polls `bd list --json` every
/// `poll_interval`, writes a `SpawnRequest` for each open engineer bead
/// that hasn't already been routed, and tracks routed bead ids in-memory
/// so the same bead doesn't trigger repeated spawn-requests within one
/// session.
///
/// Cancellation: returns when the runtime root disappears (rare — fellowship
/// owns it) or when `bd list --json` fails persistently with an error that
/// indicates the bd CLI is gone.
pub async fn run_orchestrator_loop(
    runtime_root: PathBuf,
    max_engineers: usize,
    poll_interval: Duration,
) {
    let mut ticker = time::interval(poll_interval);
    // Skip the immediate first tick so fellowship has a moment to finish
    // boot (singleton PTYs starting, beads pane initial fetch).
    ticker.tick().await;
    let mut already_spawned: HashSet<String> = HashSet::new();
    let mut last_error: Option<String> = None;

    loop {
        ticker.tick().await;

        let beads = match beads::list_beads().await {
            Ok(b) => {
                if last_error.is_some() {
                    info!(count = b.len(), "orchestrator: bd recovered");
                    last_error = None;
                }
                b
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if last_error.as_deref() != Some(msg.as_str()) {
                    error!(error = %msg, "orchestrator: bd list failed");
                    last_error = Some(msg);
                }
                continue;
            }
        };

        // Engineer pool size is best-effort: we count beads currently
        // claimed by `engineer-*` assignees. Fellowship's own engineer
        // pool count would be more accurate but isn't reachable from here
        // without a channel; routing is conservative either way (extra
        // spawns are queued in fellowship beyond `max_engineers`).
        let current_pool = beads
            .iter()
            .filter(|b| {
                b.assignee
                    .as_deref()
                    .is_some_and(|a| a.starts_with("engineer-"))
                    && matches!(b.status, Status::InProgress | Status::InReview)
            })
            .count();

        let to_spawn = decide_spawns(&beads, &already_spawned, current_pool, max_engineers);
        if to_spawn.is_empty() {
            debug!(
                pool = current_pool,
                cap = max_engineers,
                "orchestrator: nothing to route"
            );
            continue;
        }

        for bead_id in to_spawn {
            let branch = format!("feat/bead-{}", bead_id);
            match runtime::write_spawn_request(&runtime_root, Some(branch.clone()), false) {
                Ok((path, request_id)) => {
                    info!(
                        bead = %bead_id,
                        branch = %branch,
                        request_id = %request_id,
                        path = %path.display(),
                        "orchestrator: spawn-request enqueued"
                    );
                    already_spawned.insert(bead_id);
                }
                Err(e) => {
                    error!(bead = %bead_id, "orchestrator: failed to write spawn-request: {e:#}");
                }
            }
        }
    }
}

/// One-shot wrapper that catches a fatal error path. Used by `main` so the
/// orchestrator task exits cleanly rather than silently dying.
pub async fn run(
    runtime_root: PathBuf,
    max_engineers: usize,
    poll_interval: Duration,
) -> Result<()> {
    if max_engineers == 0 {
        warn!("orchestrator: max_engineers is 0 — nothing to route");
        return Ok(());
    }
    run_orchestrator_loop(runtime_root, max_engineers, poll_interval).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bead(id: &str, status: Status, assignee: Option<&str>, labels: &[&str]) -> Bead {
        Bead {
            id: id.into(),
            title: format!("title for {id}"),
            status,
            assignee: assignee.map(|s| s.into()),
            labels: labels.iter().map(|s| (*s).to_string()).collect(),
            priority: None,
            issue_type: None,
        }
    }

    #[test]
    fn decide_spawns_routes_unclaimed_engineer_beads() {
        let beads = vec![
            bead("bd-1", Status::Open, None, &["role:engineer"]),
            bead(
                "bd-2",
                Status::Open,
                None,
                &["role:engineer", "kind:bugfix"],
            ),
        ];
        let routed = decide_spawns(&beads, &HashSet::new(), 0, 4);
        assert_eq!(routed, vec!["bd-1".to_string(), "bd-2".to_string()]);
    }

    #[test]
    fn decide_spawns_skips_beads_with_assignees() {
        let beads = vec![
            bead("bd-1", Status::Open, Some("engineer-1"), &["role:engineer"]),
            bead("bd-2", Status::Open, None, &["role:engineer"]),
        ];
        let routed = decide_spawns(&beads, &HashSet::new(), 0, 4);
        assert_eq!(routed, vec!["bd-2".to_string()]);
    }

    #[test]
    fn decide_spawns_skips_already_routed_ids() {
        let beads = vec![bead("bd-1", Status::Open, None, &["role:engineer"])];
        let mut already = HashSet::new();
        already.insert("bd-1".to_string());
        let routed = decide_spawns(&beads, &already, 0, 4);
        assert!(routed.is_empty(), "already-routed bead must not re-spawn");
    }

    #[test]
    fn decide_spawns_skips_non_engineer_roles() {
        let beads = vec![
            bead("bd-1", Status::Open, None, &["role:architect"]),
            bead("bd-2", Status::Open, None, &["role:recon"]),
            bead("bd-3", Status::Open, None, &["role:engineer"]),
        ];
        let routed = decide_spawns(&beads, &HashSet::new(), 0, 4);
        assert_eq!(routed, vec!["bd-3".to_string()]);
    }

    #[test]
    fn decide_spawns_skips_non_open_status() {
        let beads = vec![
            bead("bd-1", Status::InProgress, None, &["role:engineer"]),
            bead("bd-2", Status::Closed, None, &["role:engineer"]),
            bead("bd-3", Status::Open, None, &["role:engineer"]),
        ];
        let routed = decide_spawns(&beads, &HashSet::new(), 0, 4);
        assert_eq!(routed, vec!["bd-3".to_string()]);
    }

    #[test]
    fn decide_spawns_respects_pool_cap() {
        let beads = vec![
            bead("bd-1", Status::Open, None, &["role:engineer"]),
            bead("bd-2", Status::Open, None, &["role:engineer"]),
            bead("bd-3", Status::Open, None, &["role:engineer"]),
        ];
        // Pool already at 4, max 4 → no headroom.
        let routed = decide_spawns(&beads, &HashSet::new(), 4, 4);
        assert!(routed.is_empty());

        // Pool at 3, max 4 → exactly one slot.
        let routed = decide_spawns(&beads, &HashSet::new(), 3, 4);
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0], "bd-1");

        // Pool at 0, max 2 → two slots; third bead waits.
        let routed = decide_spawns(&beads, &HashSet::new(), 0, 2);
        assert_eq!(routed, vec!["bd-1".to_string(), "bd-2".to_string()]);
    }

    #[test]
    fn decide_spawns_returns_empty_for_empty_board() {
        let routed = decide_spawns(&[], &HashSet::new(), 0, 4);
        assert!(routed.is_empty());
    }

    #[test]
    fn is_open_engineer_bead_requires_role_label() {
        // No role label at all.
        let b = bead("bd-1", Status::Open, None, &["kind:implementation"]);
        assert!(!is_open_engineer_bead(&b));
        // Wrong role.
        let b = bead("bd-2", Status::Open, None, &["role:architect"]);
        assert!(!is_open_engineer_bead(&b));
        // Correct.
        let b = bead("bd-3", Status::Open, None, &["role:engineer"]);
        assert!(is_open_engineer_bead(&b));
    }
}
