//! Derived variants: single-flight derivation, deterministic job identity,
//! validated completion, late-duplicate arbitration, retry re-arming, frozen
//! variant sets, and deterministic profile resolution.

mod common;
use common::*;
use makepad_asset_store::*;
use makepad_asset_data::*;

const LEASE: u64 = 60_000;

fn tool() -> ToolClosure {
    ToolClosure {
        processor: "mp_derive".into(),
        version: "1.0".into(),
        build: "deadbeef".into(),
        deterministic: true,
    }
}

fn thumb_recipe() -> ProcessingRecipe {
    ProcessingRecipe {
        settings: RecipeSettings::MeshThumbnail {
            width: 512,
            height: 512,
            media: ThumbnailMedia::Png,
        },
        tool: tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    }
}

fn lod_recipe() -> ProcessingRecipe {
    ProcessingRecipe {
        settings: RecipeSettings::MeshLod {
            lod: 1,
            target_triangles: 8,
        },
        tool: tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    }
}

/// The deterministic bytes the fixture "worker" produces for one recipe over
/// one input: a pure function of both, standing in for a real kernel.
fn worker_bytes(tag: &str, input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(tag.as_bytes());
    out.push(b':');
    out.extend_from_slice(&sha256(input));
    out
}

fn thumb_result(core: &AssetServerCore, now: u64) -> DerivedResult {
    let bytes = worker_bytes("THUMB-512", PACK_GLB);
    core.put_blob(&bytes, now).unwrap();
    DerivedResult {
        outputs: vec![],
        thumbnail: Some(ThumbnailMeta {
            blob: BlobId::hash_of(&bytes),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: bytes.len() as u64,
        }),
        metrics: Metrics {
            total_bytes: bytes.len() as u64,
            ..Default::default()
        },
    }
}

fn lod_result(core: &AssetServerCore, now: u64) -> DerivedResult {
    let bytes = worker_bytes("LOD1-T8", PACK_GLB);
    core.put_blob(&bytes, now).unwrap();
    DerivedResult {
        outputs: vec![AssetFile {
            role: FileRole::Lod1Glb,
            tier: DeviceTier::Low,
            lod: 1,
            media: MediaType::Glb,
            blob: BlobId::hash_of(&bytes),
            byte_len: bytes.len() as u64,
            dims: None,
        }],
        thumbnail: None,
        metrics: Metrics {
            total_bytes: bytes.len() as u64,
            triangles: 6,
            vertices: 5,
            ..Default::default()
        },
    }
}

/// Arm a derivation, enqueue its job, claim it as `worker`, and return the
/// (dkey, job) pair ready for completion.
fn arm_and_claim(
    core: &AssetServerCore,
    base: &AssetRevisionRef,
    recipe: &ProcessingRecipe,
    worker: &str,
    now: u64,
) -> (DerivationKey, JobId) {
    let recipe_bytes = recipe.to_canonical_bytes().unwrap();
    let outcome = core.variants().begin_derivation(base, &recipe_bytes, now).unwrap();
    let DerivationOutcome::NeedsJob { dkey, job_id, kind, .. } = outcome else {
        panic!("expected NeedsJob, got {outcome:?}");
    };
    core.jobs()
        .enqueue(
            &NewJob {
                job_id,
                parent: None,
                kind,
                payload: b"{}",
                priority: 0,
                max_attempts: 1,
                not_before_ms: 0,
                deps: &[],
            },
            now,
        )
        .unwrap();
    let claimed = core
        .jobs()
        .claim_allowed(worker, now, LEASE, &[kind])
        .unwrap()
        .expect("armed job is claimable");
    assert_eq!(claimed.job_id, job_id);
    (dkey, job_id)
}

/// Import the Kenney pack and return the watchtower's exact base ref.
fn watchtower_base(core: &AssetServerCore, now: u64) -> AssetRevisionRef {
    let report = run_kenney_import(core, "1.0", now);
    let entry = report
        .entries
        .iter()
        .find(|e| e.key == "models/watchtower")
        .unwrap();
    AssetRevisionRef {
        asset_id: entry.asset_id,
        revision: entry.revision,
    }
}

#[test]
fn single_flight_derivation_and_cache_hit() {
    let (_root, core) = open_core("variants_single_flight");
    let base = watchtower_base(&core, NOW);
    let recipe_bytes = thumb_recipe().to_canonical_bytes().unwrap();

    let (dkey, job) = arm_and_claim(&core, &base, &thumb_recipe(), "w-1", NOW);
    // A concurrent identical request joins the live job — no second job.
    match core.variants().begin_derivation(&base, &recipe_bytes, NOW + 1).unwrap() {
        DerivationOutcome::InFlight { dkey: k, job_id } => {
            assert_eq!((k, job_id), (dkey, job));
        }
        other => panic!("expected InFlight, got {other:?}"),
    }
    // Status reads pending while the job runs.
    let status = core.variants().derivation_status(&dkey).unwrap().unwrap();
    assert_eq!((status.state, status.round), ("pending", 0));

    let result = thumb_result(&core, NOW + 2);
    let variant = core
        .variants()
        .complete_derivation(&dkey, &job, "w-1", &result, NOW + 2)
        .unwrap();
    // Ready: the job succeeded atomically with publication.
    assert_eq!(core.jobs().state(&job).unwrap(), Some(JobState::Succeeded));
    let status = core.variants().derivation_status(&dkey).unwrap().unwrap();
    assert_eq!((status.state, status.variant), ("ready", Some(variant)));

    // A later identical request is a pure cache hit.
    match core.variants().begin_derivation(&base, &recipe_bytes, NOW + 3).unwrap() {
        DerivationOutcome::Ready { variant: v, .. } => assert_eq!(v, variant),
        other => panic!("expected Ready, got {other:?}"),
    }
    // The stored manifest is canonical, validated, and recipe-bound.
    let bytes = core.variants().variant_manifest(&variant).unwrap().unwrap();
    let manifest = DerivedVariantManifest::from_canonical_bytes(&bytes).unwrap();
    thumb_recipe().validate_result(&manifest).unwrap();
    assert_eq!(manifest.base, base);
    assert_eq!(manifest.rights.license, "CC0-1.0");
}

#[test]
fn two_clean_servers_derive_identical_variant_ids() {
    let (_ra, a) = open_core("variants_det_a");
    let (_rb, b) = open_core("variants_det_b");
    let mut ids = Vec::new();
    for core in [&a, &b] {
        let base = watchtower_base(core, NOW);
        let (dkey, job) = arm_and_claim(core, &base, &thumb_recipe(), "w-1", NOW);
        let result = thumb_result(core, NOW);
        ids.push(
            core.variants()
                .complete_derivation(&dkey, &job, "w-1", &result, NOW + 1)
                .unwrap(),
        );
    }
    assert_eq!(ids[0], ids[1]);
    assert_eq!(
        a.variants().variant_manifest(&ids[0]).unwrap(),
        b.variants().variant_manifest(&ids[1]).unwrap(),
    );
}

#[test]
fn completion_validates_against_the_recipe_and_the_store() {
    let (_root, core) = open_core("variants_validation");
    let base = watchtower_base(&core, NOW);
    let (dkey, job) = arm_and_claim(&core, &base, &thumb_recipe(), "w-1", NOW);

    // Wrong dimensions: refused, derivation stays pending, lease survives.
    let mut wrong_dims = thumb_result(&core, NOW);
    wrong_dims.thumbnail.as_mut().unwrap().width = 256;
    assert!(matches!(
        core.variants().complete_derivation(&dkey, &job, "w-1", &wrong_dims, NOW + 1),
        Err(ServerError::Content(AssetDataError::Mismatch { .. }))
    ));
    // Unuploaded output blob: refused.
    let mut missing_blob = thumb_result(&core, NOW);
    missing_blob.thumbnail.as_mut().unwrap().blob = BlobId::hash_of(b"never uploaded");
    assert!(matches!(
        core.variants().complete_derivation(&dkey, &job, "w-1", &missing_blob, NOW + 1),
        Err(ServerError::NotFound { what: "variant thumbnail blob" })
    ));
    // A worker without the lease cannot publish.
    let good = thumb_result(&core, NOW);
    assert!(matches!(
        core.variants().complete_derivation(&dkey, &job, "w-2", &good, NOW + 1),
        Err(ServerError::LeaseLost { .. })
    ));
    let status = core.variants().derivation_status(&dkey).unwrap().unwrap();
    assert_eq!(status.state, "pending");

    // The holding worker with valid facts still lands it.
    core.variants()
        .complete_derivation(&dkey, &job, "w-1", &good, NOW + 2)
        .unwrap();
}

#[test]
fn late_duplicate_completion_cannot_replace_the_winner() {
    let (_root, core) = open_core("variants_late_dup");
    let base = watchtower_base(&core, NOW);
    let (dkey, job) = arm_and_claim(&core, &base, &thumb_recipe(), "w-1", NOW);
    let result = thumb_result(&core, NOW);
    let winner = core
        .variants()
        .complete_derivation(&dkey, &job, "w-1", &result, NOW + 1)
        .unwrap();

    // Identical late report: idempotent success.
    assert_eq!(
        core.variants()
            .complete_derivation(&dkey, &job, "w-1", &result, NOW + 2)
            .unwrap(),
        winner
    );
    // Divergent late report: refused, winner untouched.
    let mut divergent_bytes = worker_bytes("THUMB-512-DIVERGENT", PACK_GLB);
    divergent_bytes.push(b'!');
    core.put_blob(&divergent_bytes, NOW + 2).unwrap();
    let mut divergent = thumb_result(&core, NOW + 2);
    divergent.thumbnail.as_mut().unwrap().blob = BlobId::hash_of(&divergent_bytes);
    divergent.thumbnail.as_mut().unwrap().byte_len = divergent_bytes.len() as u64;
    divergent.metrics.total_bytes = divergent_bytes.len() as u64;
    assert!(matches!(
        core.variants().complete_derivation(&dkey, &job, "w-1", &divergent, NOW + 3),
        Err(ServerError::Conflict { what: "late duplicate derivation" })
    ));
    let bytes = core.variants().variant_manifest(&winner).unwrap().unwrap();
    let manifest = DerivedVariantManifest::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(manifest.thumbnail.unwrap().blob, result.thumbnail.unwrap().blob);
}

#[test]
fn terminal_failure_rearms_with_a_fresh_deterministic_job() {
    let (_root, core) = open_core("variants_rearm");
    let base = watchtower_base(&core, NOW);
    let recipe_bytes = lod_recipe().to_canonical_bytes().unwrap();
    let (dkey, job0) = arm_and_claim(&core, &base, &lod_recipe(), "w-1", NOW);

    // max_attempts=1: one failure is terminal.
    assert_eq!(
        core.jobs().fail(&job0, "w-1", NOW + 1, 0).unwrap(),
        JobState::Failed
    );
    // Status reads failed by joining the job state.
    let status = core.variants().derivation_status(&dkey).unwrap().unwrap();
    assert_eq!(status.state, "failed");

    // A new request re-arms round 1 with a different deterministic job id.
    let outcome = core
        .variants()
        .begin_derivation(&base, &recipe_bytes, NOW + 2)
        .unwrap();
    let DerivationOutcome::NeedsJob { job_id: job1, kind, .. } = outcome else {
        panic!("expected NeedsJob, got {outcome:?}");
    };
    assert_ne!(job0, job1);
    core.jobs()
        .enqueue(
            &NewJob {
                job_id: job1,
                parent: None,
                kind,
                payload: b"{}",
                priority: 0,
                max_attempts: 1,
                not_before_ms: 0,
                deps: &[],
            },
            NOW + 2,
        )
        .unwrap();
    core.jobs()
        .claim_allowed("w-1", NOW + 2, LEASE, &[kind])
        .unwrap()
        .expect("re-armed job claimable");
    // A late completion on the SUPERSEDED job refuses.
    let result = lod_result(&core, NOW + 2);
    assert!(matches!(
        core.variants().complete_derivation(&dkey, &job0, "w-1", &result, NOW + 3),
        Err(ServerError::LeaseLost { what: "superseded derivation job" })
    ));
    // The live round completes normally and dedupes thereafter.
    let variant = core
        .variants()
        .complete_derivation(&dkey, &job1, "w-1", &result, NOW + 3)
        .unwrap();
    match core.variants().begin_derivation(&base, &recipe_bytes, NOW + 4).unwrap() {
        DerivationOutcome::Ready { variant: v, .. } => assert_eq!(v, variant),
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[test]
fn crash_between_arm_and_enqueue_reoffers_the_same_job() {
    let (_root, core) = open_core("variants_crash_repair");
    let base = watchtower_base(&core, NOW);
    let recipe_bytes = thumb_recipe().to_canonical_bytes().unwrap();
    // Arm round 0 but "crash" before enqueue.
    let DerivationOutcome::NeedsJob { job_id: first, .. } = core
        .variants()
        .begin_derivation(&base, &recipe_bytes, NOW)
        .unwrap()
    else {
        panic!("expected NeedsJob");
    };
    // Repair: the exact same deterministic job id is offered again.
    let DerivationOutcome::NeedsJob { job_id: second, .. } = core
        .variants()
        .begin_derivation(&base, &recipe_bytes, NOW + 1)
        .unwrap()
    else {
        panic!("expected NeedsJob again");
    };
    assert_eq!(first, second);
}

#[test]
fn derivation_refuses_bad_bases_and_missing_input_roles() {
    let (_root, core) = open_core("variants_bad_base");
    let report = run_kenney_import(&core, "1.0", NOW);
    let texture = report
        .entries
        .iter()
        .find(|e| e.key == "textures/hull-panel")
        .unwrap();
    let texture_base = AssetRevisionRef {
        asset_id: texture.asset_id,
        revision: texture.revision,
    };
    // A mesh recipe on a texture asset: the input role does not exist.
    assert!(matches!(
        core.variants().begin_derivation(
            &texture_base,
            &thumb_recipe().to_canonical_bytes().unwrap(),
            NOW,
        ),
        Err(ServerError::NotFound { what: "recipe input role in base" })
    ));
    // An image recipe on the texture works: role Texture exists.
    let resize = ProcessingRecipe {
        settings: RecipeSettings::ImageResize {
            source_role: FileRole::Texture,
            width: 256,
            height: 256,
            media: ThumbnailMedia::Png,
        },
        tool: tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    };
    assert!(matches!(
        core.variants()
            .begin_derivation(&texture_base, &resize.to_canonical_bytes().unwrap(), NOW),
        Ok(DerivationOutcome::NeedsJob { .. })
    ));
    // Unknown base revision refuses.
    let ghost = AssetRevisionRef {
        asset_id: texture.asset_id,
        revision: AssetRevisionId::hash_of(b"ghost"),
    };
    assert!(matches!(
        core.variants()
            .begin_derivation(&ghost, &resize.to_canonical_bytes().unwrap(), NOW),
        Err(ServerError::NotFound { what: "derivation base revision" })
    ));
    // A quarantined base refuses new derivations.
    let watchtower = report
        .entries
        .iter()
        .find(|e| e.key == "models/watchtower")
        .unwrap();
    core.catalog()
        .quarantine_asset(&watchtower.asset_id, &watchtower.revision, NOW + 1)
        .unwrap();
    let quarantined = AssetRevisionRef {
        asset_id: watchtower.asset_id,
        revision: watchtower.revision,
    };
    assert!(matches!(
        core.variants().begin_derivation(
            &quarantined,
            &thumb_recipe().to_canonical_bytes().unwrap(),
            NOW + 2,
        ),
        Err(ServerError::InvalidState { what: "derivation base", state: "quarantined" })
    ));
    // A nondeterministic tool cannot even encode: validation refuses first,
    // so it can never reach a derivation key.
    let mut nondet = thumb_recipe();
    nondet.tool.deterministic = false;
    assert!(nondet.to_canonical_bytes().is_err());
}

#[test]
fn forbidden_derivatives_fail_closed_and_rights_inherit_exactly() {
    let (_root, core) = open_core("variants_rights");
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        core.put_blob(bytes, NOW).unwrap();
    }
    // A source whose registered terms forbid derivatives.
    let mut locked_terms = cc_by_terms();
    locked_terms.derivatives = DerivativePolicy::Forbidden;
    let locked = collection_with_terms("noderiv", locked_terms);
    core.imports()
        .register_source(&locked.to_canonical_bytes().unwrap(), NOW)
        .unwrap();
    let report = core
        .imports()
        .run_import(&pack_with_terms(&locked).to_canonical_bytes().unwrap(), NOW)
        .unwrap();
    let entry = report
        .entries
        .iter()
        .find(|e| e.key == "models/watchtower")
        .unwrap();
    let locked_base = AssetRevisionRef {
        asset_id: entry.asset_id,
        revision: entry.revision,
    };
    // Derivation refuses outright, whatever capability the caller holds.
    assert!(matches!(
        core.variants().begin_derivation(
            &locked_base,
            &thumb_recipe().to_canonical_bytes().unwrap(),
            NOW + 1,
        ),
        Err(ServerError::InvalidState {
            what: "derivation rights",
            state: "derivatives forbidden"
        })
    ));

    // An allowed base's derived manifest inherits the base's EXACT rights
    // record — nothing weakened, nothing dropped.
    let base = watchtower_base(&core, NOW + 2);
    let (dkey, job) = arm_and_claim(&core, &base, &thumb_recipe(), "w-1", NOW + 2);
    let variant = core
        .variants()
        .complete_derivation(&dkey, &job, "w-1", &thumb_result(&core, NOW + 2), NOW + 3)
        .unwrap();
    let base_manifest = AssetManifest::from_canonical_bytes(
        &core
            .catalog()
            .asset_revision_manifest(&base.revision)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let derived = DerivedVariantManifest::from_canonical_bytes(
        &core.variants().variant_manifest(&variant).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(derived.rights, base_manifest.rights);
    assert_eq!(derived.rights.terms_digest, Some(sha256(b"CC0-1.0 legal text")));
    assert_eq!(
        derived.rights.source_archive,
        Some(sha256(b"space-kit-1.0.zip"))
    );
}

#[test]
fn variant_sets_freeze_and_resolve_deterministically() {
    let (_root, core) = open_core("variants_sets");
    let base = watchtower_base(&core, NOW);

    let (k1, thumb_job) = arm_and_claim(&core, &base, &thumb_recipe(), "w-1", NOW);
    let thumb_variant = core
        .variants()
        .complete_derivation(&k1, &thumb_job, "w-1", &thumb_result(&core, NOW), NOW + 1)
        .unwrap();
    let (k2, lod_job) = arm_and_claim(&core, &base, &lod_recipe(), "w-1", NOW + 1);
    let lod_variant = core
        .variants()
        .complete_derivation(&k2, &lod_job, "w-1", &lod_result(&core, NOW + 1), NOW + 2)
        .unwrap();

    // Freeze is idempotent by digest and order-independent.
    let set_a = core
        .variants()
        .freeze_variant_set(&base, &[thumb_variant, lod_variant], NOW + 3)
        .unwrap();
    let set_b = core
        .variants()
        .freeze_variant_set(&base, &[lod_variant, thumb_variant], NOW + 4)
        .unwrap();
    assert_eq!(set_a, set_b);
    let set_bytes = core.variants().variant_set_manifest(&set_a).unwrap().unwrap();
    let set = VariantSetManifest::from_canonical_bytes(&set_bytes).unwrap();
    assert_eq!(set.base, base);
    assert_eq!(set.variants.len(), 2);

    // Unknown variants and foreign bases refuse.
    assert!(matches!(
        core.variants()
            .freeze_variant_set(&base, &[DerivedVariantId::hash_of(b"ghost")], NOW),
        Err(ServerError::NotFound { what: "variant for set" })
    ));
    let foreign = AssetRevisionRef {
        asset_id: base.asset_id,
        revision: AssetRevisionId::hash_of(b"other rev"),
    };
    assert!(matches!(
        core.variants().freeze_variant_set(&foreign, &[thumb_variant], NOW),
        Err(ServerError::Conflict { what: "variant set base" })
    ));

    // Deterministic resolution: full-featured profile takes both roles.
    let profile = ClientProfile {
        policy_version: RESOLUTION_POLICY_V1,
        tier: DeviceTier::High,
        max_texture_dim: 2048,
        max_triangles: 1_000_000,
        max_variant_bytes: 64 * 1024 * 1024,
        accept_png: true,
        accept_jpeg: true,
        accept_glb: true,
        accept_bin: true,
    };
    let map = core.variants().resolve(&set_a, &profile).unwrap();
    assert_eq!(map.entries.len(), 2);
    assert_eq!(map.entries[0].role, VariantRole::Thumbnail);
    assert_eq!(map.entries[0].variant, thumb_variant);
    assert_eq!(map.entries[1].role, VariantRole::File(FileRole::Lod1Glb));
    assert_eq!(map.entries[1].variant, lod_variant);
    // Same inputs, same digest on repeat: resolution is pure.
    assert_eq!(
        map.digest().unwrap(),
        core.variants().resolve(&set_a, &profile).unwrap().digest().unwrap()
    );
    // A profile that cannot take GLB fails closed on the LOD role.
    let mut no_glb = profile;
    no_glb.accept_glb = false;
    assert!(matches!(
        core.variants().resolve(&set_a, &no_glb),
        Err(ServerError::Content(AssetDataError::Missing {
            what: "compatible variant for role"
        }))
    ));
    // Resolving an unknown set refuses.
    assert!(matches!(
        core.variants()
            .resolve(&VariantSetId::hash_of(b"ghost set"), &profile),
        Err(ServerError::NotFound { what: "variant set" })
    ));
}
