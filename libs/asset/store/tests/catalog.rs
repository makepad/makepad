//! Catalog behavior: immutable revisions, candidate lifecycle, aliases,
//! game revision pin refs, and restart durability.

mod common;
use common::*;
use makepad_asset_store::{AssetServerCore, Budgets, CandidateState, ServerError};
use makepad_asset_data::{AssetAlias, AssetRevisionRef, BlobId};

#[test]
fn stage_requires_registered_asset_and_present_blobs() {
    let (_root, core) = open_core("admission");
    let glb = b"glb bytes".to_vec();
    let thumb = b"thumb bytes".to_vec();
    let manifest = prop_manifest(asset_id_n(1), &glb, &thumb);
    let bytes = manifest.to_canonical_bytes().unwrap();

    // Unregistered asset refuses.
    assert!(matches!(
        core.catalog().stage_asset_revision(&bytes, NOW).unwrap_err(),
        ServerError::NotFound { what: "asset for revision" }
    ));

    // Registered but blobs absent refuses (fail closed on dangling refs).
    core.catalog().register_asset(&asset_id_n(1), "rik2", NOW).unwrap();
    assert!(matches!(
        core.catalog().stage_asset_revision(&bytes, NOW).unwrap_err(),
        ServerError::NotFound { what: "asset file blob" }
    ));

    // With the GLB present the thumbnail is still missing.
    core.put_blob(&glb, NOW).unwrap();
    assert!(matches!(
        core.catalog().stage_asset_revision(&bytes, NOW).unwrap_err(),
        ServerError::NotFound { what: "asset thumbnail blob" }
    ));

    core.put_blob(&thumb, NOW).unwrap();
    let rev = core.catalog().stage_asset_revision(&bytes, NOW).unwrap();
    // The revision ID is the digest of the canonical manifest bytes.
    assert_eq!(rev, manifest.revision().unwrap());
    assert_eq!(
        core.catalog().asset_candidate_state(&asset_id_n(1), &rev).unwrap(),
        Some(CandidateState::Staged)
    );
    assert_eq!(
        core.catalog().asset_revision_manifest(&rev).unwrap().unwrap(),
        bytes
    );
}

#[test]
fn manifest_over_budget_refused() {
    let root = test_root("manifest_budget");
    let budgets = Budgets {
        max_manifest_bytes: 16,
        ..Budgets::default_v1()
    };
    let core = AssetServerCore::open(&root, budgets).unwrap();
    let manifest = prop_manifest(asset_id_n(1), b"g", b"t");
    let bytes = manifest.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.catalog().stage_asset_revision(&bytes, NOW).unwrap_err(),
        ServerError::OverBudget { what: "asset manifest bytes", .. }
    ));
}

#[test]
fn dependency_revisions_must_exist() {
    let (_root, core) = open_core("deps");
    let glb = b"glb".to_vec();
    let thumb = b"thumb".to_vec();
    core.put_blob(&glb, NOW).unwrap();
    core.put_blob(&thumb, NOW).unwrap();
    core.catalog().register_asset(&asset_id_n(1), "rik2", NOW).unwrap();
    let mut manifest = prop_manifest(asset_id_n(1), &glb, &thumb);
    manifest.dependencies = vec![AssetRevisionRef {
        asset_id: asset_id_n(9),
        revision: makepad_asset_data::AssetRevisionId::from_bytes([9; 32]),
    }];
    let bytes = manifest.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.catalog().stage_asset_revision(&bytes, NOW).unwrap_err(),
        ServerError::NotFound { what: "asset dependency revision" }
    ));
}

#[test]
fn candidate_lifecycle_and_alias_rules() {
    let (_root, core) = open_core("lifecycle");
    let glb = b"lifecycle glb".to_vec();
    let thumb = b"lifecycle thumb".to_vec();
    core.put_blob(&glb, NOW).unwrap();
    core.put_blob(&thumb, NOW).unwrap();
    let id = asset_id_n(1);
    core.catalog().register_asset(&id, "rik2", NOW).unwrap();
    let manifest = prop_manifest(id, &glb, &thumb);
    let bytes = manifest.to_canonical_bytes().unwrap();
    let rev = core.catalog().stage_asset_revision(&bytes, NOW).unwrap();
    let target = AssetRevisionRef { asset_id: id, revision: rev };
    let alias = "rik2/props/box".parse().unwrap();

    // Re-staging identical content while staged is idempotent.
    assert_eq!(core.catalog().stage_asset_revision(&bytes, NOW + 1).unwrap(), rev);

    // Aliases may only point at PUBLISHED revisions.
    assert!(matches!(
        core.catalog().set_asset_alias(&alias, &target, NOW).unwrap_err(),
        ServerError::InvalidState { what: "alias target", .. }
    ));

    core.catalog().publish_asset(&id, &rev, NOW + 2).unwrap();
    // Publishing again is an idempotent repeat.
    core.catalog().publish_asset(&id, &rev, NOW + 3).unwrap();
    assert_eq!(
        core.catalog().asset_candidate_state(&id, &rev).unwrap(),
        Some(CandidateState::Published)
    );

    // Namespace mismatch between alias and asset refuses.
    let foreign_alias = "other/props/box".parse().unwrap();
    assert!(matches!(
        core.catalog().set_asset_alias(&foreign_alias, &target, NOW).unwrap_err(),
        ServerError::Conflict { what: "alias namespace" }
    ));

    core.catalog().set_asset_alias(&alias, &target, NOW + 4).unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(&alias).unwrap(), Some(target));

    // Re-admitting content that already published is refused.
    assert!(matches!(
        core.catalog().stage_asset_revision(&bytes, NOW + 5).unwrap_err(),
        ServerError::InvalidState { what: "candidate", state: "published" }
    ));

    // Alias head retargets to a second published revision.
    let glb2 = b"lifecycle glb v2".to_vec();
    core.put_blob(&glb2, NOW + 6).unwrap();
    let manifest2 = prop_manifest(id, &glb2, &thumb);
    let bytes2 = manifest2.to_canonical_bytes().unwrap();
    let rev2 = core.catalog().stage_asset_revision(&bytes2, NOW + 6).unwrap();
    assert_ne!(rev, rev2);
    core.catalog().publish_asset(&id, &rev2, NOW + 7).unwrap();
    let target2 = AssetRevisionRef { asset_id: id, revision: rev2 };
    core.catalog().set_asset_alias(&alias, &target2, NOW + 8).unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(&alias).unwrap(), Some(target2));

    // Quarantine is reachable from published and is terminal; the alias head
    // pointing at the pulled revision is torn down in the same transaction.
    core.catalog().quarantine_asset(&id, &rev2, NOW + 9).unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(&alias).unwrap(), None);
    core.catalog().quarantine_asset(&id, &rev2, NOW + 10).unwrap(); // idempotent
    assert!(matches!(
        core.catalog().publish_asset(&id, &rev2, NOW + 11).unwrap_err(),
        ServerError::InvalidState { what: "candidate transition", state: "quarantined" }
    ));
    assert!(matches!(
        core.catalog().stage_asset_revision(&bytes2, NOW + 12).unwrap_err(),
        ServerError::InvalidState { what: "candidate", state: "quarantined" }
    ));
    // A quarantined revision can no longer become an alias target.
    assert!(matches!(
        core.catalog().set_asset_alias(&alias, &target2, NOW + 13).unwrap_err(),
        ServerError::InvalidState { what: "alias target", .. }
    ));
}

#[test]
fn quarantine_tears_down_every_alias_head_on_the_revision() {
    let (_root, core) = open_core("quarantine_alias");
    let glb1 = b"qa glb v1".to_vec();
    let glb2 = b"qa glb v2".to_vec();
    let thumb = b"qa thumb".to_vec();
    for blob in [&glb1, &glb2, &thumb] {
        core.put_blob(blob, NOW).unwrap();
    }
    let id = asset_id_n(1);
    core.catalog().register_asset(&id, "rik2", NOW).unwrap();
    let bytes1 = prop_manifest(id, &glb1, &thumb).to_canonical_bytes().unwrap();
    let bytes2 = prop_manifest(id, &glb2, &thumb).to_canonical_bytes().unwrap();
    let rev1 = core.catalog().stage_asset_revision(&bytes1, NOW).unwrap();
    let rev2 = core.catalog().stage_asset_revision(&bytes2, NOW).unwrap();
    core.catalog().publish_asset(&id, &rev1, NOW).unwrap();
    core.catalog().publish_asset(&id, &rev2, NOW).unwrap();
    let t1 = AssetRevisionRef { asset_id: id, revision: rev1 };
    let t2 = AssetRevisionRef { asset_id: id, revision: rev2 };
    let a1: AssetAlias = "rik2/props/one".parse().unwrap();
    let a2: AssetAlias = "rik2/props/two".parse().unwrap();
    let a3: AssetAlias = "rik2/props/three".parse().unwrap();
    core.catalog().set_asset_alias(&a1, &t1, NOW).unwrap();
    core.catalog().set_asset_alias(&a2, &t1, NOW).unwrap();
    core.catalog().set_asset_alias(&a3, &t2, NOW).unwrap();

    // Quarantining rev1 drops BOTH heads pointing at it and only those.
    core.catalog().quarantine_asset(&id, &rev1, NOW + 1).unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(&a1).unwrap(), None);
    assert_eq!(core.catalog().resolve_asset_alias(&a2).unwrap(), None);
    assert_eq!(core.catalog().resolve_asset_alias(&a3).unwrap(), Some(t2));

    // clear_asset_alias is an idempotent head removal; a published target
    // can be re-pointed afterwards.
    assert!(core.catalog().clear_asset_alias(&a3).unwrap());
    assert!(!core.catalog().clear_asset_alias(&a3).unwrap());
    assert_eq!(core.catalog().resolve_asset_alias(&a3).unwrap(), None);
    core.catalog().set_asset_alias(&a3, &t2, NOW + 2).unwrap();
    assert_eq!(core.catalog().resolve_asset_alias(&a3).unwrap(), Some(t2));
}

#[test]
fn asset_registration_conflicts_refuse() {
    let (_root, core) = open_core("register");
    core.catalog().register_asset(&asset_id_n(1), "rik2", NOW).unwrap();
    // Idempotent same-namespace repeat.
    core.catalog().register_asset(&asset_id_n(1), "rik2", NOW + 1).unwrap();
    assert!(matches!(
        core.catalog().register_asset(&asset_id_n(1), "other", NOW + 2).unwrap_err(),
        ServerError::Conflict { what: "asset namespace" }
    ));
    assert!(matches!(
        core.catalog().register_asset(&asset_id_n(2), "Bad Namespace", NOW).unwrap_err(),
        ServerError::InvalidInput { .. }
    ));
}

#[test]
fn game_revision_pins_published_assets_only() {
    let (_root, core) = open_core("game");
    let now = NOW;
    let (id_a, rev_a) = publish_prop(&core, "rik2", 1, b"glb a", b"thumb a", now);
    // Second asset stays staged only.
    let glb_b = b"glb b".to_vec();
    let thumb_b = b"thumb b".to_vec();
    core.put_blob(&glb_b, now).unwrap();
    core.put_blob(&thumb_b, now).unwrap();
    core.catalog().register_asset(&asset_id_n(2), "rik2", now).unwrap();
    let manifest_b = prop_manifest(asset_id_n(2), &glb_b, &thumb_b);
    let rev_b = core
        .catalog()
        .stage_asset_revision(&manifest_b.to_canonical_bytes().unwrap(), now)
        .unwrap();

    let gid = game_id_n(7);
    core.catalog().register_game(&gid, "rik2", now).unwrap();
    let ref_a = AssetRevisionRef { asset_id: id_a, revision: rev_a };
    let ref_b = AssetRevisionRef { asset_id: asset_id_n(2), revision: rev_b };
    let lock = lock_for(gid, &[("rik2/props/a", ref_a), ("rik2/props/b", ref_b)]);
    let splash = b"splash source".to_vec();
    let toml = b"manifest toml".to_vec();
    let gthumb = b"game thumb".to_vec();
    for blob in [&splash, &toml, &lock, &gthumb] {
        core.put_blob(blob, now).unwrap();
    }
    let gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    let gm_bytes = gm.to_canonical_bytes().unwrap();

    // One pinned revision is not published -> refuse.
    assert!(matches!(
        core.catalog().stage_game_revision(&gm_bytes, &lock, now).unwrap_err(),
        ServerError::InvalidState { what: "pinned asset revision", .. }
    ));

    core.catalog().publish_asset(&asset_id_n(2), &rev_b, now).unwrap();
    let grev = core.catalog().stage_game_revision(&gm_bytes, &lock, now).unwrap();
    assert_eq!(grev, gm.revision().unwrap());

    // The refs table pins exactly the closure.
    let mut expected = vec![ref_a, ref_b];
    expected.sort();
    assert_eq!(core.catalog().game_revision_refs(&grev).unwrap(), expected);

    // Game alias requires a published game revision.
    let galias = "rik2/games/test-game".parse().unwrap();
    assert!(matches!(
        core.catalog().set_game_alias(&galias, &gid, &grev, now).unwrap_err(),
        ServerError::InvalidState { what: "game alias target", .. }
    ));
    core.catalog().publish_game(&gid, &grev, now).unwrap();
    core.catalog().set_game_alias(&galias, &gid, &grev, now).unwrap();
    assert_eq!(
        core.catalog().resolve_game_alias(&galias).unwrap(),
        Some((gid, grev))
    );

    // Game alias heads clear idempotently and re-point while published.
    assert!(core.catalog().clear_game_alias(&galias).unwrap());
    assert!(!core.catalog().clear_game_alias(&galias).unwrap());
    core.catalog().set_game_alias(&galias, &gid, &grev, now).unwrap();

    // Quarantine the game revision: terminal there too, and the alias head
    // pointing at the pulled revision is gone.
    core.catalog().quarantine_game(&gid, &grev, now).unwrap();
    assert_eq!(core.catalog().resolve_game_alias(&galias).unwrap(), None);
    assert!(matches!(
        core.catalog().publish_game(&gid, &grev, now).unwrap_err(),
        ServerError::InvalidState { .. }
    ));
}

#[test]
fn game_lock_binding_is_verified() {
    let (_root, core) = open_core("game_lock");
    let now = NOW;
    let (id_a, rev_a) = publish_prop(&core, "rik2", 1, b"glb a", b"thumb a", now);
    let gid = game_id_n(7);
    core.catalog().register_game(&gid, "rik2", now).unwrap();
    let ref_a = AssetRevisionRef { asset_id: id_a, revision: rev_a };
    let lock = lock_for(gid, &[("rik2/props/a", ref_a)]);
    let other_lock = lock_for(game_id_n(8), &[("rik2/props/a", ref_a)]);
    let splash = b"s".to_vec();
    let toml = b"t".to_vec();
    let gthumb = b"th".to_vec();
    for blob in [&splash, &toml, &lock, &other_lock, &gthumb] {
        core.put_blob(blob, now).unwrap();
    }

    // Manifest names one lock digest but different lock bytes arrive.
    let gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    let gm_bytes = gm.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.catalog().stage_game_revision(&gm_bytes, &other_lock, now).unwrap_err(),
        ServerError::DigestMismatch { what: "game lock blob", .. }
    ));

    // Lock whose game_id disagrees with the manifest refuses.
    let gm_cross = game_manifest(gid, &splash, &toml, &other_lock, &gthumb);
    let gm_cross_bytes = gm_cross.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.catalog().stage_game_revision(&gm_cross_bytes, &other_lock, now).unwrap_err(),
        ServerError::Conflict { what: "lock game_id" }
    ));

    // Missing lock blob record refuses even when digests agree.
    let unrecorded_lock = lock_for(gid, &[("rik2/props/x", ref_a)]);
    let gm2 = game_manifest(gid, &splash, &toml, &unrecorded_lock, &gthumb);
    let gm2_bytes = gm2.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.catalog().stage_game_revision(&gm2_bytes, &unrecorded_lock, now).unwrap_err(),
        ServerError::NotFound { what: "game lock blob" }
    ));
}

#[test]
fn asset_declared_byte_lengths_must_match_recorded_sizes() {
    let (_root, core) = open_core("size_lies");
    let glb = b"glb payload".to_vec();
    let thumb = b"thumb payload".to_vec();
    core.put_blob(&glb, NOW).unwrap();
    core.put_blob(&thumb, NOW).unwrap();
    core.catalog().register_asset(&asset_id_n(1), "rik2", NOW).unwrap();

    // File length lie. metrics.total_bytes tracks the lie so the manifest
    // stays contract-valid: the SERVER's size-vs-CAS check must refuse it.
    let mut lying = prop_manifest(asset_id_n(1), &glb, &thumb);
    lying.files[0].byte_len += 1;
    lying.metrics.total_bytes += 1;
    let err = core
        .catalog()
        .stage_asset_revision(&lying.to_canonical_bytes().unwrap(), NOW)
        .unwrap_err();
    assert!(matches!(err, ServerError::SizeMismatch { what: "asset file blob size", .. }), "{err}");

    // Thumbnail length lie.
    let mut lying = prop_manifest(asset_id_n(1), &glb, &thumb);
    lying.thumbnail.as_mut().unwrap().byte_len -= 1;
    lying.metrics.total_bytes -= 1;
    let err = core
        .catalog()
        .stage_asset_revision(&lying.to_canonical_bytes().unwrap(), NOW)
        .unwrap_err();
    assert!(matches!(err, ServerError::SizeMismatch { what: "asset thumbnail blob size", .. }), "{err}");

    // The truthful manifest still stages.
    let honest = prop_manifest(asset_id_n(1), &glb, &thumb);
    core.catalog()
        .stage_asset_revision(&honest.to_canonical_bytes().unwrap(), NOW)
        .unwrap();
}

#[test]
fn game_declared_byte_lengths_must_match_recorded_sizes() {
    let (_root, core) = open_core("game_size_lies");
    let now = NOW;
    let (id_a, rev_a) = publish_prop(&core, "rik2", 1, b"glb a", b"thumb a", now);
    let gid = game_id_n(7);
    core.catalog().register_game(&gid, "rik2", now).unwrap();
    let ref_a = AssetRevisionRef { asset_id: id_a, revision: rev_a };
    let lock = lock_for(gid, &[("rik2/props/a", ref_a)]);
    let splash = b"splash source".to_vec();
    let toml = b"manifest toml".to_vec();
    let gthumb = b"game thumb".to_vec();
    for blob in [&splash, &toml, &lock, &gthumb] {
        core.put_blob(blob, now).unwrap();
    }

    // Splash length lie.
    let mut gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    gm.splash_byte_len += 1;
    let err = core
        .catalog()
        .stage_game_revision(&gm.to_canonical_bytes().unwrap(), &lock, now)
        .unwrap_err();
    assert!(matches!(err, ServerError::SizeMismatch { what: "game splash blob size", .. }), "{err}");

    // Thumbnail length lie.
    let mut gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    gm.thumbnail.byte_len += 1;
    let err = core
        .catalog()
        .stage_game_revision(&gm.to_canonical_bytes().unwrap(), &lock, now)
        .unwrap_err();
    assert!(matches!(err, ServerError::SizeMismatch { what: "game thumbnail blob size", .. }), "{err}");

    // Lock record size lie: this lock's digest carries a corrupt catalog
    // size, so even the true bytes (matching digest) must refuse admission.
    let lock2 = lock_for(gid, &[("rik2/props/b", ref_a)]);
    core.catalog()
        .record_blob(&BlobId::hash_of(&lock2), lock2.len() as u64 + 5, now)
        .unwrap();
    let gm2 = game_manifest(gid, &splash, &toml, &lock2, &gthumb);
    let err = core
        .catalog()
        .stage_game_revision(&gm2.to_canonical_bytes().unwrap(), &lock2, now)
        .unwrap_err();
    assert!(matches!(err, ServerError::SizeMismatch { what: "game lock blob size", .. }), "{err}");

    // The truthful manifest still stages.
    let gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    core.catalog()
        .stage_game_revision(&gm.to_canonical_bytes().unwrap(), &lock, now)
        .unwrap();
}

#[test]
fn game_catalog_snapshot_pin_is_checked() {
    let (_root, core) = open_core("game_snapshot");
    let now = NOW;
    let (id_a, rev_a) = publish_prop(&core, "rik2", 1, b"glb a", b"thumb a", now);
    let gid = game_id_n(7);
    core.catalog().register_game(&gid, "rik2", now).unwrap();
    let ref_a = AssetRevisionRef { asset_id: id_a, revision: rev_a };
    let lock = lock_for(gid, &[("rik2/props/a", ref_a)]);
    let splash = b"splash source".to_vec();
    let toml = b"manifest toml".to_vec();
    let gthumb = b"game thumb".to_vec();
    for blob in [&splash, &toml, &lock, &gthumb] {
        core.put_blob(blob, now).unwrap();
    }
    let snapshot = b"catalog snapshot document".to_vec();

    // Pinned but unrecorded snapshot refuses.
    let mut gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    gm.catalog_snapshot = Some(BlobId::hash_of(&snapshot));
    let err = core
        .catalog()
        .stage_game_revision(&gm.to_canonical_bytes().unwrap(), &lock, now)
        .unwrap_err();
    assert!(matches!(err, ServerError::NotFound { what: "game catalog snapshot blob" }), "{err}");

    // Empty snapshot refuses.
    let empty = BlobId::hash_of(b"");
    core.catalog().record_blob(&empty, 0, now).unwrap();
    let mut gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    gm.catalog_snapshot = Some(empty);
    let err = core
        .catalog()
        .stage_game_revision(&gm.to_canonical_bytes().unwrap(), &lock, now)
        .unwrap_err();
    assert!(matches!(err, ServerError::InvalidInput { what: "game catalog snapshot empty" }), "{err}");

    // Snapshot beyond the canonical-document bound refuses; the recorded
    // size is what admission judges, so record an over-bound size directly.
    let big = BlobId::hash_of(b"pretend huge snapshot");
    core.catalog().record_blob(&big, 1024 * 1024 + 1, now).unwrap();
    let mut gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    gm.catalog_snapshot = Some(big);
    let err = core
        .catalog()
        .stage_game_revision(&gm.to_canonical_bytes().unwrap(), &lock, now)
        .unwrap_err();
    assert!(matches!(err, ServerError::OverBudget { what: "game catalog snapshot bytes", .. }), "{err}");

    // A real recorded snapshot stages, and the revision is the manifest hash.
    core.put_blob(&snapshot, now).unwrap();
    let mut gm = game_manifest(gid, &splash, &toml, &lock, &gthumb);
    gm.catalog_snapshot = Some(BlobId::hash_of(&snapshot));
    let grev = core
        .catalog()
        .stage_game_revision(&gm.to_canonical_bytes().unwrap(), &lock, now)
        .unwrap();
    assert_eq!(grev, gm.revision().unwrap());
}

#[test]
fn catalog_survives_reopen() {
    let root = test_root("durable");
    let alias: makepad_asset_data::AssetAlias = "rik2/props/box".parse().unwrap();
    let (id, rev);
    {
        let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
        let pair = publish_prop(&core, "rik2", 1, b"durable glb", b"durable thumb", NOW);
        id = pair.0;
        rev = pair.1;
        core.catalog()
            .set_asset_alias(&alias, &AssetRevisionRef { asset_id: id, revision: rev }, NOW)
            .unwrap();
    }
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    core.recover(NOW + 100).unwrap();
    assert_eq!(
        core.catalog().resolve_asset_alias(&alias).unwrap(),
        Some(AssetRevisionRef { asset_id: id, revision: rev })
    );
    assert_eq!(
        core.catalog().asset_candidate_state(&id, &rev).unwrap(),
        Some(CandidateState::Published)
    );
    let manifest_bytes = core.catalog().asset_revision_manifest(&rev).unwrap().unwrap();
    assert_eq!(BlobId::hash_of(&manifest_bytes).as_bytes(), rev.as_bytes());
}
