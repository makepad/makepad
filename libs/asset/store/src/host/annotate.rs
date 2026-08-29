//! The vision-annotation queue: what an asset owes the catalog, and the
//! job that pays it.
//!
//! `search_annotations.description` is the only free text the search index
//! weighs, and an import writes none: a Kenney kit lands as 329 rows called
//! `wall-corner-window` with a kit label and nothing an AI level builder
//! can retrieve by asking for "a corner wall with a window". The pass that
//! writes those descriptions runs a turntable thumbnail sheet through a
//! vision model (libs/asset/annotate), and it is a JOB, not a script an
//! operator remembers to run — so the SERVER queues one the moment any
//! publisher (an import, a generation, a game agent) makes such an asset
//! live, and a worker drains the queue.
//!
//! This module owns three things and no policy beyond them:
//!
//! * which kinds are annotatable ([`crate::search::ANNOTATABLE_KINDS`]),
//! * the job id, which is DERIVED from the asset and the annotator version
//!   so enqueueing twice is a no-op with no dedupe table to keep,
//! * the job body a worker claims.

use super::json::{obj, s, Value};
use super::state::{envelope_build, StateCtx};
use crate::auth::PrincipalId;
use crate::error::ServerResult;
use crate::jobs::{JobId, NewJob};
use crate::search::BacklogRow;
use makepad_asset_data::{sha256, AssetId};

/// Job kind workers filter their claim on. Not a generation kind: the
/// generation coordinator's claim loop names its own kinds and never sees
/// these.
pub const JOB_KIND: &str = "annotate.asset";

/// The annotator version this server enqueues for.
///
/// SOURCE OF TRUTH is `makepad_asset_annotate::ANNOTATOR_VERSION`; the
/// store cannot depend on the annotate crate (it sits above the store), so
/// the number is mirrored here and the drift is caught by a test in the app
/// that links both. Bumping one without the other means the server queues
/// work the pass thinks is already done, or never queues work it owes.
pub const ANNOTATOR_VERSION: u32 = 7;

/// The tag an annotated asset carries, and the one thing "already done"
/// means anywhere in this system.
pub fn version_tag() -> String {
    format!("vlm-v{ANNOTATOR_VERSION}")
}

/// Attempts per job before the failure is terminal. A vision box that
/// blinks should not cost an asset its description; a box that is gone
/// should stop being retried and become visible as a failed job.
pub const MAX_ATTEMPTS: u32 = 3;

/// The job's identity, derived rather than random.
///
/// Two publishers racing the same asset, a re-publish, a backlog sweep over
/// an asset a publish already queued: all mint the same id, and the second
/// `enqueue` is refused by the primary key. That is the whole dedupe — no
/// pending-jobs table, no scan, and nothing to leak when a job is dropped.
/// `epoch` is normally 0; an operator re-driving assets whose jobs failed
/// terminally passes a fresh one to mint a distinct id.
pub fn job_id_for(asset: &AssetId, tag: &str, epoch: u64) -> JobId {
    let mut seed = Vec::with_capacity(64);
    seed.extend_from_slice(b"annotate.asset\0");
    seed.extend_from_slice(tag.as_bytes());
    seed.push(0);
    seed.extend_from_slice(asset.as_bytes());
    seed.extend_from_slice(&epoch.to_le_bytes());
    let digest = sha256(&seed);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    JobId(id)
}

/// What a worker claims: enough to fetch the sheet and publish the result
/// without a second lookup, and nothing that can go stale except the alias
/// (which the worker re-resolves through the store anyway).
pub fn job_body(row: &BacklogRow, tag: &str) -> Value {
    obj(vec![
        ("asset", s(row.asset_id.to_string())),
        ("alias", s(row.alias.clone())),
        ("kind", match row.kind {
            Some(k) => s(crate::search::kind_name(k)),
            None => Value::Null,
        }),
        ("version_tag", s(tag.to_string())),
    ])
}

/// Queue one asset's annotation. `Ok(false)` = already queued or already
/// run at this version, which is the common case and not a failure.
///
/// The caller supplies the principal the job is enqueued as: for a publish
/// that is the publisher, so the job lands in a namespace they already hold
/// capabilities on and the existing worker claim gate needs no new grant.
pub fn enqueue(
    ctx: &StateCtx,
    by: &PrincipalId,
    row: &BacklogRow,
    epoch: u64,
    now: u64,
) -> ServerResult<bool> {
    let tag = version_tag();
    let job_id = job_id_for(&row.asset_id, &tag, epoch);
    let payload = envelope_build(&row.namespace, by, &job_body(row, &tag));
    match ctx.core.jobs().state(&job_id)? {
        // Already queued or being worked on: the derived id did its job.
        Some(state) if !state.is_terminal() => return Ok(false),
        // The job ran, and the asset owes a description AGAIN — a re-import
        // replaced the annotation the pass had written. Without this the
        // derived id would be a tombstone and a re-imported library could
        // never be described a second time.
        Some(_) => return ctx.core.jobs().requeue(&job_id, &payload, MAX_ATTEMPTS, now),
        None => {}
    }
    // Routing metadata first, exactly as the enqueue route does: the claim
    // gate reads namespaces from it, so a crash between the two writes
    // leaves a gated orphan rather than an ungated job.
    ctx.meta_insert(&job_id, &row.namespace, JOB_KIND, by, now)?;
    let job = NewJob {
        job_id,
        parent: None,
        kind: JOB_KIND,
        payload: &payload,
        priority: 0,
        max_attempts: MAX_ATTEMPTS,
        not_before_ms: 0,
        deps: &[],
    };
    if let Err(e) = ctx.core.jobs().enqueue(&job, now) {
        let _ = ctx.meta_delete(&job_id);
        return Err(e);
    }
    Ok(true)
}

/// Queue a just-published asset, best effort.
///
/// A publish must never fail because the annotation queue did — the asset
/// is in the catalog either way, and a backlog sweep will find it. The
/// boolean says whether a job was minted, for the caller's log.
///
/// Everything the decision needs (live, alias, kind, tag) is read from the
/// annotation the publish just wrote, so the two publish routes share one
/// answer and neither can drift from the backlog sweep's.
pub fn enqueue_published(ctx: &StateCtx, by: &PrincipalId, asset_id: &AssetId, now: u64) -> bool {
    let tag = version_tag();
    let row = match ctx.core.search().backlog_row_for(asset_id, &tag) {
        Ok(Some(row)) => row,
        _ => return false,
    };
    matches!(enqueue(ctx, by, &row, 0, now), Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_id_is_the_dedupe() {
        let a = AssetId::from_bytes([3; 16]);
        let b = AssetId::from_bytes([4; 16]);
        // Same asset, same version, same epoch -> the same job, so a
        // publish and a backlog sweep cannot both queue it.
        assert_eq!(job_id_for(&a, "vlm-v7", 0), job_id_for(&a, "vlm-v7", 0));
        // Anything that means "this is different work" mints a new id.
        assert_ne!(job_id_for(&a, "vlm-v7", 0), job_id_for(&b, "vlm-v7", 0));
        assert_ne!(job_id_for(&a, "vlm-v7", 0), job_id_for(&a, "vlm-v8", 0));
        assert_ne!(job_id_for(&a, "vlm-v7", 0), job_id_for(&a, "vlm-v7", 1));
    }

    #[test]
    fn the_body_carries_what_a_worker_needs() {
        let row = BacklogRow {
            asset_id: AssetId::from_bytes([9; 16]),
            namespace: "kenney".into(),
            alias: "kenney/nature-kit/tree".into(),
            kind: Some(makepad_asset_data::AssetKind::Mesh),
        };
        let body = job_body(&row, "vlm-v7");
        assert_eq!(body.get("alias").and_then(Value::as_str), Some("kenney/nature-kit/tree"));
        assert_eq!(body.get("kind").and_then(Value::as_str), Some("mesh"));
        assert_eq!(body.get("version_tag").and_then(Value::as_str), Some("vlm-v7"));
        assert_eq!(
            body.get("asset").and_then(Value::as_str),
            Some(row.asset_id.to_string().as_str())
        );
    }
}
