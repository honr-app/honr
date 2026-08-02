//! The state machine is the contract. Neither transport owns any of this —
//! REST handlers and MCP tools both route every mutation through here.

use crate::model::{State, WorkItem};

/// Legal edges, straight off the lifecycle diagram.
pub fn allowed(from: State, to: State) -> bool {
    use State::*;

    // Cutting scope is always available, from anywhere that isn't already
    // terminal. Retired is not deleted — the subtree stays visible and greyed.
    if to == Retired {
        return from != Retired;
    }

    match (from, to) {
        (Draft, Shaping) => true,

        (Shaping, Ready) => true,
        // Too ambiguous to split.
        (Shaping, NeedsHuman) => true,

        (Ready, Claimed) => true,
        // Contract rewritten: unclaimed leaves return to shaping.
        (Ready, Shaping) => true,

        (Claimed, Running) => true,
        (Claimed, Splitting) => true,
        (Claimed, NeedsHuman) => true,
        // Graceful release before any work happened.
        (Claimed, Ready) => true,

        // Heartbeat + progress.
        (Running, Running) => true,
        // Lease expired, released, or halted by a human.
        (Running, Ready) => true,
        // Self-orchestration: the work was bigger than the card.
        (Running, Splitting) => true,
        (Running, NeedsHuman) => true,
        // Agent opened a PR — mechanical checks are CI's job, not a board column.
        (Running, Review) => true,

        // Sibling tasks created under the Project; original may requeue or finish.
        (Splitting, Ready) => true,
        (Splitting, Shaping) => true,
        // Flat model: the split card is replaced by siblings, not nested under.
        (Splitting, Done) => true,
        (Splitting, Retired) => true,

        (NeedsHuman, Running) => true,
        // Human reassigns.
        (NeedsHuman, Ready) => true,

        (Review, Done) => true,
        (Shaping, Done) => true,
        (Ready, Done) => true,
        (NeedsHuman, Done) => true,
        (Running, Done) => true,
        (Review, Ready) => true,

        _ => false,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransitionError {
    #[error("illegal transition {from:?} -> {to:?} for #{id}")]
    Illegal { id: u64, from: State, to: State },
    #[error("#{id} has children, so it is a container and cannot be claimed")]
    ContainerNotClaimable { id: u64 },
    #[error("leaf #{id} needs a definition of done before it can leave shaping")]
    LeafNeedsDoD { id: u64 },
    #[error("#{id} is blocked by {blockers:?}")]
    Blocked { id: u64, blockers: Vec<u64> },
    #[error("no work item #{0}")]
    NoSuchItem(u64),
    #[error("#{id} is parked; resume before claiming")]
    Parked { id: u64 },
}

/// States in which an agent is actively holding the card.
fn requires_claimable(s: State) -> bool {
    matches!(s, State::Claimed | State::Running | State::Splitting)
}

/// The whole invariant: loose at the schema, strict at the node.
///
/// `unresolved_blockers` is the subset of `item.blocked_by` that has not
/// reached a terminal state — the caller resolves it because only the board
/// knows sibling states.
pub fn check(
    item: &WorkItem,
    to: State,
    has_children: bool,
    unresolved_blockers: &[u64],
) -> Result<(), TransitionError> {
    if !allowed(item.state, to) {
        return Err(TransitionError::Illegal { id: item.id, from: item.state, to });
    }

    // A node with children is a Project (container); containers are not picked up.
    if has_children && requires_claimable(to) {
        return Err(TransitionError::ContainerNotClaimable { id: item.id });
    }

    // Without this, the tree is a wish list.
    if to == State::Ready && !has_children && item.definition_of_done.is_none() {
        return Err(TransitionError::LeafNeedsDoD { id: item.id });
    }

    if to == State::Claimed && !unresolved_blockers.is_empty() {
        return Err(TransitionError::Blocked {
            id: item.id,
            blockers: unresolved_blockers.to_vec(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{State::*, WorkItem};

    fn leaf() -> WorkItem {
        let mut i = WorkItem::new(1, "t", "intent");
        i.definition_of_done = Some("tests green".into());
        i
    }

    #[test]
    fn happy_path_edges_are_legal() {
        for (a, b) in [
            (Draft, Shaping),
            (Shaping, Ready),
            (Ready, Claimed),
            (Claimed, Running),
            (Running, Review),
            (Review, Done),
        ] {
            assert!(allowed(a, b), "{a:?} -> {b:?} should be legal");
        }
    }

    #[test]
    fn verifying_is_not_a_lifecycle_state() {
        // Mechanical checks are CI. Running goes straight to Review.
        assert!(!allowed(Running, Shaping));
        assert!(allowed(Running, Review));
    }

    #[test]
    fn skipping_the_queue_is_illegal() {
        assert!(!allowed(Ready, Running), "must go through Claimed");
        assert!(!allowed(Draft, Ready), "must be shaped first");
        assert!(!allowed(Draft, Done), "must be shaped first");
    }

    #[test]
    fn done_is_terminal_but_retire_is_always_available() {
        assert!(!allowed(Done, Ready));
        assert!(allowed(Done, Retired));
        assert!(allowed(Running, Retired));
        assert!(!allowed(Retired, Retired));
    }

    #[test]
    fn lease_expiry_and_halt_return_to_ready() {
        assert!(allowed(Running, Ready));
        assert!(allowed(Claimed, Ready));
    }

    #[test]
    fn escalation_round_trips() {
        assert!(allowed(Running, NeedsHuman));
        assert!(allowed(NeedsHuman, Running));
        assert!(allowed(NeedsHuman, Ready));
    }

    #[test]
    fn claimed_can_split_or_escalate() {
        assert!(allowed(Claimed, Splitting));
        assert!(allowed(Claimed, NeedsHuman));
    }

    #[test]
    fn containers_cannot_be_claimed() {
        let item = { let mut i = leaf(); i.state = Ready; i };
        let err = check(&item, Claimed, true, &[]).unwrap_err();
        assert!(matches!(err, TransitionError::ContainerNotClaimable { .. }));
        // The same node without children is fine.
        assert!(check(&item, Claimed, false, &[]).is_ok());
    }

    #[test]
    fn leaves_need_a_definition_of_done_to_reach_ready() {
        let mut item = WorkItem::new(2, "t", "intent");
        item.state = Shaping;
        let err = check(&item, Ready, false, &[]).unwrap_err();
        assert!(matches!(err, TransitionError::LeafNeedsDoD { .. }));

        // Containers are exempt — they are never executed directly.
        assert!(check(&item, Ready, true, &[]).is_ok());

        item.definition_of_done = Some("integration tests green".into());
        assert!(check(&item, Ready, false, &[]).is_ok());
    }

    #[test]
    fn blocked_items_cannot_be_claimed() {
        let item = { let mut i = leaf(); i.state = Ready; i };
        let err = check(&item, Claimed, false, &[41]).unwrap_err();
        assert!(matches!(err, TransitionError::Blocked { .. }));
    }
}
