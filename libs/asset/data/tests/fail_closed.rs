//! Hostile-size/property tests. Every malformed, truncated, over-budget, or
//! non-canonical input must yield a structured `AssetDataError` — never a panic,
//! an allocation bomb, or a silently substituted value.

mod common;

use common::*;
use makepad_asset_data::*;

/// Decoding any strict prefix of a valid document errors cleanly, and any
/// appended byte is refused as trailing.
fn assert_total<T, F: Fn(&[u8]) -> Result<T, AssetDataError>>(bytes: &[u8], decode: F) {
    for len in 0..bytes.len() {
        assert!(decode(&bytes[..len]).is_err(), "prefix {len} decoded");
    }
    let mut extended = bytes.to_vec();
    extended.push(0);
    assert!(matches!(
        decode(&extended).err(),
        Some(AssetDataError::TrailingBytes) | Some(AssetDataError::OverBudget { .. })
    ));
}

#[test]
fn every_document_decode_is_total() {
    assert_total(
        &weapon_manifest().to_canonical_bytes().unwrap(),
        AssetManifest::from_canonical_bytes,
    );
    assert_total(
        &lock().to_canonical_bytes().unwrap(),
        ContentLock::from_canonical_bytes,
    );
    assert_total(
        &game_revision_manifest().to_canonical_bytes().unwrap(),
        GameRevisionManifest::from_canonical_bytes,
    );
    let game_rev = game_revision_manifest().revision().unwrap();
    let baseline = ContentSetManifest::baseline(game_rev, &lock()).unwrap();
    assert_total(
        &baseline.to_canonical_bytes().unwrap(),
        ContentSetManifest::from_canonical_bytes,
    );
    let plan = scene_plan(game_rev, baseline.id().unwrap());
    assert_total(
        &plan.to_canonical_bytes().unwrap(),
        ScenePlan::from_canonical_bytes,
    );
    let mig = migration_plan(game_rev, GameRevisionId::hash_of(b"next"));
    assert_total(
        &mig.to_canonical_bytes().unwrap(),
        SceneMigrationPlan::from_canonical_bytes,
    );
    let descriptor = RealmDescriptor {
        ticket: JoinTicket {
            transaction_id: txn(1),
            realm_epoch: RealmEpoch(1),
            game_revision: game_rev,
            content_set: baseline.id().unwrap(),
        },
        variant_policy_version: RESOLUTION_POLICY_V1,
        origins: vec!["http://198.51.100.1:8770".into()],
        read_capability: vec![9; 32],
    };
    assert_total(
        &descriptor.to_canonical_bytes().unwrap(),
        RealmDescriptor::from_canonical_bytes,
    );
}

#[test]
fn bad_magic_kind_and_version_refuse() {
    let bytes = weapon_manifest().to_canonical_bytes().unwrap();

    let mut bad = bytes.clone();
    bad[0] ^= 0xff;
    assert_eq!(
        AssetManifest::from_canonical_bytes(&bad).unwrap_err(),
        AssetDataError::BadMagic
    );

    let mut bad = bytes.clone();
    bad[4] = 200; // unknown document kind
    assert!(matches!(
        AssetManifest::from_canonical_bytes(&bad),
        Err(AssetDataError::BadDocKind { .. })
    ));

    let mut bad = bytes;
    bad[5] = 0xff; // future schema version
    assert!(matches!(
        AssetManifest::from_canonical_bytes(&bad),
        Err(AssetDataError::UnsupportedSchema { .. })
    ));
}

/// A forged file-count of 0xFFFF_FFFF must be refused before allocation.
#[test]
fn count_bomb_is_refused_before_allocation() {
    let bytes = weapon_manifest().to_canonical_bytes().unwrap();
    // Header (7) + asset_id (16) + kind (1), then the files count.
    let count_at = 7 + 16 + 1;
    let mut bad = bytes;
    bad[count_at..count_at + 4].copy_from_slice(&0xffff_ffffu32.to_be_bytes());
    assert!(matches!(
        AssetManifest::from_canonical_bytes(&bad),
        Err(AssetDataError::OverBudget { .. })
    ));
}

/// A forged string length larger than the remaining input must be refused
/// before allocation.
#[test]
fn string_bomb_is_refused() {
    let descriptor = RealmDescriptor {
        ticket: JoinTicket {
            transaction_id: txn(1),
            realm_epoch: RealmEpoch(1),
            game_revision: GameRevisionId::hash_of(b"g"),
            content_set: ContentSetId::hash_of(b"s"),
        },
        variant_policy_version: RESOLUTION_POLICY_V1,
        origins: vec!["x".into()],
        read_capability: vec![],
    };
    let bytes = descriptor.to_canonical_bytes().unwrap();
    // Origin string length sits right after ticket (16+8+32+32), the
    // variant policy version (4), and the origin count (4).
    let len_at = 7 + 16 + 8 + 32 + 32 + 4 + 4;
    let mut bad = bytes;
    bad[len_at..len_at + 4].copy_from_slice(&0x7fff_ffffu32.to_be_bytes());
    assert!(matches!(
        RealmDescriptor::from_canonical_bytes(&bad),
        Err(AssetDataError::OverBudget { .. })
    ));
}

/// NaN and -0.0 bit patterns are refused on decode even if bytes are forged
/// after encoding.
#[test]
fn non_canonical_floats_refuse_on_decode() {
    let bytes = weapon_manifest().to_canonical_bytes().unwrap();
    // bounds.min.x is -1.5: a distinctive, unique bit pattern in this fixture.
    let needle = (-1.5f32).to_bits().to_be_bytes();
    let at = bytes
        .windows(4)
        .position(|w| w == needle)
        .expect("fixture float present");
    for forged in [f32::NAN.to_bits(), 0x8000_0000, f32::INFINITY.to_bits()] {
        let mut bad = bytes.clone();
        bad[at..at + 4].copy_from_slice(&forged.to_be_bytes());
        assert!(
            matches!(
                AssetManifest::from_canonical_bytes(&bad),
                Err(AssetDataError::Malformed { .. })
            ),
            "float bits {forged:08x} accepted"
        );
    }
}

#[test]
fn producer_side_nan_is_refused() {
    let mut m = weapon_manifest();
    m.bounds.min.x = f32::NAN;
    assert!(m.to_canonical_bytes().is_err());
    let mut m = weapon_manifest();
    m.coordinate_system.units_per_meter = f32::INFINITY;
    assert!(m.to_canonical_bytes().is_err());
}

#[test]
fn mesh_thumbnail_contract_fails_closed() {
    // Missing thumbnail on a mesh-bearing kind.
    let mut m = weapon_manifest();
    m.thumbnail = None;
    assert_eq!(
        m.validate().unwrap_err(),
        AssetDataError::Missing {
            what: "mesh thumbnail"
        }
    );

    // Under the 256px minimum.
    let mut m = weapon_manifest();
    m.thumbnail.as_mut().unwrap().width = 128;
    assert!(matches!(
        m.validate(),
        Err(AssetDataError::TooSmall { .. })
    ));

    // Over the maximum.
    let mut m = weapon_manifest();
    m.thumbnail.as_mut().unwrap().height = 100_000;
    assert!(matches!(m.validate(), Err(AssetDataError::OverBudget { .. })));

    // A texture-only asset needs no thumbnail...
    let mut t = weapon_manifest();
    t.kind = AssetKind::Texture;
    t.thumbnail = None;
    t.metrics.total_bytes -= 300;
    assert!(t.validate().is_ok());
    // ...but a lying byte total is still refused.
    t.metrics.total_bytes += 1;
    assert!(matches!(t.validate(), Err(AssetDataError::Mismatch { .. })));
}

#[test]
fn asset_shape_violations_refuse() {
    // Role/media mismatch: a GLB posing as a PNG texture.
    let mut m = weapon_manifest();
    m.files[3].media = MediaType::Glb;
    m.files[3].dims = None;
    assert!(matches!(m.validate(), Err(AssetDataError::Mismatch { .. })));

    // Image without dimensions.
    let mut m = weapon_manifest();
    m.files[3].dims = None;
    assert!(matches!(m.validate(), Err(AssetDataError::Missing { .. })));

    // Duplicate (role, tier, lod).
    let mut m = weapon_manifest();
    let dup = m.files[0];
    m.files.insert(1, dup);
    assert!(matches!(m.validate(), Err(AssetDataError::Duplicate { .. })));

    // Duplicate anchor name.
    let mut m = weapon_manifest();
    let dup = m.anchors[0].clone();
    m.anchors.insert(1, dup);
    assert!(matches!(m.validate(), Err(AssetDataError::Duplicate { .. })));

    // Two pinned revisions of one dependency asset.
    let mut m = weapon_manifest();
    m.dependencies = vec![aref(2, 0x21), aref(2, 0x22)];
    assert!(matches!(m.validate(), Err(AssetDataError::Duplicate { .. })));

    // Self-dependency.
    let mut m = weapon_manifest();
    m.dependencies = vec![aref(1, 0x21)];
    assert!(matches!(m.validate(), Err(AssetDataError::Malformed { .. })));

    // Mesh without a render GLB role.
    let mut m = weapon_manifest();
    m.files.retain(|f| f.role != FileRole::RenderGlb);
    m.metrics.total_bytes -= 1000;
    assert!(matches!(m.validate(), Err(AssetDataError::Missing { .. })));

    // Over metric ceilings.
    let mut m = weapon_manifest();
    m.metrics.triangles = u32::MAX;
    assert!(matches!(m.validate(), Err(AssetDataError::OverBudget { .. })));

    // Missing license.
    let mut m = weapon_manifest();
    m.rights.license.clear();
    assert_eq!(
        m.validate().unwrap_err(),
        AssetDataError::Missing { what: "license" }
    );

    // Degenerate up/forward axes.
    let mut m = weapon_manifest();
    m.coordinate_system.forward = Axis::YNeg;
    assert!(matches!(m.validate(), Err(AssetDataError::Mismatch { .. })));
}

#[test]
fn lock_violations_refuse() {
    // Entry revision absent from the closure.
    let mut l = lock();
    l.entries[0].revision = arev(0x99);
    assert!(matches!(l.validate(), Err(AssetDataError::Missing { .. })));

    // Closure with two revisions of one asset.
    let mut l = lock();
    l.closure.push(aref(4, 0x42));
    l.canonicalize();
    assert!(matches!(l.validate(), Err(AssetDataError::Duplicate { .. })));

    // Unsorted entries refuse rather than reorder.
    let mut l = lock();
    l.entries.reverse();
    assert!(matches!(l.validate(), Err(AssetDataError::NotSorted { .. })));
}

#[test]
fn content_set_violations_refuse() {
    let game_rev = game_revision_manifest().revision().unwrap();
    let baseline = ContentSetManifest::baseline(game_rev, &lock()).unwrap();

    // Re-adding an existing revision cannot double-slot it.
    assert!(matches!(
        baseline.extended(&[aref(1, 0x11)]),
        Err(AssetDataError::Duplicate { .. })
    ));
    // A duplicate inside one delta is refused.
    assert!(matches!(
        baseline.extended(&[aref(5, 0x51), aref(5, 0x51)]),
        Err(AssetDataError::Duplicate { .. })
    ));

    // Tampering with an inherited slot breaks the parent prefix proof.
    let mut forged = baseline.extended(&[aref(5, 0x51)]).unwrap();
    forged.slots[0] = aref(9, 0x99);
    assert!(matches!(
        forged.extends(&baseline),
        Err(AssetDataError::Mismatch { .. })
    ));

    // Claiming a different parent fails the digest check.
    let other_parent = ContentSetManifest {
        game_revision: game_rev,
        parent: None,
        slots: vec![aref(9, 0x99)],
    };
    let extended = baseline.extended(&[aref(5, 0x51)]).unwrap();
    assert!(matches!(
        extended.extends(&other_parent),
        Err(AssetDataError::Mismatch { .. })
    ));
}

#[test]
fn scene_plan_violations_refuse() {
    let game_rev = game_revision_manifest().revision().unwrap();
    let set_id = ContentSetManifest::baseline(game_rev, &lock())
        .unwrap()
        .id()
        .unwrap();

    // Duplicate stable keys.
    let mut p = scene_plan(game_rev, set_id);
    let dup = p.objects[0].clone();
    p.objects.insert(1, dup);
    assert!(matches!(p.validate(), Err(AssetDataError::Duplicate { .. })));

    // Dangling key reference.
    let mut p = scene_plan(game_rev, set_id);
    p.objects.retain(|o| o.key.as_str() != "arena/switch");
    assert!(matches!(p.validate(), Err(AssetDataError::Missing { .. })));

    // State default of the wrong type.
    let mut p = scene_plan(game_rev, set_id);
    p.state_schemas[0].default = Value::Bool(true);
    assert!(matches!(p.validate(), Err(AssetDataError::Mismatch { .. })));

    // Zero scale is corruption, not a style.
    let mut p = scene_plan(game_rev, set_id);
    p.objects[0].transform.scale = Vec3::new(0.0, 1.0, 1.0);
    assert!(matches!(p.validate(), Err(AssetDataError::Malformed { .. })));
}

#[test]
fn migration_plan_cannot_downgrade_or_collide() {
    let from = game_revision_manifest().revision().unwrap();
    let to = GameRevisionId::hash_of(b"next");

    // Declared mode below the plan's own findings.
    let mut m = migration_plan(from, to);
    m.activation_mode = ActivationMode::HotPatch;
    assert_eq!(
        m.validate().unwrap_err(),
        AssetDataError::Mismatch {
            what: "activation mode below verified minimum"
        }
    );

    // A hard-reset-class reason forces HardReset even with no ops.
    let mut m = migration_plan(from, to);
    m.reasons.push(MigrationReason {
        code: MigrationReasonCode::OpaqueScriptState,
        key: None,
        detail: "closure captured".into(),
    });
    assert_eq!(m.verified_minimum(), ActivationMode::HardReset);
    assert!(m.validate().is_err()); // still declared Migrate
    m.activation_mode = ActivationMode::HardReset;
    assert!(m.validate().is_ok());

    // Escalation above the minimum is allowed (explicit host reset)...
    let mut m = migration_plan(from, to);
    m.activation_mode = ActivationMode::HardReset;
    assert!(m.validate().is_ok());

    // Rename target colliding with another op key.
    let mut m = migration_plan(from, to);
    m.ops[1].kind = SceneOpKind::RenameTo {
        new_key: "arena/turret".parse().unwrap(),
    };
    assert!(matches!(m.validate(), Err(AssetDataError::Duplicate { .. })));

    // Rename to itself.
    let mut m = migration_plan(from, to);
    m.ops[1].kind = SceneOpKind::RenameTo {
        new_key: "arena/old_wall".parse().unwrap(),
    };
    assert!(matches!(m.validate(), Err(AssetDataError::Malformed { .. })));

    // A no-op revision pair is refused.
    let mut m = migration_plan(from, to);
    m.to_game_revision = from;
    assert!(matches!(m.validate(), Err(AssetDataError::Mismatch { .. })));

    // State migration that does not advance its version.
    let mut m = migration_plan(from, to);
    m.state_migrations[0].to_version = 1;
    assert!(matches!(m.validate(), Err(AssetDataError::Malformed { .. })));
}

/// An update whose component rules include a typed-state migrator is a
/// migration by definition: the executor snapshots and transforms live state,
/// which hot patch promises never to do.
#[test]
fn migrate_component_rule_forbids_hot_patch() {
    let from = game_revision_manifest().revision().unwrap();
    let to = GameRevisionId::hash_of(b"next");
    let mut m = SceneMigrationPlan {
        from_game_revision: from,
        to_game_revision: to,
        activation_mode: ActivationMode::HotPatch,
        ops: vec![SceneOp {
            key: "arena/main_gate".parse().unwrap(),
            kind: SceneOpKind::Update {
                components: vec![ComponentRule {
                    component: "door".into(),
                    rule: PreserveRule::Migrate,
                }],
            },
        }],
        state_migrations: vec![],
        terrain_policy: TerrainPolicy::Keep,
        rebuild_scopes: RebuildScopes::default(),
        reasons: vec![MigrationReason {
            code: MigrationReasonCode::CompatibleParamChange,
            key: Some("arena/main_gate".parse().unwrap()),
            detail: "door params".into(),
        }],
        requires_user_confirmation: false,
    };
    assert_eq!(m.verified_minimum(), ActivationMode::Migrate);
    assert_eq!(
        m.validate().unwrap_err(),
        AssetDataError::Mismatch {
            what: "activation mode below verified minimum"
        }
    );
    m.activation_mode = ActivationMode::Migrate;
    assert!(m.validate().is_ok());

    // The same op with Preserve/Reset rules stays hot-patchable.
    m.activation_mode = ActivationMode::HotPatch;
    if let SceneOpKind::Update { components } = &mut m.ops[0].kind {
        components[0].rule = PreserveRule::Preserve;
    }
    assert!(m.validate().is_ok());
}

#[test]
fn migration_reason_order_is_canonical() {
    let from = game_revision_manifest().revision().unwrap();
    let to = GameRevisionId::hash_of(b"next");

    // Producer-shuffled reasons refuse rather than reorder silently.
    let mut m = migration_plan(from, to);
    m.reasons.reverse();
    assert!(matches!(
        m.to_canonical_bytes(),
        Err(AssetDataError::NotSorted { .. })
    ));

    // An exact duplicate finding is refused as noise.
    let mut m = migration_plan(from, to);
    let dup = m.reasons[0].clone();
    m.reasons.insert(1, dup);
    assert!(matches!(
        m.to_canonical_bytes(),
        Err(AssetDataError::Duplicate { .. })
    ));

    // Same code and key with different detail is two findings, ordered by
    // detail.
    let mut m = migration_plan(from, to);
    let mut second = m.reasons[0].clone();
    second.detail = "second finding".into();
    m.reasons.push(second);
    m.canonicalize();
    assert!(m.validate().is_ok());
}

/// Reordering the encoded reasons on the wire must be refused by the decoder,
/// proving decode really re-validates canonical order.
#[test]
fn forged_reason_order_refuses_on_decode() {
    let from = game_revision_manifest().revision().unwrap();
    let mig = migration_plan(from, GameRevisionId::hash_of(b"next"));
    let bytes = mig.to_canonical_bytes().unwrap();

    // Layout tail: ... count, reason1 (32 bytes, detail "new turret"),
    // reason2 (36 bytes, detail "wall removed"), requires_confirmation bool.
    let len = bytes.len();
    let r2 = &bytes[len - 1 - 36..len - 1];
    let r1 = &bytes[len - 1 - 36 - 32..len - 1 - 36];
    assert!(r1.ends_with(b"new turret"), "fixture layout drifted");
    assert!(r2.ends_with(b"wall removed"), "fixture layout drifted");

    let mut forged = bytes[..len - 1 - 36 - 32].to_vec();
    forged.extend_from_slice(r2);
    forged.extend_from_slice(r1);
    forged.push(bytes[len - 1]);
    assert!(matches!(
        SceneMigrationPlan::from_canonical_bytes(&forged),
        Err(AssetDataError::NotSorted { .. })
    ));
}

/// Spawnability is a promise about a recipe: no flag without a bounded valid
/// recipe, no recipe on a non-spawnable asset, and no recipe-less prefab.
#[test]
fn spawnability_requires_bounded_recipe() {
    // Positive: the weapon fixture is spawnable with a recipe.
    assert!(weapon_manifest().validate().is_ok());

    // Positive: a prefab carrying its recipe and the spawnable capability.
    let mut p = weapon_manifest();
    p.kind = AssetKind::Prefab;
    assert!(p.validate().is_ok());

    // Spawnable without a recipe refuses.
    let mut m = weapon_manifest();
    m.spawn_recipe = None;
    assert_eq!(
        m.validate().unwrap_err(),
        AssetDataError::Missing {
            what: "spawn recipe on spawnable asset"
        }
    );

    // A recipe on a non-spawnable asset is a contradiction.
    let mut m = weapon_manifest();
    m.capabilities.spawnable = false;
    assert_eq!(
        m.validate().unwrap_err(),
        AssetDataError::Mismatch {
            what: "spawn recipe on non-spawnable asset"
        }
    );

    // A prefab with neither flag nor recipe is an empty promise.
    let mut p = weapon_manifest();
    p.kind = AssetKind::Prefab;
    p.capabilities.spawnable = false;
    p.spawn_recipe = None;
    assert_eq!(
        p.validate().unwrap_err(),
        AssetDataError::Missing {
            what: "prefab spawn recipe"
        }
    );

    // Non-spawnable, recipe-free non-prefab stays valid.
    let mut m = weapon_manifest();
    m.capabilities.spawnable = false;
    m.spawn_recipe = None;
    assert!(m.validate().is_ok());
}

#[test]
fn activation_dto_violations_refuse() {
    let game_rev = GameRevisionId::hash_of(b"g");
    // Hard reset may never travel the scene-change path.
    let prepare = PrepareSceneChange {
        transaction_id: txn(1),
        realm_epoch: RealmEpoch(1),
        parent_game_revision: game_rev,
        next_game_revision: GameRevisionId::hash_of(b"g2"),
        parent_scene_sequence: SceneSequence(1),
        next_content_set: ContentSetId::hash_of(b"s"),
        scene_plan_digest: ScenePlanDigest::hash_of(b"p"),
        migration_plan_digest: MigrationPlanDigest::hash_of(b"m"),
        activation_mode: ActivationMode::HardReset,
        proposed_activation_tick: Tick(1),
        readiness: readiness(),
    };
    assert!(prepare.to_canonical_bytes().is_err());

    // A descriptor without origins is useless and refused.
    let descriptor = RealmDescriptor {
        ticket: JoinTicket {
            transaction_id: txn(1),
            realm_epoch: RealmEpoch(1),
            game_revision: game_rev,
            content_set: ContentSetId::hash_of(b"s"),
        },
        variant_policy_version: RESOLUTION_POLICY_V1,
        origins: vec![],
        read_capability: vec![],
    };
    assert!(descriptor.to_canonical_bytes().is_err());

    // An unknown variant-resolution policy version refuses: a peer never
    // guesses which resolution algorithm a realm meant.
    let descriptor = RealmDescriptor {
        ticket: JoinTicket {
            transaction_id: txn(1),
            realm_epoch: RealmEpoch(1),
            game_revision: game_rev,
            content_set: ContentSetId::hash_of(b"s"),
        },
        variant_policy_version: 99,
        origins: vec!["http://198.51.100.1:8770".into()],
        read_capability: vec![],
    };
    assert!(matches!(
        descriptor.to_canonical_bytes(),
        Err(AssetDataError::UnsupportedSchema { .. })
    ));

    // An empty required delta means nothing to prepare: refused.
    let prep = PrepareContentChange {
        transaction_id: txn(1),
        realm_epoch: RealmEpoch(1),
        next_set: ContentSetId::hash_of(b"s2"),
        required_delta: vec![],
        proposed_activation_tick: Tick(1),
        readiness: readiness(),
    };
    assert!(prep.to_canonical_bytes().is_err());

    // Over-budget capability token.
    let descriptor = RealmDescriptor {
        ticket: JoinTicket {
            transaction_id: txn(1),
            realm_epoch: RealmEpoch(1),
            game_revision: game_rev,
            content_set: ContentSetId::hash_of(b"s"),
        },
        variant_policy_version: RESOLUTION_POLICY_V1,
        origins: vec!["http://198.51.100.1:8770".into()],
        read_capability: vec![0; 4096],
    };
    assert!(descriptor.to_canonical_bytes().is_err());
}

#[test]
fn oversized_document_is_refused_symmetrically() {
    // A plan with the maximum object count would exceed the document budget;
    // the writer refuses to emit it, so no decoder ever sees it.
    let game_rev = game_revision_manifest().revision().unwrap();
    let set_id = ContentSetId::hash_of(b"s");
    let mut plan = scene_plan(game_rev, set_id);
    let base: SceneObjectKey = "bulk".parse().unwrap();
    for i in 0..20_000u32 {
        plan.objects.push(SceneObject {
            key: base.child_indexed("obj", i).unwrap(),
            asset: None,
            transform: Transform::IDENTITY,
            fixed: true,
            components: vec![],
        });
    }
    plan.canonicalize();
    assert!(matches!(
        plan.to_canonical_bytes(),
        Err(AssetDataError::OverBudget { .. })
    ));
}
