//! Typed asset operations: registry fail-closure, truthful availability,
//! exact input pinning, idempotency join/conflict, the atomic finalizer
//! (lineage, inherited rights, model facts, alias CAS), owner scoping,
//! cancellation, retry rounds, and malicious-worker refusals.

mod common;
use common::*;
use makepad_asset_store::operations::{OperationOutputFacts, ParamValue};
use makepad_asset_store::{
    AliasExpect, ArmedJob, AssetServerCore, Budgets, CandidateState, NewJob,
    OperationCreateOutcome, OperationCreateRequest, OperationId, OperationInputBinding,
    OperationPublication, OperationResultFacts, OperationState, ServerError,
    MESH_FROM_IMAGE_V1,
};
use makepad_asset_data::{
    AssetAlias, AssetFile, AssetId, AssetKind, AssetManifest, AssetRevisionId, DerivativePolicy,
    DeviceTier, FileRole, ImageDims, MediaType, Metrics, ThumbnailMedia, ThumbnailMeta,
};
use std::str::FromStr;

const EXEC_KIND: &str = "op.mesh.from_image.v1";
const WORKER: &str = "prin_w/w1";

fn opid(n: u8) -> OperationId {
    OperationId([n; 16])
}

/// Publish one PNG texture asset the operation can consume; returns its ids.
fn publish_texture(
    core: &AssetServerCore,
    ns: &str,
    id_byte: u8,
    png: &[u8],
) -> (AssetId, AssetRevisionId) {
    let id = asset_id_n(id_byte);
    core.put_blob(png, NOW).unwrap();
    let manifest = texture_manifest(id, png);
    let bytes = manifest.to_canonical_bytes().unwrap();
    let revision = manifest.revision().unwrap();
    core.catalog().register_asset(&id, ns, NOW).unwrap();
    core.catalog().stage_asset_revision(&bytes, NOW).unwrap();
    core.catalog().publish_asset(&id, &revision, NOW).unwrap();
    (id, revision)
}

fn texture_manifest(asset_id: AssetId, png: &[u8]) -> AssetManifest {
    let mut manifest = prop_manifest(asset_id, b"unused", b"unused");
    manifest.kind = AssetKind::Texture;
    manifest.files = vec![AssetFile {
        role: FileRole::Texture,
        tier: DeviceTier::Any,
        lod: 0,
        media: MediaType::Png,
        blob: makepad_asset_data::BlobId::hash_of(png),
        byte_len: png.len() as u64,
        dims: Some(ImageDims { width: 64, height: 64 }),
    }];
    manifest.thumbnail = None;
    manifest.metrics = Metrics {
        total_bytes: png.len() as u64,
        triangles: 0,
        vertices: 0,
        joints: 0,
        clips: 0,
        max_texture_dim: 64,
        media_millis: 0,
    };
    manifest
}

fn binding(asset: AssetId, revision: AssetRevisionId) -> OperationInputBinding {
    OperationInputBinding {
        slot: "image".into(),
        asset_id: asset,
        revision,
        role: FileRole::Texture,
        tier: None,
        lod: None,
        expected_media: None,
    }
}

fn create_req<'a>(
    op: OperationId,
    owner_byte: u8,
    key: &'a str,
    inputs: &'a [OperationInputBinding],
) -> OperationCreateRequest<'a> {
    OperationCreateRequest {
        operation_id: op,
        owner: pid_n(owner_byte),
        namespace: "gen",
        kind: MESH_FROM_IMAGE_V1,
        idempotency_key: key,
        inputs,
        params: &[],
        publication: OperationPublication::Publish,
    }
}

/// Mark the executor kind live so creation passes the availability gate.
fn worker_live(core: &AssetServerCore, now: u64) {
    core.operations().note_worker_kinds(&[EXEC_KIND], now).unwrap();
}

/// Enqueue an armed operation job the way the transport does.
fn enqueue_armed(core: &AssetServerCore, job: &ArmedJob, now: u64) {
    core.jobs()
        .enqueue(
            &NewJob {
                job_id: job.job_id,
                parent: None,
                kind: job.kind,
                payload: b"{}",
                priority: 0,
                max_attempts: 1,
                not_before_ms: 0,
                deps: &[],
            },
            now,
        )
        .unwrap();
}

fn claim_op_job(core: &AssetServerCore, now: u64) -> makepad_asset_store::ClaimedJob {
    core.jobs()
        .claim_allowed(WORKER, now, 60_000, &[EXEC_KIND])
        .unwrap()
        .expect("operation job claimable")
}

/// Upload deterministic mesh output blobs and build truthful facts.
fn good_facts(core: &AssetServerCore, tag: u8) -> OperationResultFacts {
    let glb = vec![tag; 900];
    let thumb = vec![tag ^ 0xFF; 1_400];
    core.put_blob(&glb, NOW).unwrap();
    core.put_blob(&thumb, NOW).unwrap();
    OperationResultFacts {
        outputs: vec![OperationOutputFacts {
            name: "mesh".into(),
            files: vec![AssetFile {
                role: FileRole::RenderGlb,
                tier: DeviceTier::Any,
                lod: 0,
                media: MediaType::Glb,
                blob: makepad_asset_data::BlobId::hash_of(&glb),
                byte_len: glb.len() as u64,
                dims: None,
            }],
            thumbnail: Some(ThumbnailMeta {
                blob: makepad_asset_data::BlobId::hash_of(&thumb),
                media: ThumbnailMedia::Png,
                width: 512,
                height: 512,
                byte_len: thumb.len() as u64,
            }),
            metrics: Metrics {
                total_bytes: glb.len() as u64 + thumb.len() as u64,
                triangles: 240,
                vertices: 128,
                joints: 0,
                clips: 0,
                max_texture_dim: 512,
                media_millis: 0,
            },
            bounds: None,
        }],
        generator: "trellis".into(),
        model: "trellis-image-large".into(),
        version: "1".into(),
        seed: 0,
    }
}

fn created(outcome: OperationCreateOutcome) -> (OperationId, ArmedJob) {
    match outcome {
        OperationCreateOutcome::Created { snapshot, job } => (snapshot.id, job),
        other => panic!("expected Created, got {other:?}"),
    }
}

// ---------------------------------------------------------------- registry

#[test]
fn unknown_kinds_and_versions_fail_closed() {
    let (_root, core) = open_core("op_unknown_kind");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-1");
    let inputs = [binding(asset, rev)];
    for kind in ["mesh.from_image.v2", "mesh.from_image", "nonsense", ""] {
        let mut req = create_req(opid(1), 9, "k1", &inputs);
        req.kind = kind;
        assert!(
            matches!(
                core.operations().create(&req, NOW).unwrap_err(),
                ServerError::NotFound { what: "operation kind" }
            ),
            "kind {kind:?} must fail closed"
        );
    }
}

#[test]
fn availability_is_truthful_and_gates_creation() {
    let (_root, core) = open_core("op_availability");
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-1");
    let inputs = [binding(asset, rev)];

    // No worker has ever offered the executor kind: unavailable, and
    // creation refuses with a structured state.
    let caps = core.operations().capabilities(NOW).unwrap();
    let cap = caps.iter().find(|c| c.def.kind == MESH_FROM_IMAGE_V1).unwrap();
    assert!(!cap.available);
    assert!(cap.reason.is_some());
    assert!(matches!(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap_err(),
        ServerError::InvalidState { what: "operation kind", state: "unavailable" }
    ));

    // A live worker flips it; a stale sighting past the window flips it back.
    worker_live(&core, NOW);
    assert!(core.operations().capabilities(NOW).unwrap()[0].available);
    let later = NOW + Budgets::default_v1().operation_worker_liveness_ms + 1;
    assert!(!core.operations().capabilities(later).unwrap()[0].available);

    // Unregistered kinds never enter the liveness table.
    core.operations().note_worker_kinds(&["video.generate"], NOW).unwrap();
}

// ---------------------------------------------------------------- creation

#[test]
fn create_pins_exact_input_and_replay_joins() {
    let (_root, core) = open_core("op_create_idem");
    worker_live(&core, NOW);
    let png = b"png-pin";
    let (asset, rev) = publish_texture(&core, "gen", 1, png);
    let inputs = [binding(asset, rev)];

    let (op, job) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    assert_eq!(job.kind, EXEC_KIND);
    assert_eq!(job.round, 0);
    let snap = core.operations().get(&pid_n(9), &op, NOW).unwrap();
    assert_eq!(snap.inputs.len(), 1);
    assert_eq!(snap.inputs[0].blob, makepad_asset_data::BlobId::hash_of(png));
    assert_eq!(snap.inputs[0].byte_len, png.len() as u64);
    assert_eq!(snap.inputs[0].media, MediaType::Png);
    assert_eq!(snap.display_state, "queued");
    // Resolved params carry the complete defaulted set.
    assert!(snap.params.iter().any(|(n, v)| n == "seed" && *v == ParamValue::Int(0)));

    // Exact replay (even with the defaults spelled out) JOINS: same
    // operation, and the armed-but-unenqueued job is re-offered.
    let explicit = [("seed".to_string(), ParamValue::Int(0))];
    let mut replay = create_req(opid(2), 9, "k1", &inputs);
    replay.params = &explicit;
    match core.operations().create(&replay, NOW + 5).unwrap() {
        OperationCreateOutcome::Joined { snapshot, rearm } => {
            assert_eq!(snapshot.id, op);
            let rearm = rearm.expect("job vanished before enqueue: must re-offer");
            assert_eq!(rearm.job_id, job.job_id);
        }
        other => panic!("expected Joined, got {other:?}"),
    }
    // Once the job is enqueued, the replay joins without a re-offer.
    enqueue_armed(&core, &job, NOW + 6);
    match core.operations().create(&replay, NOW + 7).unwrap() {
        OperationCreateOutcome::Joined { rearm, .. } => assert!(rearm.is_none()),
        other => panic!("expected Joined, got {other:?}"),
    }

    // Same key, different spec (another seed): conflict.
    let other_params = [("seed".to_string(), ParamValue::Int(7))];
    let mut conflicting = create_req(opid(3), 9, "k1", &inputs);
    conflicting.params = &other_params;
    assert!(matches!(
        core.operations().create(&conflicting, NOW + 8).unwrap_err(),
        ServerError::Conflict { what: "operation idempotency key" }
    ));

    // Different key, same spec: a NEW operation.
    let (op2, _) = created(
        core.operations()
            .create(&create_req(opid(4), 9, "k2", &inputs), NOW + 9)
            .unwrap(),
    );
    assert_ne!(op2, op);
}

#[test]
fn input_validation_fails_closed() {
    let (_root, core) = open_core("op_input_validation");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-ok");
    let ops = core.operations();

    let refuse = |inputs: &[OperationInputBinding], key: &str| {
        ops.create(&create_req(opid(50), 9, key, inputs), NOW).unwrap_err()
    };

    // Unknown revision.
    let ghost = AssetRevisionId::from_bytes([0x5A; 32]);
    assert!(matches!(
        refuse(&[binding(asset, ghost)], "r1"),
        ServerError::NotFound { what: "operation input revision" }
    ));
    // Asset/revision mismatch.
    assert!(matches!(
        refuse(&[binding(asset_id_n(7), rev)], "r2"),
        ServerError::Conflict { what: "operation input asset" }
    ));
    // Wrong role for the slot.
    let mut wrong_role = binding(asset, rev);
    wrong_role.role = FileRole::Video;
    assert!(matches!(
        refuse(&[wrong_role], "r3"),
        ServerError::InvalidInput { what: "operation input role" }
    ));
    // Role accepted by the slot but absent from the manifest.
    let mut missing_role = binding(asset, rev);
    missing_role.role = FileRole::Albedo;
    assert!(matches!(
        refuse(&[missing_role], "r4"),
        ServerError::NotFound { what: "operation input file" }
    ));
    // Expected-media guard.
    let mut wrong_media = binding(asset, rev);
    wrong_media.expected_media = Some(MediaType::Jpeg);
    assert!(matches!(
        refuse(&[wrong_media], "r5"),
        ServerError::Conflict { what: "operation input media" }
    ));
    // Unknown slot.
    let mut wrong_slot = binding(asset, rev);
    wrong_slot.slot = "mask".into();
    assert!(matches!(
        refuse(&[wrong_slot], "r6"),
        ServerError::InvalidInput { what: "operation input slot" }
    ));
    // Slot count (two bindings into a 1..=1 slot).
    assert!(matches!(
        refuse(&[binding(asset, rev), binding(asset, rev)], "r7"),
        ServerError::InvalidInput { what: "operation input count" }
    ));
    // Wrong asset kind: publish a prop and offer it as the image.
    let (prop, prop_rev) = publish_prop(&core, "gen", 2, b"glb", b"thumbthumb", NOW);
    let mut prop_binding = binding(prop, prop_rev);
    prop_binding.role = FileRole::Texture;
    assert!(matches!(
        refuse(&[prop_binding], "r8"),
        ServerError::InvalidInput { what: "operation input kind" }
    ));
    // Staged-but-unpublished input.
    let staged_manifest = texture_manifest(asset_id_n(3), b"png-staged");
    core.put_blob(b"png-staged", NOW).unwrap();
    core.catalog().register_asset(&asset_id_n(3), "gen", NOW).unwrap();
    let staged_bytes = staged_manifest.to_canonical_bytes().unwrap();
    let staged_rev = core.catalog().stage_asset_revision(&staged_bytes, NOW).unwrap();
    assert!(matches!(
        refuse(&[binding(asset_id_n(3), staged_rev)], "r9"),
        ServerError::InvalidState { what: "operation input", state: "not published" }
    ));
    // Derivatives-forbidden rights refuse BEFORE any job exists.
    let mut locked = texture_manifest(asset_id_n(4), b"png-locked");
    locked.rights.derivatives = DerivativePolicy::Forbidden;
    core.put_blob(b"png-locked", NOW).unwrap();
    core.catalog().register_asset(&asset_id_n(4), "gen", NOW).unwrap();
    let locked_bytes = locked.to_canonical_bytes().unwrap();
    let locked_rev = core.catalog().stage_asset_revision(&locked_bytes, NOW).unwrap();
    core.catalog().publish_asset(&asset_id_n(4), &locked_rev, NOW).unwrap();
    assert!(matches!(
        refuse(&[binding(asset_id_n(4), locked_rev)], "r10"),
        ServerError::InvalidState { what: "operation input rights", state: "derivatives forbidden" }
    ));

    // Unknown / out-of-range / duplicate parameters.
    let inputs = [binding(asset, rev)];
    let unknown = [("steps".to_string(), ParamValue::Int(4))];
    let mut req = create_req(opid(51), 9, "p1", &inputs);
    req.params = &unknown;
    assert!(matches!(
        ops.create(&req, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "operation parameter unknown" }
    ));
    let out_of_range = [("seed".to_string(), ParamValue::Int(-1))];
    let mut req = create_req(opid(52), 9, "p2", &inputs);
    req.params = &out_of_range;
    assert!(matches!(
        ops.create(&req, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "operation parameter range" }
    ));
    let wrong_type = [("seed".to_string(), ParamValue::Text("zero".into()))];
    let mut req = create_req(opid(53), 9, "p3", &inputs);
    req.params = &wrong_type;
    assert!(matches!(
        ops.create(&req, NOW).unwrap_err(),
        ServerError::InvalidInput { what: "operation parameter type" }
    ));

    // Nothing above armed a job.
    assert!(core.jobs().claim(WORKER, NOW, 1000).unwrap().is_none());
}

// ---------------------------------------------------------------- finalize

#[test]
fn finalize_publishes_atomically_with_lineage_and_rights() {
    let (_root, core) = open_core("op_finalize_happy");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];
    let (op, job) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job, NOW + 1);
    let claimed = claim_op_job(&core, NOW + 2);
    assert_eq!(claimed.job_id, job.job_id);

    let facts = good_facts(&core, 0x11);
    let (out_asset, out_rev) = core
        .operations()
        .finalize(&op, &job.job_id, WORKER, &facts, NOW + 3)
        .unwrap();

    // The revision is published, with exact parent lineage, the actual model
    // facts, the server-computed spec digest, and the parent's rights.
    assert_eq!(
        core.catalog().asset_candidate_state(&out_asset, &out_rev).unwrap(),
        Some(CandidateState::Published)
    );
    let manifest_bytes = core.catalog().asset_revision_manifest(&out_rev).unwrap().unwrap();
    let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes).unwrap();
    assert_eq!(manifest.kind, AssetKind::Mesh);
    let prov = manifest.provenance.as_ref().expect("provenance");
    assert_eq!(prov.parents, vec![rev]);
    assert_eq!(prov.generator, "trellis");
    assert_eq!(prov.model, "trellis-image-large");
    assert_eq!(prov.seed, 0);
    let parent_bytes = core.catalog().asset_revision_manifest(&rev).unwrap().unwrap();
    let parent = AssetManifest::from_canonical_bytes(&parent_bytes).unwrap();
    assert_eq!(manifest.rights, parent.rights, "rights inherit verbatim");
    let snap = core.operations().get(&pid_n(9), &op, NOW + 4).unwrap();
    assert_eq!(prov.params_digest, Some(snap.spec_digest));

    // Operation, job, and events all agree, atomically.
    assert_eq!(snap.state, OperationState::Succeeded);
    assert_eq!(snap.result, Some((out_asset, out_rev)));
    assert_eq!(
        core.jobs().state(&job.job_id).unwrap(),
        Some(makepad_asset_store::JobState::Succeeded)
    );
    let events = core.operations().events(&pid_n(9), &op, 0, 16, NOW + 4).unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["created", "succeeded"]);

    // Identical replay is idempotent; a divergent late duplicate refuses.
    let (again_asset, again_rev) = core
        .operations()
        .finalize(&op, &job.job_id, WORKER, &facts, NOW + 5)
        .unwrap();
    assert_eq!((again_asset, again_rev), (out_asset, out_rev));
    let divergent = good_facts(&core, 0x22);
    assert!(matches!(
        core.operations()
            .finalize(&op, &job.job_id, WORKER, &divergent, NOW + 6)
            .unwrap_err(),
        ServerError::Conflict { what: "late duplicate operation result" }
    ));
}

#[test]
fn malicious_worker_facts_are_refused() {
    let (_root, core) = open_core("op_malicious_worker");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];
    let (op, job) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job, NOW + 1);
    let _claimed = claim_op_job(&core, NOW + 2);
    let ops = core.operations();

    // Unclaimed blob (never uploaded).
    let mut facts = good_facts(&core, 0x31);
    facts.outputs[0].files[0].blob = makepad_asset_data::BlobId::hash_of(b"never uploaded");
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::NotFound { what: "operation output blob" }
    ));
    // Lying byte length.
    let mut facts = good_facts(&core, 0x32);
    facts.outputs[0].files[0].byte_len += 1;
    facts.outputs[0].metrics.total_bytes += 1;
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::SizeMismatch { what: "operation output blob size", .. }
    ));
    // Disallowed output role.
    let mut facts = good_facts(&core, 0x33);
    facts.outputs[0].files[0].role = FileRole::Video;
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::InvalidInput { what: "operation output role" }
    ));
    // Missing required render mesh.
    let mut facts = good_facts(&core, 0x34);
    facts.outputs[0].files[0].role = FileRole::Collider;
    facts.outputs[0].files[0].media = MediaType::Bin;
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::InvalidInput { what: "operation output role missing" }
    ));
    // Metrics that do not account for the reported bytes.
    let mut facts = good_facts(&core, 0x35);
    facts.outputs[0].metrics.total_bytes += 999;
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::Conflict { what: "operation output metrics" }
    ));
    // Seed not the pinned one.
    let mut facts = good_facts(&core, 0x36);
    facts.seed = 42;
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::Conflict { what: "operation model seed" }
    ));
    // Empty model facts.
    let mut facts = good_facts(&core, 0x37);
    facts.model = String::new();
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::InvalidInput { what: "operation model facts" }
    ));
    // Wrong output name / cardinality.
    let mut facts = good_facts(&core, 0x38);
    facts.outputs[0].name = "meshes".into();
    assert!(matches!(
        ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 3).unwrap_err(),
        ServerError::InvalidInput { what: "operation output name" }
    ));

    // After all refusals: nothing published, operation still live.
    let snap = ops.get(&pid_n(9), &op, NOW + 4).unwrap();
    assert_eq!(snap.state, OperationState::Queued);
    assert_eq!(snap.display_state, "running");
    assert!(snap.result.is_none());

    // And the honest facts still land afterwards.
    let facts = good_facts(&core, 0x39);
    ops.finalize(&op, &job.job_id, WORKER, &facts, NOW + 5).unwrap();
}

#[test]
fn stale_lease_wrong_worker_and_cancelled_cannot_finalize() {
    let (_root, core) = open_core("op_lease_gates");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];

    // (1) Never claimed: no lease, refuse.
    let (op1, job1) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job1, NOW);
    let facts = good_facts(&core, 0x41);
    assert!(matches!(
        core.operations()
            .finalize(&op1, &job1.job_id, WORKER, &facts, NOW + 1)
            .unwrap_err(),
        ServerError::LeaseLost { .. }
    ));

    // (2) Claimed by another worker: refuse.
    let claimed = claim_op_job(&core, NOW + 2);
    assert_eq!(claimed.job_id, job1.job_id);
    assert!(matches!(
        core.operations()
            .finalize(&op1, &job1.job_id, "prin_other/w9", &facts, NOW + 3)
            .unwrap_err(),
        ServerError::LeaseLost { .. }
    ));

    // (3) Lease expired: refuse.
    let expired_at = NOW + 2 + 60_000;
    assert!(matches!(
        core.operations()
            .finalize(&op1, &job1.job_id, WORKER, &facts, expired_at)
            .unwrap_err(),
        ServerError::LeaseLost { .. }
    ));

    // (4) Cancelled operation: the worker's finalize refuses even while its
    // lease would otherwise be live.
    let (op2, job2) = created(
        core.operations()
            .create(&create_req(opid(2), 9, "k2", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job2, NOW + 4);
    let claimed = claim_op_job(&core, NOW + 5);
    assert_eq!(claimed.job_id, job2.job_id);
    assert!(core.operations().cancel(&pid_n(9), &op2, NOW + 6).unwrap());
    assert!(matches!(
        core.operations()
            .finalize(&op2, &job2.job_id, WORKER, &facts, NOW + 7)
            .unwrap_err(),
        ServerError::InvalidState { what: "operation finalize", state: "cancelled" }
    ));
    // Nothing of op2 was published; its snapshot is terminal-cancelled.
    let snap = core.operations().get(&pid_n(9), &op2, NOW + 8).unwrap();
    assert_eq!(snap.state, OperationState::Cancelled);
    assert!(snap.result.is_none());
}

#[test]
fn alias_cas_and_atomic_fault_expose_nothing() {
    let (_root, core) = open_core("op_alias_cas");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let alias = AssetAlias::from_str("gen/hero-mesh").unwrap();

    // Operation A: publish_and_alias expecting the alias to be absent.
    let inputs = [binding(asset, rev)];
    let mut req = create_req(opid(1), 9, "ka", &inputs);
    req.publication = OperationPublication::PublishAndAlias {
        alias: alias.clone(),
        expect: AliasExpect::Absent,
    };
    let (op_a, job_a) = created(core.operations().create(&req, NOW).unwrap());
    enqueue_armed(&core, &job_a, NOW);
    let _ = claim_op_job(&core, NOW + 1);
    let facts_a = good_facts(&core, 0x51);
    let (asset_a, rev_a) = core
        .operations()
        .finalize(&op_a, &job_a.job_id, WORKER, &facts_a, NOW + 2)
        .unwrap();
    let head = core.catalog().resolve_asset_alias(&alias).unwrap().expect("alias set");
    assert_eq!(head.revision, rev_a);

    // Operation B: also expects Absent — the CAS fails, and the WHOLE
    // finalize rolls back: no published revision, no succeeded operation,
    // alias untouched, lease still live.
    let mut req = create_req(opid(2), 9, "kb", &inputs);
    req.publication = OperationPublication::PublishAndAlias {
        alias: alias.clone(),
        expect: AliasExpect::Absent,
    };
    let (op_b, job_b) = created(core.operations().create(&req, NOW + 3).unwrap());
    enqueue_armed(&core, &job_b, NOW + 3);
    let _ = claim_op_job(&core, NOW + 4);
    let facts_b = good_facts(&core, 0x52);
    assert!(matches!(
        core.operations()
            .finalize(&op_b, &job_b.job_id, WORKER, &facts_b, NOW + 5)
            .unwrap_err(),
        ServerError::Conflict { what: "operation alias occupied" }
    ));
    let snap_b = core.operations().get(&pid_n(9), &op_b, NOW + 6).unwrap();
    assert_eq!(snap_b.state, OperationState::Queued);
    assert!(snap_b.result.is_none());
    // The would-be output revision must not exist in any candidate state.
    let ghost_facts_rev = {
        // Rebuild what B would have published: its output asset id is
        // deterministic, so absence of ANY candidate rows for it proves the
        // rollback (register_asset + stage happened inside the failed tx).
        core.catalog().asset_namespace(&asset_of(&core, &op_b)).unwrap()
    };
    assert!(ghost_facts_rev.is_none(), "rolled-back output asset must not exist");
    assert_eq!(
        core.catalog().resolve_asset_alias(&alias).unwrap().unwrap().revision,
        rev_a,
        "alias must not move on a failed CAS"
    );

    // Operation C: expects Head(rev_a) — succeeds and retargets.
    let mut req = create_req(opid(3), 9, "kc", &inputs);
    req.publication = OperationPublication::PublishAndAlias {
        alias: alias.clone(),
        expect: AliasExpect::Head(rev_a),
    };
    let (op_c, job_c) = created(core.operations().create(&req, NOW + 7).unwrap());
    enqueue_armed(&core, &job_c, NOW + 7);
    let _ = claim_op_job(&core, NOW + 8);
    let facts_c = good_facts(&core, 0x53);
    let (_, rev_c) = core
        .operations()
        .finalize(&op_c, &job_c.job_id, WORKER, &facts_c, NOW + 9)
        .unwrap();
    assert_eq!(
        core.catalog().resolve_asset_alias(&alias).unwrap().unwrap().revision,
        rev_c
    );
    let _ = asset_a;
    let _ = op_a;
}

/// The deterministic output asset id of an operation (test-side mirror).
fn asset_of(core: &AssetServerCore, op: &OperationId) -> AssetId {
    // Derive through the snapshot after success, or recompute: the core does
    // not expose the derivation, so probe via the recorded result when
    // present. For rolled-back operations we recompute the id the same way
    // the server does.
    if let Ok(snap) = core.operations().get(&pid_n(9), op, NOW + 6) {
        if let Some((asset, _)) = snap.result {
            return asset;
        }
    }
    let mut h = makepad_asset_data::sha256::Sha256::new();
    h.update(b"mp-operation-asset:v1\0");
    h.update(&op.0);
    h.update(b"mesh");
    let digest = h.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    AssetId::from_bytes(id)
}

#[test]
fn injected_db_fault_rolls_back_every_side_effect() {
    let (root, core) = open_core("op_injected_fault");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];
    let (op, job) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job, NOW);
    let _ = claim_op_job(&core, NOW + 1);

    // Inject a fault at the LAST step of the finalize transaction: the
    // operations row flip to succeeded. Everything before it (asset
    // registration, staging, publication, alias) must roll back with it.
    let db = root.join("catalog.sqlite3");
    raw::exec(
        &db,
        "CREATE TRIGGER op_fault BEFORE UPDATE ON operations
         WHEN NEW.state='succeeded' BEGIN SELECT RAISE(ABORT, 'injected'); END",
    );
    let facts = good_facts(&core, 0x61);
    assert!(matches!(
        core.operations()
            .finalize(&op, &job.job_id, WORKER, &facts, NOW + 2)
            .unwrap_err(),
        ServerError::Db { .. }
    ));
    // Nothing surfaced: no output asset, operation still queued/running,
    // job still running under its lease.
    assert!(core.catalog().asset_namespace(&asset_of(&core, &op)).unwrap().is_none());
    let snap = core.operations().get(&pid_n(9), &op, NOW + 3).unwrap();
    assert_eq!(snap.state, OperationState::Queued);
    assert!(snap.result.is_none());
    assert_eq!(
        core.jobs().state(&job.job_id).unwrap(),
        Some(makepad_asset_store::JobState::Running)
    );

    // Drop the fault: the SAME worker report now lands completely.
    raw::exec(&db, "DROP TRIGGER op_fault");
    let (out_asset, out_rev) = core
        .operations()
        .finalize(&op, &job.job_id, WORKER, &facts, NOW + 4)
        .unwrap();
    assert_eq!(
        core.catalog().asset_candidate_state(&out_asset, &out_rev).unwrap(),
        Some(CandidateState::Published)
    );
}

// ------------------------------------------------------- cancel/retry/owner

#[test]
fn cancel_is_owner_scoped_and_idempotent() {
    let (_root, core) = open_core("op_cancel");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];
    let (op, job) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job, NOW);

    // Cross-owner: hidden, not refused.
    assert!(matches!(
        core.operations().cancel(&pid_n(8), &op, NOW + 1).unwrap_err(),
        ServerError::NotFound { what: "operation" }
    ));
    assert!(matches!(
        core.operations().get(&pid_n(8), &op, NOW + 1).unwrap_err(),
        ServerError::NotFound { what: "operation" }
    ));
    assert!(matches!(
        core.operations().events(&pid_n(8), &op, 0, 16, NOW + 1).unwrap_err(),
        ServerError::NotFound { what: "operation" }
    ));
    assert!(matches!(
        core.operations().retry(&pid_n(8), &op, NOW + 1).unwrap_err(),
        ServerError::NotFound { what: "operation" }
    ));

    // Owner cancel: job dies with it; a second cancel is a no-op.
    assert!(core.operations().cancel(&pid_n(9), &op, NOW + 2).unwrap());
    assert_eq!(
        core.jobs().state(&job.job_id).unwrap(),
        Some(makepad_asset_store::JobState::Cancelled)
    );
    assert!(!core.operations().cancel(&pid_n(9), &op, NOW + 3).unwrap());
    let events = core.operations().events(&pid_n(9), &op, 0, 16, NOW + 4).unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "cancelled").count(),
        1,
        "idempotent cancel must not duplicate events"
    );
}

#[test]
fn failed_executor_flips_operation_and_retry_arms_next_round() {
    let (_root, core) = open_core("op_retry");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];
    let (op, job0) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job0, NOW);

    // Retry while in flight refuses.
    assert!(matches!(
        core.operations().retry(&pid_n(9), &op, NOW + 1).unwrap_err(),
        ServerError::InvalidState { what: "operation retry", state: "in flight" }
    ));

    // Worker claims and fails terminally (max_attempts = 1).
    let claimed = claim_op_job(&core, NOW + 2);
    core.jobs().fail(&claimed.job_id, WORKER, NOW + 3, 0).unwrap();

    // The lazy sync reports the truth and records the durable event.
    let snap = core.operations().get(&pid_n(9), &op, NOW + 4).unwrap();
    assert_eq!(snap.state, OperationState::Failed);
    assert!(snap.error.is_some());

    // Retry arms round 1 under a new deterministic job id.
    let (snap, job1) = core.operations().retry(&pid_n(9), &op, NOW + 5).unwrap();
    assert_eq!(snap.state, OperationState::Queued);
    assert_eq!(job1.round, 1);
    assert_ne!(job1.job_id, job0.job_id);
    enqueue_armed(&core, &job1, NOW + 5);

    // The superseded round-0 job can never finalize.
    let facts = good_facts(&core, 0x71);
    assert!(matches!(
        core.operations()
            .finalize(&op, &job0.job_id, WORKER, &facts, NOW + 6)
            .unwrap_err(),
        ServerError::LeaseLost { what: "superseded operation job" }
    ));

    // Round 1 completes normally.
    let claimed = claim_op_job(&core, NOW + 7);
    assert_eq!(claimed.job_id, job1.job_id);
    core.operations()
        .finalize(&op, &job1.job_id, WORKER, &facts, NOW + 8)
        .unwrap();
    let snap = core.operations().get(&pid_n(9), &op, NOW + 9).unwrap();
    assert_eq!(snap.state, OperationState::Succeeded);
    let events = core.operations().events(&pid_n(9), &op, 0, 16, NOW + 9).unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, vec!["created", "failed", "retried", "succeeded"]);

    // Event cursoring: `after` skips consumed rows.
    let tail = core.operations().events(&pid_n(9), &op, events[1].seq, 16, NOW + 9).unwrap();
    assert_eq!(tail.len(), 2);
    assert!(tail.iter().all(|e| e.seq > events[1].seq));
}

#[test]
fn quarantined_input_cannot_gain_new_derivatives() {
    let (_root, core) = open_core("op_quarantined_parent");
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];
    let (op, job) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job, NOW);
    let _ = claim_op_job(&core, NOW + 1);

    // The parent is pulled AFTER creation: the pinned revision still names
    // it exactly, but finalization refuses to derive from pulled content.
    core.catalog().quarantine_asset(&asset, &rev, NOW + 2).unwrap();
    let facts = good_facts(&core, 0x81);
    assert!(matches!(
        core.operations()
            .finalize(&op, &job.job_id, WORKER, &facts, NOW + 3)
            .unwrap_err(),
        ServerError::InvalidState { what: "operation input", state: "quarantined" }
    ));
    let snap = core.operations().get(&pid_n(9), &op, NOW + 4).unwrap();
    assert!(snap.result.is_none());
}

#[test]
fn retry_rounds_are_bounded() {
    let (_root, core) = open_core("op_retry_bound");
    let budgets = Budgets { max_operation_rounds: 2, ..Budgets::default_v1() };
    drop(core);
    let root = test_root("op_retry_bound2");
    let core = AssetServerCore::open(&root, budgets).unwrap();
    worker_live(&core, NOW);
    let (asset, rev) = publish_texture(&core, "gen", 1, b"png-src");
    let inputs = [binding(asset, rev)];
    let (op, job0) = created(
        core.operations()
            .create(&create_req(opid(1), 9, "k1", &inputs), NOW)
            .unwrap(),
    );
    enqueue_armed(&core, &job0, NOW);
    let claimed = claim_op_job(&core, NOW + 1);
    core.jobs().fail(&claimed.job_id, WORKER, NOW + 2, 0).unwrap();
    let (_, job1) = core.operations().retry(&pid_n(9), &op, NOW + 3).unwrap();
    enqueue_armed(&core, &job1, NOW + 3);
    let claimed = claim_op_job(&core, NOW + 4);
    core.jobs().fail(&claimed.job_id, WORKER, NOW + 5, 0).unwrap();
    assert!(matches!(
        core.operations().retry(&pid_n(9), &op, NOW + 6).unwrap_err(),
        ServerError::InvalidState { what: "operation retry", state: "rounds exhausted" }
    ));
}
