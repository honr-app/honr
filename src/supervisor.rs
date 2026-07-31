//! The execution side of the board.
//!
//! An agent is **material, not a participant in the control plane**: it gets no
//! network path to honr, and the supervisor drives `claim`/`heartbeat`/`report`
//! on its behalf. An agent that could reach honr's MCP could approve its own
//! review.
//!
//! Right now this holds only the lease sweeper. The per-card lifecycle —
//! sandbox create, briefing, `claude -p`, verdict files, gates — lands here
//! next; see `docs/phase-0-findings.md` for the proven incantation.

use crate::schema::ExecutionConfig;
use crate::store::SharedBoard;

use std::time::Duration;

pub fn spawn(board: SharedBoard, cfg: ExecutionConfig) {
    tokio::spawn(sweeper_loop(board, cfg));
}

/// What makes pull-based dispatch survivable: a dead agent simply stops
/// renewing and the card returns to Ready. No orphan-cleanup job, no
/// supervisor involvement in the common case.
async fn sweeper_loop(board: SharedBoard, cfg: ExecutionConfig) {
    let mut t = tokio::time::interval(Duration::from_millis(cfg.sweep_interval_ms));
    loop {
        t.tick().await;
        for id in board.sweep_leases() {
            tracing::info!("lease expired on #{id}; requeued");
        }
    }
}
