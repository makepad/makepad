//! Canonical encoding/hashing fixtures: two independent processes must
//! encode the same manifests to the same hash.
//!
//! The golden digests below are the frozen contract for schema version 1. If
//! one changes, the encoding changed for every peer: that is a schema bump,
//! not a test update.

mod common;

use common::*;
use makepad_asset_data::*;

// Frozen golden digests for CONTENT_SCHEMA_VERSION 3.
const GOLDEN_WEAPON_REVISION: &str =
    "arev_1b574c4720123d58814b1f247a31d9cd7eeea7bbe6c9ce004a383bc53cdafb39";
const GOLDEN_LOCK_BLOB: &str =
    "sha256:520fcf64b8e570ccbf94714f1df166a473674ff32030733e769db180a85d7fd2";
const GOLDEN_GAME_REVISION: &str =
    "grev_43246c4a488b20c53903d71ad146cf0028248ae9ecf70968237c66e0e889ab54";
const GOLDEN_BASELINE_SET: &str =
    "cset_b16ab12ba146ceae0f5bcf6de2289558aa75317e2c0c09e8d6e7ebb0a1b3c841";
const GOLDEN_SCENE_PLAN: &str =
    "splan_3436db8c59fb1dba34b820b209fbfacd2064167b22be96c3cf38a34c740e766f";
const GOLDEN_MIGRATION_PLAN: &str =
    "mplan_5fc48af85a631dc4f7728b32e0976ec99b34bbca09f3cee1b4dc6654edc1918f";

#[test]
fn asset_manifest_roundtrip_and_golden_digest() {
    let manifest = weapon_manifest();
    let bytes = manifest.to_canonical_bytes().unwrap();
    let back = AssetManifest::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(back, manifest);
    // Encode-decode-encode is byte-identical: the encoding is a bijection on
    // accepted documents.
    assert_eq!(back.to_canonical_bytes().unwrap(), bytes);
    assert_eq!(manifest.revision().unwrap().to_string(), GOLDEN_WEAPON_REVISION);
}

#[test]
fn producer_field_order_cannot_change_the_hash() {
    // A producer that filled its vectors in reverse (map iteration, discovery
    // order) canonicalizes to the identical bytes...
    let mut shuffled = weapon_manifest();
    shuffled.files.reverse();
    shuffled.dependencies.reverse();
    shuffled.anchors.reverse();
    shuffled.spawn_recipe.as_mut().unwrap().params.reverse();
    // ...but is refused, not silently reordered, if it skips canonicalize.
    assert!(matches!(
        shuffled.to_canonical_bytes(),
        Err(AssetDataError::NotSorted { .. })
    ));
    shuffled.canonicalize();
    assert_eq!(
        shuffled.to_canonical_bytes().unwrap(),
        weapon_manifest().to_canonical_bytes().unwrap()
    );
}

#[test]
fn lock_baseline_set_and_game_revision_goldens() {
    let lock = lock();
    assert_eq!(lock.blob_id().unwrap().to_string(), GOLDEN_LOCK_BLOB);

    let game = game_revision_manifest();
    let game_rev = game.revision().unwrap();
    assert_eq!(game_rev.to_string(), GOLDEN_GAME_REVISION);

    let baseline = ContentSetManifest::baseline(game_rev, &lock).unwrap();
    assert_eq!(baseline.id().unwrap().to_string(), GOLDEN_BASELINE_SET);

    // The baseline slot table is the lock closure in canonical order.
    assert_eq!(baseline.slots, lock.closure);
    assert_eq!(baseline.get(AssetSlot(0)), Some(&aref(1, 0x11)));
    assert_eq!(baseline.slot_of(&aref(4, 0x41)), Some(AssetSlot(3)));

    // Alias resolution against the lock is exact.
    let alias: AssetAlias = "rik2/weapons/fancy-rocket-launcher".parse().unwrap();
    assert_eq!(lock.resolve(&alias), Some(aref(1, 0x11)));
    let missing: AssetAlias = "rik2/weapons/unknown".parse().unwrap();
    assert_eq!(lock.resolve(&missing), None);

    let bytes = lock.to_canonical_bytes().unwrap();
    assert_eq!(ContentLock::from_canonical_bytes(&bytes).unwrap(), lock);
    let bytes = game.to_canonical_bytes().unwrap();
    assert_eq!(
        GameRevisionManifest::from_canonical_bytes(&bytes).unwrap(),
        game
    );
}

#[test]
fn content_set_extension_is_append_only_and_verified() {
    let lock = lock();
    let game_rev = game_revision_manifest().revision().unwrap();
    let baseline = ContentSetManifest::baseline(game_rev, &lock).unwrap();

    // Extension appends in canonical order regardless of the caller's order.
    let extended = baseline.extended(&[aref(6, 0x61), aref(5, 0x51)]).unwrap();
    assert_eq!(extended.parent, Some(baseline.id().unwrap()));
    assert_eq!(extended.slots.len(), baseline.slots.len() + 2);
    assert_eq!(
        extended.get(AssetSlot(baseline.slots.len() as u32)),
        Some(&aref(5, 0x51))
    );
    // Existing slots keep their exact assignment.
    for (i, slot) in baseline.slots.iter().enumerate() {
        assert_eq!(extended.get(AssetSlot(i as u32)), Some(slot));
    }
    extended.extends(&baseline).unwrap();

    // Chained extension still verifies against its own parent.
    let extended2 = extended.extended(&[aref(8, 0x81)]).unwrap();
    extended2.extends(&extended).unwrap();
    assert!(extended2.extends(&baseline).is_err());

    let bytes = extended.to_canonical_bytes().unwrap();
    assert_eq!(
        ContentSetManifest::from_canonical_bytes(&bytes).unwrap(),
        extended
    );
}

#[test]
fn scene_and_migration_plan_goldens() {
    let lock = lock();
    let game_rev = game_revision_manifest().revision().unwrap();
    let set_id = ContentSetManifest::baseline(game_rev, &lock)
        .unwrap()
        .id()
        .unwrap();

    let plan = scene_plan(game_rev, set_id);
    assert_eq!(plan.digest().unwrap().to_string(), GOLDEN_SCENE_PLAN);
    let bytes = plan.to_canonical_bytes().unwrap();
    assert_eq!(ScenePlan::from_canonical_bytes(&bytes).unwrap(), plan);

    let key: SceneObjectKey = "arena/main_gate".parse().unwrap();
    assert!(plan.object(&key).is_some());
    assert_eq!(plan.asset_refs().count(), 1);

    let to_rev = GameRevisionId::hash_of(b"next");
    let mig = migration_plan(game_rev, to_rev);
    assert_eq!(mig.digest().unwrap().to_string(), GOLDEN_MIGRATION_PLAN);
    let bytes = mig.to_canonical_bytes().unwrap();
    assert_eq!(SceneMigrationPlan::from_canonical_bytes(&bytes).unwrap(), mig);
    assert_eq!(mig.verified_minimum(), ActivationMode::Migrate);

    // Reason discovery order cannot change the plan digest: shuffled input
    // canonicalizes to the identical bytes.
    let mut shuffled = migration_plan(game_rev, to_rev);
    shuffled.reasons.reverse();
    shuffled.canonicalize();
    assert_eq!(shuffled.to_canonical_bytes().unwrap(), bytes);
}

#[test]
fn activation_dtos_roundtrip() {
    let game_rev = GameRevisionId::hash_of(b"g");
    let set = ContentSetId::hash_of(b"s");
    let ticket = JoinTicket {
        transaction_id: txn(1),
        realm_epoch: RealmEpoch(3),
        game_revision: game_rev,
        content_set: set,
    };

    let descriptor = RealmDescriptor {
        ticket,
        variant_policy_version: RESOLUTION_POLICY_V1,
        origins: vec!["http://192.0.2.7:8770".into()],
        read_capability: vec![1, 2, 3],
    };
    let bytes = descriptor.to_canonical_bytes().unwrap();
    assert_eq!(
        RealmDescriptor::from_canonical_bytes(&bytes).unwrap(),
        descriptor
    );

    let ready = JoinContentReady {
        ticket,
        resolution: rres(5),
    };
    let bytes = ready.to_canonical_bytes().unwrap();
    assert_eq!(JoinContentReady::from_canonical_bytes(&bytes).unwrap(), ready);

    let scene_ready = SceneContentReady {
        transaction_id: txn(2),
        realm_epoch: RealmEpoch(3),
        next_game_revision: GameRevisionId::hash_of(b"g2"),
        next_content_set: set,
        resolution: rres(5),
    };
    let bytes = scene_ready.to_canonical_bytes().unwrap();
    assert_eq!(
        SceneContentReady::from_canonical_bytes(&bytes).unwrap(),
        scene_ready
    );

    let prepare = PrepareSceneChange {
        transaction_id: txn(2),
        realm_epoch: RealmEpoch(3),
        parent_game_revision: game_rev,
        next_game_revision: GameRevisionId::hash_of(b"g2"),
        parent_scene_sequence: SceneSequence(7),
        next_content_set: set,
        scene_plan_digest: ScenePlanDigest::hash_of(b"p"),
        migration_plan_digest: MigrationPlanDigest::hash_of(b"m"),
        activation_mode: ActivationMode::HotPatch,
        proposed_activation_tick: Tick(9000),
        readiness: readiness(),
    };
    let bytes = prepare.to_canonical_bytes().unwrap();
    assert_eq!(
        PrepareSceneChange::from_canonical_bytes(&bytes).unwrap(),
        prepare
    );

    let commit = CommitSceneChange {
        transaction_id: txn(2),
        realm_epoch: RealmEpoch(3),
        next_game_revision: GameRevisionId::hash_of(b"g2"),
        next_scene_sequence: SceneSequence(8),
        next_content_set: set,
        activation_tick: Tick(9060),
    };
    let bytes = commit.to_canonical_bytes().unwrap();
    assert_eq!(
        CommitSceneChange::from_canonical_bytes(&bytes).unwrap(),
        commit
    );

    let applied = SceneApplied {
        transaction_id: txn(2),
        realm_epoch: RealmEpoch(3),
        next_scene_sequence: SceneSequence(8),
    };
    let bytes = applied.to_canonical_bytes().unwrap();
    assert_eq!(SceneApplied::from_canonical_bytes(&bytes).unwrap(), applied);

    let prep_content = PrepareContentChange {
        transaction_id: txn(4),
        realm_epoch: RealmEpoch(3),
        next_set: ContentSetId::hash_of(b"s2"),
        required_delta: vec![aref(5, 0x51)],
        proposed_activation_tick: Tick(10_000),
        readiness: readiness(),
    };
    let bytes = prep_content.to_canonical_bytes().unwrap();
    assert_eq!(
        PrepareContentChange::from_canonical_bytes(&bytes).unwrap(),
        prep_content
    );

    let ready = ContentChangeReady {
        transaction_id: txn(4),
        realm_epoch: RealmEpoch(3),
        next_set: ContentSetId::hash_of(b"s2"),
        resolution: rres(5),
    };
    let bytes = ready.to_canonical_bytes().unwrap();
    assert_eq!(
        ContentChangeReady::from_canonical_bytes(&bytes).unwrap(),
        ready
    );

    let commit = CommitContentChange {
        transaction_id: txn(4),
        realm_epoch: RealmEpoch(3),
        next_set: ContentSetId::hash_of(b"s2"),
        dynamic_spawn_slots: vec![AssetSlot(4), AssetSlot(4)],
        activation_tick: Tick(10_060),
    };
    let bytes = commit.to_canonical_bytes().unwrap();
    assert_eq!(
        CommitContentChange::from_canonical_bytes(&bytes).unwrap(),
        commit
    );

    let prep_realm = PrepareRealm {
        transaction_id: txn(5),
        next_epoch: RealmEpoch(4),
        game_revision: game_rev,
        content_set: set,
        readiness: readiness(),
    };
    let bytes = prep_realm.to_canonical_bytes().unwrap();
    assert_eq!(PrepareRealm::from_canonical_bytes(&bytes).unwrap(), prep_realm);

    let commit_realm = CommitRealm {
        transaction_id: txn(5),
        next_epoch: RealmEpoch(4),
        game_revision: game_rev,
        content_set: set,
    };
    let bytes = commit_realm.to_canonical_bytes().unwrap();
    assert_eq!(CommitRealm::from_canonical_bytes(&bytes).unwrap(), commit_realm);

    let tuple = RoomContentTuple {
        realm_epoch: RealmEpoch(3),
        game_revision: game_rev,
        scene_sequence: SceneSequence(8),
        content_set: set,
    };
    let bytes = tuple.to_canonical_bytes().unwrap();
    assert_eq!(RoomContentTuple::from_canonical_bytes(&bytes).unwrap(), tuple);
}

#[test]
fn wrong_or_stale_resolution_cannot_ready_a_join() {
    let expected_ticket = ticket(1);
    let expected = rres(5);
    let ready = JoinContentReady {
        ticket: expected_ticket,
        resolution: expected,
    };
    ready.verify(&expected_ticket, &expected).unwrap();

    // Correct ticket, wrong aggregate digest: refused. A peer that resolved
    // against different variants can never be embodied.
    let wrong = JoinContentReady {
        ticket: expected_ticket,
        resolution: rres(6),
    };
    assert!(matches!(
        wrong.verify(&expected_ticket, &expected),
        Err(AssetDataError::Mismatch { what: "join ready resolution" })
    ));

    // Stale ticket from an abandoned preparation: refused even with the
    // aggregate the host currently expects.
    let stale = JoinContentReady {
        ticket: ticket(9),
        resolution: expected,
    };
    assert!(matches!(
        stale.verify(&expected_ticket, &expected),
        Err(AssetDataError::Mismatch { what: "join ready ticket" })
    ));

    // A cache fill or preference advance mid-join changes the expected
    // aggregate; the previously acknowledged digest no longer verifies.
    assert!(ready.verify(&expected_ticket, &rres(7)).is_err());
}

#[test]
fn multi_asset_realm_resolution_matches_the_lock_pin_table() {
    let lock = lock();
    let set = ContentSetId::hash_of(b"s");

    // The aggregate covers exactly the lock's pinned (base, set) pairs, in
    // the same canonical order, with this client's per-asset map digests.
    let mut resolution = RealmResolution {
        content_set: set,
        profile: ClientProfileDigest::hash_of(b"p"),
        entries: vec![
            RealmResolutionEntry {
                base: aref(4, 0x41),
                variant_set: vset_id(0xd4),
                resolved_map: rmap(0x24),
            },
            RealmResolutionEntry {
                base: aref(1, 0x11),
                variant_set: vset_id(0xd1),
                resolved_map: rmap(0x21),
            },
        ],
    };
    resolution.canonicalize();
    resolution.verify_against(&lock).unwrap();

    // Roundtrip and a stable digest for readiness.
    let bytes = resolution.to_canonical_bytes().unwrap();
    assert_eq!(
        RealmResolution::from_canonical_bytes(&bytes).unwrap(),
        resolution
    );
    let digest = resolution.digest().unwrap();

    let ready = JoinContentReady {
        ticket: ticket(1),
        resolution: digest,
    };
    ready.verify(&ticket(1), &digest).unwrap();

    // Skipping a pinned asset refuses: coverage is exact, not best-effort.
    let mut partial = resolution.clone();
    partial.entries.pop();
    assert!(matches!(
        partial.verify_against(&lock),
        Err(AssetDataError::Mismatch { what: "realm resolution coverage" })
    ));

    // Resolving against a set the release never pinned refuses.
    let mut wrong_set = resolution.clone();
    wrong_set.entries[0].variant_set = vset_id(0xdd);
    assert!(matches!(
        wrong_set.verify_against(&lock),
        Err(AssetDataError::Mismatch { what: "realm resolution pinned set" })
    ));

    // Two resolutions for one asset refuse outright.
    let mut doubled = resolution.clone();
    doubled.entries.push(RealmResolutionEntry {
        base: aref(4, 0x42),
        variant_set: vset_id(0xd4),
        resolved_map: rmap(0x25),
    });
    doubled.canonicalize();
    assert!(matches!(
        doubled.validate(),
        Err(AssetDataError::Duplicate { .. })
    ));
}

#[test]
fn empty_realm_resolution_readies_a_no_variant_realm() {
    // A lock that pins no variant sets still yields a canonical, digestable
    // aggregate, so plain realms can ready.
    let mut no_variants = lock();
    no_variants.variant_sets.clear();
    no_variants.validate().unwrap();

    let resolution = RealmResolution {
        content_set: ContentSetId::hash_of(b"s"),
        profile: ClientProfileDigest::hash_of(b"p"),
        entries: vec![],
    };
    resolution.verify_against(&no_variants).unwrap();
    let bytes = resolution.to_canonical_bytes().unwrap();
    assert_eq!(
        RealmResolution::from_canonical_bytes(&bytes).unwrap(),
        resolution
    );
    let digest = resolution.digest().unwrap();
    let ready = JoinContentReady {
        ticket: ticket(1),
        resolution: digest,
    };
    ready.verify(&ticket(1), &digest).unwrap();

    // The digest still binds content set and profile: a different profile
    // or set yields a different empty-aggregate digest.
    let other_profile = RealmResolution {
        content_set: ContentSetId::hash_of(b"s"),
        profile: ClientProfileDigest::hash_of(b"q"),
        entries: vec![],
    };
    assert_ne!(other_profile.digest().unwrap(), digest);

    // But an empty aggregate cannot ready a realm that DOES pin sets.
    assert!(resolution.verify_against(&lock()).is_err());
}

#[test]
fn document_kinds_do_not_cross_decode() {
    let manifest_bytes = weapon_manifest().to_canonical_bytes().unwrap();
    assert!(matches!(
        ContentLock::from_canonical_bytes(&manifest_bytes),
        Err(AssetDataError::BadDocKind { .. })
    ));
    let lock_bytes = lock().to_canonical_bytes().unwrap();
    assert!(matches!(
        AssetManifest::from_canonical_bytes(&lock_bytes),
        Err(AssetDataError::BadDocKind { .. })
    ));
}

#[test]
fn negative_zero_is_normalized_to_one_encoding() {
    let mut a = weapon_manifest();
    a.anchors[0].transform.pos = Vec3::new(0.0, -0.1, 0.2);
    let mut b = a.clone();
    b.anchors[0].transform.pos.x = -0.0;
    assert_eq!(
        a.to_canonical_bytes().unwrap(),
        b.to_canonical_bytes().unwrap()
    );
}
