//! Job graph behavior: claim ordering, dependency gating, leases and
//! heartbeats, retry/exhaustion, hierarchical cancellation, restart recovery.

mod common;
use common::*;
use makepad_asset_store::{
    AssetServerCore, Budgets, JobState, NewJob, ServerError,
};

fn job<'a>(id: u8, kind: &'a str, deps: &'a [makepad_asset_store::JobId]) -> NewJob<'a> {
    NewJob {
        job_id: jid(id),
        parent: None,
        kind,
        payload: b"payload",
        priority: 0,
        max_attempts: 3,
        not_before_ms: 0,
        deps,
    }
}

#[test]
fn claim_orders_by_priority_then_age() {
    let (_root, core) = open_core("order");
    let jobs = core.jobs();
    jobs.enqueue(&job(1, "convert", &[]), NOW).unwrap();
    jobs.enqueue(&NewJob { priority: 5, ..job(2, "convert", &[]) }, NOW).unwrap();
    jobs.enqueue(&job(3, "convert", &[]), NOW).unwrap();

    let lease = 60_000;
    let first = jobs.claim("w1", NOW, lease).unwrap().unwrap();
    assert_eq!(first.job_id, jid(2), "highest priority first");
    let second = jobs.claim("w1", NOW, lease).unwrap().unwrap();
    assert_eq!(second.job_id, jid(1), "then oldest/lowest id");
    let third = jobs.claim("w1", NOW, lease).unwrap().unwrap();
    assert_eq!(third.job_id, jid(3));
    assert_eq!(third.kind, "convert");
    assert_eq!(third.payload, b"payload");
    assert!(jobs.claim("w1", NOW, lease).unwrap().is_none());
}

#[test]
fn allowed_kind_claim_never_consumes_foreign_worker_jobs() {
    let (_root, core) = open_core("claim_kinds");
    let jobs = core.jobs();
    jobs.enqueue(&NewJob { priority: 50, ..job(1, "music.generate", &[]) }, NOW)
        .unwrap();
    jobs.enqueue(&job(2, "video.generate", &[]), NOW).unwrap();

    let video = jobs
        .claim_allowed("video-worker", NOW, 60_000, &["video.generate"])
        .unwrap()
        .expect("video job");
    assert_eq!(video.job_id, jid(2));
    assert_eq!(jobs.state(&jid(1)).unwrap(), Some(JobState::Pending));

    let music = jobs.claim("music-worker", NOW + 1, 60_000).unwrap().expect("music remains");
    assert_eq!(music.job_id, jid(1));
    assert!(matches!(
        jobs.claim_allowed("w", NOW, 60_000, &[]),
        Err(ServerError::InvalidInput { .. })
    ));
    assert!(matches!(
        jobs.claim_allowed("w", NOW, 60_000, &["video.generate", "video.generate"]),
        Err(ServerError::InvalidInput { .. })
    ));
}

#[test]
fn dependencies_gate_claims() {
    let (_root, core) = open_core("deps");
    let jobs = core.jobs();
    jobs.enqueue(&job(1, "produce", &[]), NOW).unwrap();
    let deps = [jid(1)];
    jobs.enqueue(&job(2, "consume", &deps), NOW).unwrap();

    let claimed = jobs.claim("w1", NOW, 1000).unwrap().unwrap();
    assert_eq!(claimed.job_id, jid(1));
    // The dependent is not runnable while its dep is running.
    assert!(jobs.claim("w2", NOW, 1000).unwrap().is_none());
    jobs.succeed(&jid(1), "w1", NOW + 10).unwrap();
    let next = jobs.claim("w2", NOW + 10, 1000).unwrap().unwrap();
    assert_eq!(next.job_id, jid(2));
}

#[test]
fn dependency_failure_cancels_dependents_transitively() {
    let (_root, core) = open_core("doomed");
    let jobs = core.jobs();
    jobs.enqueue(&NewJob { max_attempts: 1, ..job(1, "root", &[]) }, NOW).unwrap();
    let d1 = [jid(1)];
    jobs.enqueue(&job(2, "mid", &d1), NOW).unwrap();
    let d2 = [jid(2)];
    jobs.enqueue(&job(3, "leaf", &d2), NOW).unwrap();

    let claimed = jobs.claim("w1", NOW, 1000).unwrap().unwrap();
    assert_eq!(claimed.job_id, jid(1));
    assert_eq!(jobs.fail(&jid(1), "w1", NOW + 1, 0).unwrap(), JobState::Failed);

    // Propagation happens lazily at claim time, transitively.
    assert!(jobs.claim("w1", NOW + 2, 1000).unwrap().is_none());
    assert_eq!(jobs.state(&jid(2)).unwrap(), Some(JobState::Cancelled));
    assert_eq!(jobs.state(&jid(3)).unwrap(), Some(JobState::Cancelled));
}

#[test]
fn heartbeat_extends_and_expired_lease_refuses_results() {
    let (_root, core) = open_core("lease");
    let jobs = core.jobs();
    jobs.enqueue(&job(1, "work", &[]), NOW).unwrap();
    let claimed = jobs.claim("w1", NOW, 1000).unwrap().unwrap();
    assert_eq!(claimed.lease_expires_ms, NOW + 1000);

    let extended = jobs.heartbeat(&jid(1), "w1", NOW + 500, 1000).unwrap();
    assert_eq!(extended, NOW + 1500);

    // Another worker cannot heartbeat someone else's lease.
    assert!(matches!(
        jobs.heartbeat(&jid(1), "w2", NOW + 600, 1000).unwrap_err(),
        ServerError::LeaseLost { .. }
    ));

    // Past expiry every report from the old holder is refused.
    assert!(matches!(
        jobs.heartbeat(&jid(1), "w1", NOW + 2000, 1000).unwrap_err(),
        ServerError::LeaseLost { what: "lease expired" }
    ));
    assert!(matches!(
        jobs.succeed(&jid(1), "w1", NOW + 2000).unwrap_err(),
        ServerError::LeaseLost { what: "lease expired" }
    ));
}

#[test]
fn expired_lease_requeues_and_stale_worker_cannot_overwrite() {
    let (_root, core) = open_core("expiry");
    let jobs = core.jobs();
    jobs.enqueue(&job(1, "work", &[]), NOW).unwrap();
    jobs.claim("w1", NOW, 1000).unwrap().unwrap();

    assert_eq!(jobs.expire_leases(NOW + 2000).unwrap(), 1);
    assert_eq!(jobs.state(&jid(1)).unwrap(), Some(JobState::Pending));

    let reclaimed = jobs.claim("w2", NOW + 2000, 1000).unwrap().unwrap();
    assert_eq!(reclaimed.attempt, 2);

    // The stale worker's late success is refused; the live one lands.
    assert!(matches!(
        jobs.succeed(&jid(1), "w1", NOW + 2100).unwrap_err(),
        ServerError::LeaseLost { .. }
    ));
    jobs.succeed(&jid(1), "w2", NOW + 2200).unwrap();
    assert_eq!(jobs.state(&jid(1)).unwrap(), Some(JobState::Succeeded));

    let attempts = jobs.attempts(&jid(1)).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].attempt, 1);
    assert!(attempts[0].ended_ms.is_some(), "expired attempt closed");
    assert_eq!(attempts[1].attempt, 2);
    assert!(attempts[1].ended_ms.is_some());
}

#[test]
fn fail_retries_with_backoff_until_attempts_exhausted() {
    let (_root, core) = open_core("retry");
    let jobs = core.jobs();
    jobs.enqueue(&NewJob { max_attempts: 2, ..job(1, "flaky", &[]) }, NOW).unwrap();

    jobs.claim("w1", NOW, 1000).unwrap().unwrap();
    assert_eq!(jobs.fail(&jid(1), "w1", NOW + 10, 5000).unwrap(), JobState::Pending);
    // Backoff: not runnable before not_before_ms.
    assert!(jobs.claim("w1", NOW + 100, 1000).unwrap().is_none());
    let second = jobs.claim("w1", NOW + 5010, 1000).unwrap().unwrap();
    assert_eq!(second.attempt, 2);
    assert_eq!(jobs.fail(&jid(1), "w1", NOW + 5020, 5000).unwrap(), JobState::Failed);
    assert_eq!(jobs.state(&jid(1)).unwrap(), Some(JobState::Failed));
    assert!(jobs.claim("w1", NOW + 99_000, 1000).unwrap().is_none());
}

#[test]
fn cancel_cascades_through_the_parent_tree() {
    let (_root, core) = open_core("cancel");
    let jobs = core.jobs();
    jobs.enqueue(&job(1, "parent", &[]), NOW).unwrap();
    jobs.enqueue(&NewJob { parent: Some(jid(1)), ..job(2, "child-running", &[]) }, NOW).unwrap();
    jobs.enqueue(&NewJob { parent: Some(jid(1)), ..job(3, "child-pending", &[]) }, NOW).unwrap();
    jobs.enqueue(&NewJob { parent: Some(jid(2)), ..job(4, "grandchild", &[]) }, NOW).unwrap();
    jobs.enqueue(&NewJob { parent: Some(jid(1)), ..job(5, "child-pending-2", &[]) }, NOW).unwrap();

    // Finish the parent and child 3, leave child 2 running, 4 and 5 pending.
    let c1 = jobs.claim("w1", NOW, 60_000).unwrap().unwrap();
    assert_eq!(c1.job_id, jid(1));
    jobs.succeed(&jid(1), "w1", NOW + 1).unwrap();
    let c2 = jobs.claim("w1", NOW + 2, 60_000).unwrap().unwrap();
    assert_eq!(c2.job_id, jid(2));
    let c3 = jobs.claim("w2", NOW + 3, 60_000).unwrap().unwrap();
    assert_eq!(c3.job_id, jid(3));
    jobs.succeed(&jid(3), "w2", NOW + 4).unwrap();

    let cancelled = jobs.cancel(&jid(1), NOW + 10).unwrap();
    // Job 1 already succeeded, job 3 already succeeded: cancelled are 2, 4, 5.
    assert_eq!(cancelled, 3);
    assert_eq!(jobs.state(&jid(1)).unwrap(), Some(JobState::Succeeded));
    assert_eq!(jobs.state(&jid(2)).unwrap(), Some(JobState::Cancelled));
    assert_eq!(jobs.state(&jid(3)).unwrap(), Some(JobState::Succeeded));
    assert_eq!(jobs.state(&jid(4)).unwrap(), Some(JobState::Cancelled));
    assert_eq!(jobs.state(&jid(5)).unwrap(), Some(JobState::Cancelled));

    // The running child's worker lost its lease with the cancellation.
    assert!(matches!(
        jobs.succeed(&jid(2), "w1", NOW + 20).unwrap_err(),
        ServerError::LeaseLost { .. }
    ));
    assert!(jobs.claim("w1", NOW + 30, 1000).unwrap().is_none());
}

#[test]
fn enqueue_admission_is_fail_closed() {
    let root = test_root("admission");
    let budgets = Budgets {
        max_job_payload_bytes: 4,
        max_job_deps: 1,
        max_job_depth: 1,
        max_attempts: 2,
        ..Budgets::default_v1()
    };
    let core = AssetServerCore::open(&root, budgets).unwrap();
    let jobs = core.jobs();

    // Unknown dependency refuses.
    let deps = [jid(9)];
    assert!(matches!(
        jobs.enqueue(&NewJob { max_attempts: 2, payload: b"ok", ..job(1, "a", &deps) }, NOW).unwrap_err(),
        ServerError::NotFound { what: "job dependency" }
    ));
    jobs.enqueue(&NewJob { max_attempts: 2, payload: b"ok", ..job(1, "a", &[]) }, NOW).unwrap();
    // Duplicate id refuses.
    assert!(matches!(
        jobs.enqueue(&NewJob { max_attempts: 2, payload: b"ok", ..job(1, "a", &[]) }, NOW).unwrap_err(),
        ServerError::Conflict { what: "job id" }
    ));
    // Payload budget.
    assert!(matches!(
        jobs.enqueue(&NewJob { max_attempts: 2, payload: b"five!", ..job(2, "a", &[]) }, NOW).unwrap_err(),
        ServerError::OverBudget { what: "job payload bytes", .. }
    ));
    // Deps budget.
    let two_deps = [jid(1), jid(1)];
    assert!(matches!(
        jobs.enqueue(&NewJob { max_attempts: 2, payload: b"ok", ..job(3, "a", &two_deps) }, NOW).unwrap_err(),
        ServerError::OverBudget { what: "job dependencies", .. }
    ));
    // Attempts ceiling.
    assert!(matches!(
        jobs.enqueue(&NewJob { max_attempts: 3, payload: b"ok", ..job(4, "a", &[]) }, NOW).unwrap_err(),
        ServerError::OverBudget { what: "job max_attempts", .. }
    ));
    // Kind charset.
    assert!(matches!(
        jobs.enqueue(&NewJob { max_attempts: 2, payload: b"ok", ..job(5, "Bad Kind", &[]) }, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "job kind charset" }
    ));
    // Depth budget 1 = children allowed, grandchildren refused.
    jobs.enqueue(&NewJob { parent: Some(jid(1)), max_attempts: 2, payload: b"ok", ..job(6, "a", &[]) }, NOW).unwrap();
    assert!(matches!(
        jobs.enqueue(&NewJob { parent: Some(jid(6)), max_attempts: 2, payload: b"ok", ..job(7, "a", &[]) }, NOW).unwrap_err(),
        ServerError::OverBudget { what: "job depth", .. }
    ));
    // Zero lease refused.
    assert!(matches!(
        jobs.claim("w1", NOW, 0).unwrap_err(),
        ServerError::InvalidInput { what: "zero lease" }
    ));
}

#[test]
fn lease_boundary_instant_is_expired_everywhere() {
    let (_root, core) = open_core("lease_boundary");
    let jobs = core.jobs();
    jobs.enqueue(&job(1, "work", &[]), NOW).unwrap();
    let claimed = jobs.claim("w1", NOW, 1000).unwrap().unwrap();
    let boundary = claimed.lease_expires_ms;
    assert_eq!(boundary, NOW + 1000);

    // At exactly expires_ms every report from the holder is refused...
    assert!(matches!(
        jobs.heartbeat(&jid(1), "w1", boundary, 1000).unwrap_err(),
        ServerError::LeaseLost { what: "lease expired" }
    ));
    assert!(matches!(
        jobs.succeed(&jid(1), "w1", boundary).unwrap_err(),
        ServerError::LeaseLost { what: "lease expired" }
    ));
    assert!(matches!(
        jobs.fail(&jid(1), "w1", boundary, 0).unwrap_err(),
        ServerError::LeaseLost { what: "lease expired" }
    ));
    // ...and the reaper already collects at that same instant: no moment
    // exists where both the stale holder and the reaper may act.
    assert_eq!(jobs.expire_leases(boundary).unwrap(), 1);
    assert_eq!(jobs.state(&jid(1)).unwrap(), Some(JobState::Pending));
    let reclaimed = jobs.claim("w2", boundary, 1000).unwrap().unwrap();
    assert_eq!(reclaimed.attempt, 2);
    // One millisecond before the new expiry the lease is still live.
    jobs.heartbeat(&jid(1), "w2", boundary + 999, 1000).unwrap();
}

#[test]
fn hostile_timestamps_and_retry_delays_refuse_structured() {
    let (_root, core) = open_core("hostile_time");
    let jobs = core.jobs();

    // A not_before beyond the i64 storage domain refuses at admission
    // instead of truncating into the INTEGER column.
    assert!(matches!(
        jobs.enqueue(&NewJob { not_before_ms: u64::MAX, ..job(1, "work", &[]) }, NOW)
            .unwrap_err(),
        ServerError::InvalidInput { what: "u64 value exceeds i64 range" }
    ));
    jobs.enqueue(&job(1, "work", &[]), NOW).unwrap();

    // An absurd clock refuses the claim with a structured error (the
    // u64->i64 storage check; lease_expiry's checked_add backstops behind
    // it) and rolls back, leaving the job claimable at a sane time.
    assert!(matches!(
        jobs.claim("w1", u64::MAX - 10, 1000).unwrap_err(),
        ServerError::InvalidInput { .. }
    ));
    let claimed = jobs.claim("w1", NOW, 1000).unwrap().unwrap();
    assert_eq!(claimed.attempt, 1);

    // Retry delay beyond the budget refuses before touching lease state.
    let over = Budgets::default_v1().max_retry_delay_ms + 1;
    assert!(matches!(
        jobs.fail(&jid(1), "w1", NOW + 10, over).unwrap_err(),
        ServerError::OverBudget { what: "retry delay ms", .. }
    ));
    // The lease survived that refusal; an in-budget failure still lands.
    assert_eq!(jobs.fail(&jid(1), "w1", NOW + 20, 5000).unwrap(), JobState::Pending);
}

#[test]
fn restart_recovery_expires_orphan_leases() {
    let root = test_root("restart");
    {
        let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
        core.jobs().enqueue(&job(1, "work", &[]), NOW).unwrap();
        core.jobs().claim("w1", NOW, 1000).unwrap().unwrap();
        // Process dies here with the lease outstanding.
    }
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    let report = core.recover(NOW + 5000).unwrap();
    assert_eq!(report.leases_expired, 1);
    assert_eq!(core.jobs().state(&jid(1)).unwrap(), Some(JobState::Pending));
    let reclaimed = core.jobs().claim("w2", NOW + 5000, 1000).unwrap().unwrap();
    assert_eq!(reclaimed.attempt, 2);
    core.jobs().succeed(&jid(1), "w2", NOW + 5100).unwrap();
    assert_eq!(core.jobs().state(&jid(1)).unwrap(), Some(JobState::Succeeded));
}
