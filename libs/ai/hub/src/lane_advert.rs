//! What this box advertises about its concurrent decode capacity.
//!
//! The LLM backend publishes here when a model becomes resident and updates it
//! as lanes are claimed and released; `/health` reads it. A process-global cell
//! is the right shape because the residency policy is one heavy resident per
//! box — there is exactly one set of lane facts to describe at a time.
//!
//! The alternative, threading a handle from `ServiceShared` down through the
//! per-model backend objects, would couple the backend to the service just to
//! move five integers.
//!
//! **Absence is meaningful and is the compatibility story**: no advert means
//! "one lane, no queue, unknown context ceiling", which is exactly how every
//! service predating this behaved. A mixed fleet needs no flag day.

use std::sync::{Mutex, OnceLock};

/// Lane facts owned by whichever model is resident.
#[derive(Clone, Debug)]
pub struct LaneFacts {
    /// Resident model these facts belong to.
    pub model: String,
    /// Lanes this service is configured for.
    pub slots_total: u64,
    /// Lanes holding a conversation's KV, generating or not.
    pub slots_claimed: u64,
    /// Lanes generating at this instant — the contention signal.
    pub lanes_active: u64,
    /// Hard per-conversation token ceiling.
    pub context_per_slot: u64,
}

impl LaneFacts {
    /// A freshly loaded model: every lane free, nothing generating.
    pub fn idle(model: impl Into<String>, slots_total: u64, context_per_slot: u64) -> Self {
        Self {
            model: model.into(),
            slots_total: slots_total.max(1),
            slots_claimed: 0,
            lanes_active: 0,
            context_per_slot,
        }
    }

    pub fn slots_free(&self) -> u64 {
        self.slots_total.saturating_sub(self.slots_claimed)
    }
}

fn cell() -> &'static Mutex<Option<LaneFacts>> {
    static CELL: OnceLock<Mutex<Option<LaneFacts>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Publish the lane facts for a newly resident model.
pub fn publish(facts: LaneFacts) {
    if let Ok(mut slot) = cell().lock() {
        *slot = Some(facts);
    }
}

/// Drop the advert when the model unloads. A box with nothing resident has no
/// lanes to describe, and saying "0 of 4 free" would read as "busy" rather than
/// "not serving this".
pub fn clear() {
    if let Ok(mut slot) = cell().lock() {
        *slot = None;
    }
}

/// Current advert, or `None` when no LLM is resident.
pub fn snapshot() -> Option<LaneFacts> {
    cell().lock().ok().and_then(|slot| slot.clone())
}

/// Update the live counters without disturbing the model identity or capacity.
/// Counts are clamped to the configured total so the advert can never claim
/// more lanes are busy than exist.
pub fn set_live(slots_claimed: u64, lanes_active: u64) {
    if let Ok(mut slot) = cell().lock() {
        if let Some(facts) = slot.as_mut() {
            facts.slots_claimed = slots_claimed.min(facts.slots_total);
            facts.lanes_active = lanes_active.min(facts.slots_claimed);
        }
    }
}

/// Mark one more lane as generating, claiming it if it was not already.
///
/// The single-lane worker calls this around a turn; the batched worker will
/// call `set_live` from its scheduler instead.
pub fn lane_entered() {
    if let Ok(mut slot) = cell().lock() {
        if let Some(facts) = slot.as_mut() {
            facts.lanes_active = (facts.lanes_active + 1).min(facts.slots_total);
            facts.slots_claimed = facts.slots_claimed.max(facts.lanes_active);
        }
    }
}

/// A lane stopped generating. It stays CLAIMED — the conversation's KV is still
/// resident and its next turn should come back to it. Claimed-but-idle costs no
/// speed, which is exactly why the two counters are separate.
pub fn lane_left() {
    if let Ok(mut slot) = cell().lock() {
        if let Some(facts) = slot.as_mut() {
            facts.lanes_active = facts.lanes_active.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cell is process-global, so these run under one lock to stay
    /// independent of test ordering.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn absent_until_a_model_is_resident() {
        let _g = guard();
        clear();
        assert!(
            snapshot().is_none(),
            "a box with nothing resident must advertise no lanes at all, not zero free ones"
        );
    }

    #[test]
    fn a_fresh_model_advertises_every_lane_free() {
        let _g = guard();
        clear();
        publish(LaneFacts::idle("qwen3.8-27b", 4, 16384));
        let facts = snapshot().expect("advert");
        assert_eq!(facts.slots_total, 4);
        assert_eq!(facts.slots_free(), 4);
        assert_eq!(facts.lanes_active, 0);
        assert_eq!(facts.context_per_slot, 16384);
        clear();
    }

    #[test]
    fn an_idle_lane_stays_claimed_after_its_turn_ends() {
        let _g = guard();
        clear();
        publish(LaneFacts::idle("qwen3.8-27b", 4, 16384));
        lane_entered();
        let busy = snapshot().expect("advert");
        assert_eq!(busy.lanes_active, 1);
        assert_eq!(busy.slots_claimed, 1);

        lane_left();
        let parked = snapshot().expect("advert");
        assert_eq!(
            parked.lanes_active, 0,
            "a finished turn stops contributing contention"
        );
        assert_eq!(
            parked.slots_claimed, 1,
            "but its KV is still resident, so the lane stays claimed"
        );
        assert_eq!(parked.slots_free(), 3);
        clear();
    }

    #[test]
    fn counters_cannot_exceed_the_configured_capacity() {
        let _g = guard();
        clear();
        publish(LaneFacts::idle("qwen3.8-27b", 2, 8192));
        for _ in 0..5 {
            lane_entered();
        }
        let facts = snapshot().expect("advert");
        assert_eq!(facts.lanes_active, 2, "cannot generate on lanes that do not exist");
        assert_eq!(facts.slots_claimed, 2);
        assert_eq!(facts.slots_free(), 0);

        set_live(99, 99);
        let clamped = snapshot().expect("advert");
        assert_eq!(clamped.slots_claimed, 2);
        assert_eq!(clamped.lanes_active, 2);
        clear();
    }

    #[test]
    fn active_never_exceeds_claimed() {
        let _g = guard();
        clear();
        publish(LaneFacts::idle("qwen3.8-27b", 4, 8192));
        // A scheduler reporting more generating than claimed is incoherent;
        // the advert must not pass it on.
        set_live(2, 4);
        let facts = snapshot().expect("advert");
        assert_eq!(facts.slots_claimed, 2);
        assert_eq!(facts.lanes_active, 2);
        clear();
    }

    #[test]
    fn a_model_with_no_lanes_configured_still_reports_one() {
        let _g = guard();
        clear();
        publish(LaneFacts::idle("qwen3.8-27b", 0, 8192));
        assert_eq!(snapshot().expect("advert").slots_total, 1);
        clear();
    }
}
