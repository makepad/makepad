//! Job leases: work lives only while it is renewed (aicore.md §8).
//!
//! The fail-safe direction is the whole design: cancellation must never
//! depend on a message ARRIVING. A "cancel packet stops the job" scheme
//! turns one lost packet into a GPU grinding until someone notices; under a
//! lease, a partition, a crash and a kill -9 all look the same and all stop
//! the work. The origin renews every [`KEEPALIVE_INTERVAL`]; [`MISSED_BEATS`]
//! missed beats and the executor declares the origin dead, cancels the job
//! and frees the lane.
//!
//! 2s is deliberately unambitious: these jobs run for minutes, so a
//! worst-case ~8s of grinding on dead work is noise against the job itself,
//! and a tighter cadence costs traffic on every node for every in-flight job.
//! The keepalive must be renewed ON THE PATH THAT OWNS THE WORK, not by a
//! background timer thread — the beacon happily keeps announcing a process
//! whose session thread is deadlocked; a lease renewed by the owner proves
//! the owner is alive, which is the fact the executor needs.
//!
//! Restart is the subtle case: a process that dies and comes back fast is
//! *present again* while its old work is orphaned. The identity split from
//! `/health` solves it: `node_key` is durable, `epoch` (the per-start
//! node_id) changes — a lease action for a known key under a new epoch means
//! the previous incarnation's jobs are dead, immediately.
//!
//! Everything here is a pure state machine over an injected clock (the store
//! core's law: time never comes from a clock inside). Transport wiring —
//! the keepalive route, the reaper's cancel calls — lives with the server.

use std::collections::HashMap;
use std::time::Duration;

/// Origin renewal cadence.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
/// Missed beats before the executor declares the origin dead.
pub const MISSED_BEATS: u32 = 3;
/// One beat of slack on top, against scheduling jitter on loaded boxes.
pub const SLACK: Duration = Duration::from_secs(2);

/// The deadline a fresh renewal buys: interval × beats + slack (~8s).
pub fn lease_ms() -> u64 {
    KEEPALIVE_INTERVAL.as_millis() as u64 * MISSED_BEATS as u64 + SLACK.as_millis() as u64
}

/// Who owns a piece of work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// Durable identity (the cache-dir `node-key`): stable across restarts.
    pub node_key: String,
    /// Per-start identity (the beacon/`/health` node_id): a new one under
    /// the same key IS the restart signal.
    pub epoch: u64,
}

#[derive(Clone, Debug)]
struct Row {
    origin: Origin,
    deadline_ms: u64,
}

/// Why a job left the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lapse {
    /// No renewal arrived in time — origin unreachable or dead.
    Expired,
    /// The origin came back under a new epoch; this job belonged to the
    /// previous incarnation.
    OwnerRestarted,
    /// The origin said goodbye; release everything now, wait for nothing.
    Bye,
}

/// The outcome of one renewal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Renew {
    Renewed,
    /// Not registered here (already reaped, or never ours) — the origin
    /// should treat the job as gone and re-pick.
    UnknownJob,
    /// Registered, but to a different owner — refused.
    WrongOwner,
}

/// The executor-side table: every remotely-owned job's lease.
#[derive(Default)]
pub struct LeaseTable {
    rows: HashMap<String, Row>,
}

impl LeaseTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register work owned by `origin`. Registering is also the first
    /// renewal. A re-register of a live job by its owner just renews.
    pub fn register(&mut self, job_id: &str, origin: Origin, now_ms: u64) -> Renew {
        if let Some(row) = self.rows.get(job_id) {
            if row.origin.node_key != origin.node_key {
                return Renew::WrongOwner;
            }
        }
        self.rows.insert(
            job_id.to_string(),
            Row {
                origin,
                deadline_ms: now_ms + lease_ms(),
            },
        );
        Renew::Renewed
    }

    /// One keepalive beat from the origin.
    pub fn renew(&mut self, job_id: &str, origin: &Origin, now_ms: u64) -> Renew {
        match self.rows.get_mut(job_id) {
            None => Renew::UnknownJob,
            Some(row) => {
                if row.origin.node_key != origin.node_key || row.origin.epoch != origin.epoch {
                    return Renew::WrongOwner;
                }
                row.deadline_ms = now_ms + lease_ms();
                Renew::Renewed
            }
        }
    }

    /// The job finished or was cancelled locally; forget its lease.
    pub fn release(&mut self, job_id: &str) {
        self.rows.remove(job_id);
    }

    /// Everything whose owner is gone, and why. Call this from the reaper
    /// tick AND from any lease action that reveals a restart; the caller
    /// cancels each returned job through the ordinary cancel path.
    pub fn reap(&mut self, now_ms: u64) -> Vec<(String, Lapse)> {
        let mut lapsed = Vec::new();
        self.rows.retain(|job, row| {
            if row.deadline_ms < now_ms {
                lapsed.push((job.clone(), Lapse::Expired));
                false
            } else {
                true
            }
        });
        lapsed
    }

    /// An origin re-appeared under a new epoch: every job its previous
    /// incarnation owned is dead now, not in ~8 seconds.
    pub fn owner_restarted(&mut self, node_key: &str, new_epoch: u64) -> Vec<(String, Lapse)> {
        let mut lapsed = Vec::new();
        self.rows.retain(|job, row| {
            if row.origin.node_key == node_key && row.origin.epoch != new_epoch {
                lapsed.push((job.clone(), Lapse::OwnerRestarted));
                false
            } else {
                true
            }
        });
        lapsed
    }

    /// Graceful goodbye: release everything this origin owns immediately.
    pub fn bye(&mut self, node_key: &str) -> Vec<(String, Lapse)> {
        let mut lapsed = Vec::new();
        self.rows.retain(|job, row| {
            if row.origin.node_key == node_key {
                lapsed.push((job.clone(), Lapse::Bye));
                false
            } else {
                true
            }
        });
        lapsed
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(key: &str, epoch: u64) -> Origin {
        Origin {
            node_key: key.into(),
            epoch,
        }
    }

    #[test]
    fn a_renewed_job_lives_and_a_silent_one_dies() {
        let mut table = LeaseTable::new();
        assert_eq!(table.register("j1", origin("a", 1), 1000), Renew::Renewed);
        // Renewed before the deadline: survives the reaper.
        assert_eq!(table.renew("j1", &origin("a", 1), 1000 + 6000), Renew::Renewed);
        assert!(table.reap(1000 + 6000 + lease_ms() - 1).is_empty());
        // Silence past the deadline: reaped as Expired.
        let lapsed = table.reap(1000 + 6000 + lease_ms() + 1);
        assert_eq!(lapsed, vec![("j1".to_string(), Lapse::Expired)]);
        assert!(table.is_empty());
        // A late renewal after the reap tells the origin to re-pick.
        assert_eq!(table.renew("j1", &origin("a", 1), 99999), Renew::UnknownJob);
    }

    #[test]
    fn a_restarted_owner_kills_its_previous_incarnations_jobs_at_once() {
        let mut table = LeaseTable::new();
        table.register("j1", origin("a", 1), 0);
        table.register("j2", origin("a", 1), 0);
        table.register("j3", origin("b", 7), 0);
        let lapsed = table.owner_restarted("a", 2);
        assert_eq!(lapsed.len(), 2);
        assert!(lapsed.iter().all(|(_, why)| *why == Lapse::OwnerRestarted));
        assert_eq!(table.len(), 1, "the other origin's job is untouched");
        // The new incarnation registers fresh work fine.
        assert_eq!(table.register("j4", origin("a", 2), 0), Renew::Renewed);
    }

    #[test]
    fn bye_releases_everything_immediately() {
        let mut table = LeaseTable::new();
        table.register("j1", origin("a", 1), 0);
        table.register("j2", origin("b", 1), 0);
        let lapsed = table.bye("a");
        assert_eq!(lapsed, vec![("j1".to_string(), Lapse::Bye)]);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn a_foreign_renewal_is_refused_not_absorbed() {
        let mut table = LeaseTable::new();
        table.register("j1", origin("a", 1), 0);
        assert_eq!(table.renew("j1", &origin("b", 1), 10), Renew::WrongOwner);
        assert_eq!(table.renew("j1", &origin("a", 2), 10), Renew::WrongOwner);
        assert_eq!(
            table.register("j1", origin("b", 1), 10),
            Renew::WrongOwner,
            "re-register by a different key must not steal the job"
        );
    }

    #[test]
    fn the_deadline_is_three_beats_plus_slack() {
        assert_eq!(lease_ms(), 8000);
    }
}
