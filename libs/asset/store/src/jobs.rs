//! Durable operation/job graph: dependencies, attempts, leases, hierarchy.
//!
//! Model:
//! - A job is durable work with an opaque payload. Jobs form two graphs at
//!   once: a dependency DAG (edges may only point at already-existing jobs,
//!   so cycles are impossible by construction) and a parent tree used for
//!   hierarchical cancellation.
//! - Workers never own jobs; they hold time-bounded LEASES. Progress requires
//!   a live lease: heartbeat extends it, and success/failure reports are
//!   refused (`LeaseLost`) once the lease expired or the job was cancelled —
//!   a worker that stalls past its lease can never overwrite the outcome of
//!   the retry that replaced it.
//! - Every execution is a recorded attempt row. Failing with attempts left
//!   re-queues with an explicit backoff; exhausting attempts is terminal.
//! - A failed or cancelled dependency cancels dependents (propagated lazily
//!   at claim time, transitively).
//! - All timestamps come in as explicit `now_ms`; the store never reads a
//!   clock, so restart recovery and tests are deterministic.

use crate::budget::Budgets;
use crate::error::{ServerError, ServerResult};
use crate::sqlite::Db;

pub const JOBS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS jobs(
    job_id BLOB PRIMARY KEY,
    parent_job BLOB,
    kind TEXT NOT NULL,
    payload BLOB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','running','succeeded','failed','cancelled')),
    attempts_used INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    not_before_ms INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS jobs_by_state ON jobs(state, not_before_ms);
CREATE INDEX IF NOT EXISTS jobs_by_parent ON jobs(parent_job);
CREATE TABLE IF NOT EXISTS job_deps(
    job_id BLOB NOT NULL,
    depends_on BLOB NOT NULL,
    PRIMARY KEY(job_id, depends_on)
);
CREATE TABLE IF NOT EXISTS job_attempts(
    job_id BLOB NOT NULL,
    attempt INTEGER NOT NULL,
    worker TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    ended_ms INTEGER,
    outcome TEXT NOT NULL DEFAULT 'running',
    PRIMARY KEY(job_id, attempt)
);
CREATE TABLE IF NOT EXISTS job_leases(
    job_id BLOB PRIMARY KEY,
    worker TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL,
    heartbeat_ms INTEGER NOT NULL
);
";

/// Job identity, minted by the caller (the transport layer owns randomness;
/// the core stays deterministic).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

pub struct NewJob<'a> {
    pub job_id: JobId,
    pub parent: Option<JobId>,
    /// Short machine-readable kind, `[a-z0-9_.-]`, 1..=64 bytes.
    pub kind: &'a str,
    pub payload: &'a [u8],
    pub priority: i64,
    pub max_attempts: u32,
    pub not_before_ms: u64,
    pub deps: &'a [JobId],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedJob {
    pub job_id: JobId,
    pub kind: String,
    pub payload: Vec<u8>,
    pub attempt: u32,
    pub lease_expires_ms: u64,
}

/// Jobs of one kind, counted by state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobCounts {
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptRow {
    pub attempt: u32,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
}

pub struct Jobs<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

impl<'a> Jobs<'a> {
    // ---- admission ---------------------------------------------------------

    pub fn enqueue(&self, job: &NewJob<'_>, now_ms: u64) -> ServerResult<()> {
        validate_kind(job.kind)?;
        if job.payload.len() as u64 > self.budgets.max_job_payload_bytes {
            return Err(ServerError::OverBudget {
                what: "job payload bytes",
                limit: self.budgets.max_job_payload_bytes,
                found: job.payload.len() as u64,
            });
        }
        if job.deps.len() as u64 > self.budgets.max_job_deps as u64 {
            return Err(ServerError::OverBudget {
                what: "job dependencies",
                limit: self.budgets.max_job_deps as u64,
                found: job.deps.len() as u64,
            });
        }
        if job.max_attempts == 0 || job.max_attempts > self.budgets.max_attempts {
            return Err(ServerError::OverBudget {
                what: "job max_attempts",
                limit: self.budgets.max_attempts as u64,
                found: job.max_attempts as u64,
            });
        }
        self.db.tx(|db| {
            if self.state(&job.job_id)?.is_some() {
                return Err(ServerError::Conflict { what: "job id" });
            }
            if let Some(parent) = &job.parent {
                let pstate = self
                    .state(parent)?
                    .ok_or(ServerError::NotFound { what: "parent job" })?;
                if pstate.is_terminal() {
                    return Err(ServerError::InvalidState {
                        what: "parent job",
                        state: pstate.as_str(),
                    });
                }
                // Depth budget: walk up the parent chain.
                let mut depth = 1u32;
                let mut cursor = *parent;
                while let Some(next) = self.parent_of(&cursor)? {
                    depth += 1;
                    if depth > self.budgets.max_job_depth {
                        return Err(ServerError::OverBudget {
                            what: "job depth",
                            limit: self.budgets.max_job_depth as u64,
                            found: depth as u64,
                        });
                    }
                    cursor = next;
                }
            }
            for dep in job.deps {
                if self.state(dep)?.is_none() {
                    return Err(ServerError::NotFound { what: "job dependency" });
                }
            }
            let mut s = db.prepare(
                "enqueue job",
                "INSERT INTO jobs(job_id, parent_job, kind, payload, state, attempts_used,
                                  max_attempts, priority, not_before_ms, created_ms, updated_ms)
                 VALUES(?1, ?2, ?3, ?4, 'pending', 0, ?5, ?6, ?7, ?8, ?8)",
            )?;
            s.bind_blob(1, &job.job_id.0)?;
            match &job.parent {
                Some(p) => s.bind_blob(2, &p.0)?,
                None => s.bind_null(2)?,
            }
            s.bind_text(3, job.kind)?;
            s.bind_blob(4, job.payload)?;
            s.bind_i64(5, job.max_attempts as i64)?;
            s.bind_i64(6, job.priority)?;
            s.bind_u64(7, job.not_before_ms)?;
            s.bind_u64(8, now_ms)?;
            s.run()?;
            drop(s);
            for dep in job.deps {
                let mut s = db.prepare(
                    "enqueue job dep",
                    "INSERT OR IGNORE INTO job_deps(job_id, depends_on) VALUES(?1, ?2)",
                )?;
                s.bind_blob(1, &job.job_id.0)?;
                s.bind_blob(2, &dep.0)?;
                s.run()?;
            }
            Ok(())
        })
    }

    // ---- worker protocol ---------------------------------------------------

    /// Claim the best runnable job for `worker`: highest priority, then
    /// oldest. Takes a lease and opens an attempt. Returns None when nothing
    /// is runnable. Also propagates dependency failure/cancellation first, so
    /// doomed jobs never run.
    pub fn claim(&self, worker: &str, now_ms: u64, lease_ms: u64) -> ServerResult<Option<ClaimedJob>> {
        self.claim_inner(worker, now_ms, lease_ms, None)
    }

    /// Claim only jobs whose kind is in `allowed_kinds`. The list is strict,
    /// non-empty and duplicate-free; filtering happens inside the same
    /// transaction as lease acquisition, so a specialized worker can never
    /// consume another worker family's job.
    pub fn claim_allowed(
        &self,
        worker: &str,
        now_ms: u64,
        lease_ms: u64,
        allowed_kinds: &[&str],
    ) -> ServerResult<Option<ClaimedJob>> {
        if allowed_kinds.is_empty() {
            return Err(ServerError::InvalidInput { what: "worker allowed kinds empty" });
        }
        if allowed_kinds.len() > 32 {
            return Err(ServerError::OverBudget {
                what: "worker allowed kinds",
                limit: 32,
                found: allowed_kinds.len() as u64,
            });
        }
        for (index, kind) in allowed_kinds.iter().enumerate() {
            validate_kind(kind)?;
            if allowed_kinds[..index].contains(kind) {
                return Err(ServerError::InvalidInput { what: "worker allowed kinds duplicate" });
            }
        }
        self.claim_inner(worker, now_ms, lease_ms, Some(allowed_kinds))
    }

    fn claim_inner(
        &self,
        worker: &str,
        now_ms: u64,
        lease_ms: u64,
        allowed_kinds: Option<&[&str]>,
    ) -> ServerResult<Option<ClaimedJob>> {
        validate_worker(worker)?;
        self.check_lease_ms(lease_ms)?;
        self.db.tx(|db| {
            // Transitive lazy propagation: a pending job with any failed or
            // cancelled dependency becomes cancelled; loop to closure.
            loop {
                db.prepare(
                    "cancel doomed jobs",
                    "UPDATE jobs SET state='cancelled', updated_ms=?1
                     WHERE state='pending' AND EXISTS(
                        SELECT 1 FROM job_deps d
                        JOIN jobs dj ON dj.job_id = d.depends_on
                        WHERE d.job_id = jobs.job_id
                          AND dj.state IN ('failed','cancelled'))",
                )
                .and_then(|mut s| {
                    s.bind_u64(1, now_ms)?;
                    s.run()
                })?;
                if db.changes() == 0 {
                    break;
                }
            }
            let mut sql = String::from(
                "SELECT job_id, kind, payload, attempts_used FROM jobs
                 WHERE state='pending' AND not_before_ms <= ?1",
            );
            if let Some(kinds) = allowed_kinds {
                sql.push_str(" AND kind IN (");
                for index in 0..kinds.len() {
                    if index > 0 {
                        sql.push(',');
                    }
                    sql.push('?');
                    sql.push_str(&(index + 2).to_string());
                }
                sql.push(')');
            }
            sql.push_str(
                " AND NOT EXISTS(
                        SELECT 1 FROM job_deps d
                        JOIN jobs dj ON dj.job_id = d.depends_on
                        WHERE d.job_id = jobs.job_id AND dj.state != 'succeeded')
                 ORDER BY priority DESC, created_ms ASC, job_id ASC
                 LIMIT 1",
            );
            let mut s = db.prepare("claim job", &sql)?;
            s.bind_u64(1, now_ms)?;
            if let Some(kinds) = allowed_kinds {
                for (index, kind) in kinds.iter().enumerate() {
                    s.bind_text((index + 2) as i32, kind)?;
                }
            }
            if !s.step()? {
                return Ok(None);
            }
            let job_id = JobId(crate::catalog::fixed16(&s.column_blob(0), "job row")?);
            let kind = s.column_text(1);
            let payload = s.column_blob(2);
            let used = u32::try_from(s.column_u64(3))
                .map_err(|_| ServerError::InvalidState { what: "job row", state: "attempts corrupt" })?;
            let attempt = used
                .checked_add(1)
                .ok_or(ServerError::InvalidState { what: "job row", state: "attempts corrupt" })?;
            drop(s);

            let mut s = db.prepare(
                "claim update job",
                "UPDATE jobs SET state='running', attempts_used=?2, updated_ms=?3 WHERE job_id=?1",
            )?;
            s.bind_blob(1, &job_id.0)?;
            s.bind_i64(2, attempt as i64)?;
            s.bind_u64(3, now_ms)?;
            s.run()?;
            drop(s);

            let mut s = db.prepare(
                "claim insert attempt",
                "INSERT INTO job_attempts(job_id, attempt, worker, started_ms)
                 VALUES(?1, ?2, ?3, ?4)",
            )?;
            s.bind_blob(1, &job_id.0)?;
            s.bind_i64(2, attempt as i64)?;
            s.bind_text(3, worker)?;
            s.bind_u64(4, now_ms)?;
            s.run()?;
            drop(s);

            let expires = lease_expiry(now_ms, lease_ms)?;
            let mut s = db.prepare(
                "claim insert lease",
                "INSERT INTO job_leases(job_id, worker, attempt, expires_ms, heartbeat_ms)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
            )?;
            s.bind_blob(1, &job_id.0)?;
            s.bind_text(2, worker)?;
            s.bind_i64(3, attempt as i64)?;
            s.bind_u64(4, expires)?;
            s.bind_u64(5, now_ms)?;
            s.run()?;

            Ok(Some(ClaimedJob {
                job_id,
                kind,
                payload,
                attempt,
                lease_expires_ms: expires,
            }))
        })
    }

    /// Extend a live lease. Refused once expired, stolen, or cancelled.
    pub fn heartbeat(&self, job_id: &JobId, worker: &str, now_ms: u64, extend_ms: u64) -> ServerResult<u64> {
        self.check_lease_ms(extend_ms)?;
        self.db.tx(|db| {
            self.require_live_lease(job_id, worker, now_ms)?;
            let expires = lease_expiry(now_ms, extend_ms)?;
            let mut s = db.prepare(
                "heartbeat lease",
                "UPDATE job_leases SET expires_ms=?2, heartbeat_ms=?3 WHERE job_id=?1",
            )?;
            s.bind_blob(1, &job_id.0)?;
            s.bind_u64(2, expires)?;
            s.bind_u64(3, now_ms)?;
            s.run()?;
            Ok(expires)
        })
    }

    /// Report success. Requires a live lease held by `worker`.
    pub fn succeed(&self, job_id: &JobId, worker: &str, now_ms: u64) -> ServerResult<()> {
        self.db.tx(|db| {
            let attempt = self.require_live_lease(job_id, worker, now_ms)?;
            self.finish(db, job_id, attempt, "succeeded", "succeeded", now_ms)
        })
    }

    /// Report failure. With attempts left the job re-queues after
    /// `retry_delay_ms` (bounded by the retry budget); otherwise it is
    /// terminally failed.
    pub fn fail(&self, job_id: &JobId, worker: &str, now_ms: u64, retry_delay_ms: u64) -> ServerResult<JobState> {
        if retry_delay_ms > self.budgets.max_retry_delay_ms {
            return Err(ServerError::OverBudget {
                what: "retry delay ms",
                limit: self.budgets.max_retry_delay_ms,
                found: retry_delay_ms,
            });
        }
        self.db.tx(|db| {
            let attempt = self.require_live_lease(job_id, worker, now_ms)?;
            let (used, max) = self.attempt_counts(job_id)?;
            if used < max {
                let not_before = now_ms
                    .checked_add(retry_delay_ms)
                    .ok_or(ServerError::InvalidInput { what: "retry time overflow" })?;
                self.close_attempt(db, job_id, attempt, "failed", now_ms)?;
                self.drop_lease(db, job_id)?;
                let mut s = db.prepare(
                    "requeue failed job",
                    "UPDATE jobs SET state='pending', not_before_ms=?2, updated_ms=?3 WHERE job_id=?1",
                )?;
                s.bind_blob(1, &job_id.0)?;
                s.bind_u64(2, not_before)?;
                s.bind_u64(3, now_ms)?;
                s.run()?;
                Ok(JobState::Pending)
            } else {
                self.finish(db, job_id, attempt, "failed", "failed", now_ms)?;
                Ok(JobState::Failed)
            }
        })
    }

    /// Hierarchical cancellation: cancel this job and every descendant in the
    /// parent tree that is not already terminal. Running descendants lose
    /// their leases immediately, so their workers' reports are refused.
    pub fn cancel(&self, root: &JobId, now_ms: u64) -> ServerResult<u64> {
        self.db.tx(|db| {
            if self.state(root)?.is_none() {
                return Err(ServerError::NotFound { what: "job" });
            }
            let mut cancelled = 0u64;
            let mut queue = vec![*root];
            while let Some(id) = queue.pop() {
                // Children first-found, then processed breadth-agnostic; the
                // whole subtree lands in one transaction either way.
                let mut s = db.prepare(
                    "cancel list children",
                    "SELECT job_id FROM jobs WHERE parent_job = ?1",
                )?;
                s.bind_blob(1, &id.0)?;
                while s.step()? {
                    queue.push(JobId(crate::catalog::fixed16(&s.column_blob(0), "job row")?));
                }
                drop(s);
                let state = self.state(&id)?.ok_or(ServerError::NotFound { what: "job" })?;
                if state.is_terminal() {
                    continue;
                }
                if state == JobState::Running {
                    if let Some((_, attempt)) = self.lease_of(&id)? {
                        self.close_attempt(db, &id, attempt, "cancelled", now_ms)?;
                    }
                    self.drop_lease(db, &id)?;
                }
                let mut s = db.prepare(
                    "cancel job",
                    "UPDATE jobs SET state='cancelled', updated_ms=?2 WHERE job_id=?1",
                )?;
                s.bind_blob(1, &id.0)?;
                s.bind_u64(2, now_ms)?;
                s.run()?;
                cancelled += 1;
            }
            Ok(cancelled)
        })
    }

    /// Restart/liveness recovery: every expired lease is torn down; its job
    /// re-queues if attempts remain, otherwise fails terminally. Expiry is
    /// inclusive: a lease whose expires_ms equals `now_ms` is already dead.
    pub fn expire_leases(&self, now_ms: u64) -> ServerResult<u64> {
        self.db.tx(|db| {
            let mut expired = Vec::new();
            let mut s = db.prepare(
                "list expired leases",
                "SELECT job_id, attempt FROM job_leases WHERE expires_ms <= ?1",
            )?;
            s.bind_u64(1, now_ms)?;
            while s.step()? {
                expired.push((
                    JobId(crate::catalog::fixed16(&s.column_blob(0), "lease row")?),
                    u32::try_from(s.column_u64(1)).map_err(|_| ServerError::InvalidState {
                        what: "lease row",
                        state: "attempt corrupt",
                    })?,
                ));
            }
            drop(s);
            for (job_id, attempt) in &expired {
                self.close_attempt(db, job_id, *attempt, "expired", now_ms)?;
                self.drop_lease(db, job_id)?;
                let (used, max) = self.attempt_counts(job_id)?;
                let sql = if used < max {
                    "UPDATE jobs SET state='pending', not_before_ms=?2, updated_ms=?2
                     WHERE job_id=?1 AND state='running'"
                } else {
                    "UPDATE jobs SET state='failed', updated_ms=?2
                     WHERE job_id=?1 AND state='running'"
                };
                let mut s = db.prepare("expire job", sql)?;
                s.bind_blob(1, &job_id.0)?;
                s.bind_u64(2, now_ms)?;
                s.run()?;
            }
            Ok(expired.len() as u64)
        })
    }

    // ---- queries -----------------------------------------------------------

    /// Put a TERMINAL job back in the queue, with a fresh payload and a
    /// fresh allowance of attempts. `Ok(false)` = the job is still pending
    /// or running, so it is already queued and nothing was touched.
    ///
    /// This exists because a job id may be DERIVED from what it is about
    /// rather than minted at random (see the annotation queue): derived ids
    /// make "enqueue twice" a free no-op, but without a way back a
    /// succeeded job becomes a tombstone that blocks the same work for
    /// ever — and the work does come back, when whatever the job produced
    /// is overwritten.
    ///
    /// Attempt history is KEPT: `attempts_used` is not reset, the ceiling
    /// is raised past it, so `job_attempts` stays a truthful record and the
    /// next claim numbers its attempt after the last one.
    pub fn requeue(
        &self,
        job_id: &JobId,
        payload: &[u8],
        extra_attempts: u32,
        now_ms: u64,
    ) -> ServerResult<bool> {
        if payload.len() as u64 > self.budgets.max_job_payload_bytes {
            return Err(ServerError::OverBudget {
                what: "job payload",
                limit: self.budgets.max_job_payload_bytes,
                found: payload.len() as u64,
            });
        }
        self.db.tx(|db| {
            let mut s = db.prepare(
                "requeue read",
                "SELECT state, attempts_used FROM jobs WHERE job_id=?1",
            )?;
            s.bind_blob(1, &job_id.0)?;
            if !s.step()? {
                return Err(ServerError::NotFound { what: "job" });
            }
            let state = JobState::parse(&s.column_text(0)).ok_or(ServerError::InvalidState {
                what: "job row",
                state: "unknown state",
            })?;
            let used = s.column_u64(1);
            drop(s);
            if !state.is_terminal() {
                return Ok(false);
            }
            let mut s = db.prepare(
                "requeue job",
                "UPDATE jobs SET state='pending', payload=?2, not_before_ms=0,
                        max_attempts=?3, updated_ms=?4
                 WHERE job_id=?1",
            )?;
            s.bind_blob(1, &job_id.0)?;
            s.bind_blob(2, payload)?;
            s.bind_u64(3, used.saturating_add(extra_attempts.max(1) as u64))?;
            s.bind_u64(4, now_ms)?;
            s.run()?;
            drop(s);
            // A terminal job holds no live lease, but a stale row would let
            // the next heartbeat land on the wrong attempt.
            let mut s = db.prepare("requeue drop lease", "DELETE FROM job_leases WHERE job_id=?1")?;
            s.bind_blob(1, &job_id.0)?;
            s.run()?;
            Ok(true)
        })
    }

    /// How many jobs of one kind sit in each state.
    ///
    /// One grouped read, so an operator bar ("312 done, 18 running, 3693
    /// queued") costs a single query instead of listing thousands of jobs
    /// a page at a time.
    pub fn count_by_state(&self, kind: &str) -> ServerResult<JobCounts> {
        let mut s = self.db.prepare(
            "job counts",
            "SELECT state, COUNT(*) FROM jobs WHERE kind = ?1 GROUP BY state",
        )?;
        s.bind_text(1, kind)?;
        let mut c = JobCounts::default();
        while s.step()? {
            let n = s.column_u64(1);
            match JobState::parse(&s.column_text(0)) {
                Some(JobState::Pending) => c.pending = n,
                Some(JobState::Running) => c.running = n,
                Some(JobState::Succeeded) => c.succeeded = n,
                Some(JobState::Failed) => c.failed = n,
                Some(JobState::Cancelled) => c.cancelled = n,
                None => {}
            }
        }
        Ok(c)
    }

    pub fn state(&self, job_id: &JobId) -> ServerResult<Option<JobState>> {
        let mut s = self
            .db
            .prepare("job state", "SELECT state FROM jobs WHERE job_id = ?1")?;
        s.bind_blob(1, &job_id.0)?;
        if !s.step()? {
            return Ok(None);
        }
        let state = s.column_text(0);
        JobState::parse(&state)
            .map(Some)
            .ok_or(ServerError::InvalidState { what: "job row", state: "unknown" })
    }

    /// The durable, bounded enqueue payload for an existing job. Host
    /// projections use this to expose selected safe metadata (currently the
    /// generation prompt) without duplicating payload storage.
    pub fn payload(&self, job_id: &JobId) -> ServerResult<Option<Vec<u8>>> {
        let mut s = self.db.prepare("job payload", "SELECT payload FROM jobs WHERE job_id = ?1")?;
        s.bind_blob(1, &job_id.0)?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some(s.column_blob(0)))
    }

    pub fn attempts(&self, job_id: &JobId) -> ServerResult<Vec<AttemptRow>> {
        let mut s = self.db.prepare(
            "job attempts",
            "SELECT attempt, started_ms, ended_ms FROM job_attempts
             WHERE job_id = ?1 ORDER BY attempt",
        )?;
        s.bind_blob(1, &job_id.0)?;
        let mut out = Vec::new();
        while s.step()? {
            out.push(AttemptRow {
                attempt: u32::try_from(s.column_u64(0)).map_err(|_| {
                    ServerError::InvalidState { what: "attempt row", state: "attempt corrupt" }
                })?,
                started_ms: s.column_u64(1),
                ended_ms: if s.column_is_null(2) {
                    None
                } else {
                    Some(s.column_u64(2))
                },
            });
        }
        Ok(out)
    }

    // ---- internals ---------------------------------------------------------

    fn check_lease_ms(&self, lease_ms: u64) -> ServerResult<()> {
        if lease_ms == 0 {
            return Err(ServerError::InvalidInput { what: "zero lease" });
        }
        if lease_ms > self.budgets.max_lease_ms {
            return Err(ServerError::OverBudget {
                what: "lease ms",
                limit: self.budgets.max_lease_ms,
                found: lease_ms,
            });
        }
        Ok(())
    }

    fn parent_of(&self, job_id: &JobId) -> ServerResult<Option<JobId>> {
        let mut s = self.db.prepare(
            "job parent",
            "SELECT parent_job FROM jobs WHERE job_id = ?1",
        )?;
        s.bind_blob(1, &job_id.0)?;
        if !s.step()? || s.column_is_null(0) {
            return Ok(None);
        }
        Ok(Some(JobId(crate::catalog::fixed16(&s.column_blob(0), "job row")?)))
    }

    fn lease_of(&self, job_id: &JobId) -> ServerResult<Option<(String, u32)>> {
        let mut s = self.db.prepare(
            "job lease",
            "SELECT worker, attempt, expires_ms FROM job_leases WHERE job_id = ?1",
        )?;
        s.bind_blob(1, &job_id.0)?;
        if !s.step()? {
            return Ok(None);
        }
        let attempt = u32::try_from(s.column_u64(1))
            .map_err(|_| ServerError::InvalidState { what: "lease row", state: "attempt corrupt" })?;
        Ok(Some((s.column_text(0), attempt)))
    }

    /// A live lease means: the job is running, the lease row names this
    /// worker, and it has not expired. Anything else refuses the report.
    /// Expiry is inclusive: at exactly expires_ms the lease is already dead,
    /// matching `expire_leases` so no instant exists where both the stale
    /// holder and the reaper act on the same lease.
    pub(crate) fn require_live_lease(&self, job_id: &JobId, worker: &str, now_ms: u64) -> ServerResult<u32> {
        let state = self
            .state(job_id)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        if state != JobState::Running {
            return Err(ServerError::LeaseLost { what: "job not running" });
        }
        let mut s = self.db.prepare(
            "check lease",
            "SELECT worker, attempt, expires_ms FROM job_leases WHERE job_id = ?1",
        )?;
        s.bind_blob(1, &job_id.0)?;
        if !s.step()? {
            return Err(ServerError::LeaseLost { what: "no lease" });
        }
        let holder = s.column_text(0);
        let attempt = u32::try_from(s.column_u64(1))
            .map_err(|_| ServerError::InvalidState { what: "lease row", state: "attempt corrupt" })?;
        let expires = s.column_u64(2);
        if holder != worker {
            return Err(ServerError::LeaseLost { what: "lease held by another worker" });
        }
        if expires <= now_ms {
            return Err(ServerError::LeaseLost { what: "lease expired" });
        }
        Ok(attempt)
    }

    fn close_attempt(&self, db: &Db, job_id: &JobId, attempt: u32, outcome: &'static str, now_ms: u64) -> ServerResult<()> {
        let mut s = db.prepare(
            "close attempt",
            "UPDATE job_attempts SET ended_ms=?3, outcome=?4
             WHERE job_id=?1 AND attempt=?2 AND ended_ms IS NULL",
        )?;
        s.bind_blob(1, &job_id.0)?;
        s.bind_i64(2, attempt as i64)?;
        s.bind_u64(3, now_ms)?;
        s.bind_text(4, outcome)?;
        s.run()
    }

    fn drop_lease(&self, db: &Db, job_id: &JobId) -> ServerResult<()> {
        let mut s = db.prepare("drop lease", "DELETE FROM job_leases WHERE job_id = ?1")?;
        s.bind_blob(1, &job_id.0)?;
        s.run()
    }

    pub(crate) fn finish(
        &self,
        db: &Db,
        job_id: &JobId,
        attempt: u32,
        outcome: &'static str,
        state: &'static str,
        now_ms: u64,
    ) -> ServerResult<()> {
        self.close_attempt(db, job_id, attempt, outcome, now_ms)?;
        self.drop_lease(db, job_id)?;
        let mut s = db.prepare(
            "finish job",
            "UPDATE jobs SET state=?2, updated_ms=?3 WHERE job_id=?1",
        )?;
        s.bind_blob(1, &job_id.0)?;
        s.bind_text(2, state)?;
        s.bind_u64(3, now_ms)?;
        s.run()
    }

    fn attempt_counts(&self, job_id: &JobId) -> ServerResult<(u32, u32)> {
        let mut s = self.db.prepare(
            "attempt counts",
            "SELECT attempts_used, max_attempts FROM jobs WHERE job_id = ?1",
        )?;
        s.bind_blob(1, &job_id.0)?;
        if !s.step()? {
            return Err(ServerError::NotFound { what: "job" });
        }
        let corrupt = ServerError::InvalidState { what: "job row", state: "attempts corrupt" };
        Ok((
            u32::try_from(s.column_u64(0)).map_err(|_| corrupt)?,
            u32::try_from(s.column_u64(1)).map_err(|_| corrupt)?,
        ))
    }
}

/// Checked lease expiry: `now_ms + lease_ms` with overflow refused. An absurd
/// clock must produce a structured refusal, never a wrapped-around expiry.
fn lease_expiry(now_ms: u64, lease_ms: u64) -> ServerResult<u64> {
    now_ms
        .checked_add(lease_ms)
        .ok_or(ServerError::InvalidInput { what: "lease expiry overflow" })
}

fn validate_kind(kind: &str) -> ServerResult<()> {
    if kind.is_empty() || kind.len() > 64 {
        return Err(ServerError::InvalidInput { what: "job kind length" });
    }
    if !kind.bytes().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.' || b == b'-'
    }) {
        return Err(ServerError::InvalidInput { what: "job kind charset" });
    }
    Ok(())
}

fn validate_worker(worker: &str) -> ServerResult<()> {
    if worker.is_empty() || worker.len() > 128 {
        return Err(ServerError::InvalidInput { what: "worker id length" });
    }
    if !worker.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ServerError::InvalidInput { what: "worker id charset" });
    }
    Ok(())
}
