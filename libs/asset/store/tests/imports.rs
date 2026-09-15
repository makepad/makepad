//! Deterministic external-pack import: approved sources, the atomic import
//! transaction, idempotent replay, cross-server determinism, and fail-closed
//! refusals that leave no partial state behind.

mod common;
use common::*;
use makepad_asset_store::*;
use makepad_asset_data::*;

#[test]
fn source_registration_is_idempotent_and_approval_is_not_rewritable() {
    let (_root, core) = open_core("import_sources");
    let bytes = kenney_collection().to_canonical_bytes().unwrap();
    let digest = core.imports().register_source(&bytes, NOW).unwrap();
    // Same bytes: idempotent.
    assert_eq!(core.imports().register_source(&bytes, NOW + 1).unwrap(), digest);
    assert_eq!(core.imports().sources().unwrap(), vec![bytes.clone()]);
    assert_eq!(core.imports().source_manifest("kenney").unwrap(), Some(bytes));

    // A DIFFERENT collection under the same id refuses: approval is explicit.
    let mut changed = kenney_collection();
    changed.title = "Kenney assets, rebranded".into();
    let changed_bytes = changed.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.imports().register_source(&changed_bytes, NOW + 2),
        Err(ServerError::Conflict { what: "source collection digest" })
    ));

    // Garbage bytes refuse through the content contract.
    assert!(matches!(
        core.imports().register_source(b"junk", NOW),
        Err(ServerError::Content(_))
    ));
}

#[test]
fn import_publishes_assets_aliases_and_entries_atomically() {
    let (_root, core) = open_core("import_run");
    let generation_before = {
        // Force schema/state creation before the raw side-channel read.
        core.catalog().register_asset(&asset_id_n(9), "warmup", NOW).unwrap();
        core.search().generation().unwrap()
    };
    let report = run_kenney_import(&core, "1.0", NOW);
    assert!(report.created);
    assert_eq!(report.entries.len(), 2);

    let manifest = kenney_pack("1.0");
    let import_rev = manifest.revision().unwrap();
    assert_eq!(report.import_revision, import_rev);

    // Every entry: registered in the source namespace, published, aliased.
    for (asset, entry) in manifest.assets.iter().zip(report.entries.iter()) {
        assert_eq!(entry.key, asset.key.as_str());
        assert_eq!(entry.asset_id, manifest.asset_id_for(&asset.key));
        assert_eq!(
            core.catalog().asset_namespace(&entry.asset_id).unwrap().as_deref(),
            Some("kenney")
        );
        assert_eq!(
            core.catalog()
                .asset_candidate_state(&entry.asset_id, &entry.revision)
                .unwrap(),
            Some(CandidateState::Published)
        );
        let alias = manifest.alias_for(&asset.key).unwrap();
        assert_eq!(
            core.catalog().resolve_asset_alias(&alias).unwrap(),
            Some(AssetRevisionRef {
                asset_id: entry.asset_id,
                revision: entry.revision,
            })
        );
        // The stored manifest pins the exact import lineage.
        let stored = core
            .catalog()
            .asset_revision_manifest(&entry.revision)
            .unwrap()
            .unwrap();
        let decoded = AssetManifest::from_canonical_bytes(&stored).unwrap();
        let prov = decoded.provenance.unwrap();
        assert_eq!(prov.generator, "import");
        assert_eq!(prov.params_digest, Some(*import_rev.as_bytes()));
        assert_eq!(decoded.rights.license, "CC0-1.0");
        assert!(decoded.rights.credits.contains("Kenney"));
    }
    // Entry rows are recorded and readable.
    assert_eq!(core.imports().entries(&import_rev).unwrap(), report.entries);
    // Alias writes went through the search choke point: generation advanced.
    assert!(core.search().generation().unwrap() > generation_before);
}

#[test]
fn import_replay_is_idempotent_and_two_servers_agree() {
    let (_root_a, a) = open_core("import_det_a");
    let (_root_b, b) = open_core("import_det_b");
    let ra1 = run_kenney_import(&a, "1.0", NOW);
    let ra2 = run_kenney_import(&a, "1.0", NOW + 500);
    let rb = run_kenney_import(&b, "1.0", NOW + 999);

    assert!(ra1.created);
    // Replay on the same server: recorded result, no new work.
    assert!(!ra2.created);
    assert_eq!(ra1.import_revision, ra2.import_revision);
    assert_eq!(ra1.entries, ra2.entries);
    // A clean second server produces byte-identical identities.
    assert!(rb.created);
    assert_eq!(ra1.import_revision, rb.import_revision);
    assert_eq!(ra1.entries, rb.entries);
    for entry in &ra1.entries {
        assert_eq!(
            a.catalog().asset_revision_manifest(&entry.revision).unwrap(),
            b.catalog().asset_revision_manifest(&entry.revision).unwrap(),
        );
    }
}

#[test]
fn a_new_pack_version_revises_the_same_assets() {
    let (_root, core) = open_core("import_version");
    let v1 = run_kenney_import(&core, "1.0", NOW);

    let mut pack2 = kenney_pack("2.0");
    // v2 ships a bigger watchtower mesh.
    let new_glb = b"KENNEY-WATCHTOWER-GLB-v2-BIGGER";
    core.put_blob(new_glb, NOW + 1).unwrap();
    let watchtower = pack2
        .assets
        .iter_mut()
        .find(|a| a.key.as_str() == "models/watchtower")
        .unwrap();
    watchtower.files[0].file.blob = BlobId::hash_of(new_glb);
    watchtower.files[0].file.byte_len = new_glb.len() as u64;
    watchtower.metrics.total_bytes =
        (new_glb.len() + PACK_COLLIDER.len() + PACK_PREVIEW.len()) as u64;
    let v2 = core
        .imports()
        .run_import(&pack2.to_canonical_bytes().unwrap(), NOW + 2)
        .unwrap();

    assert_ne!(v1.import_revision, v2.import_revision);
    for (e1, e2) in v1.entries.iter().zip(v2.entries.iter()) {
        // Same stable asset identity across versions; every entry gains a
        // NEW revision because its provenance pins the new import revision
        // (a changed pack creates new asset revisions — the plan's law).
        assert_eq!(e1.asset_id, e2.asset_id);
        assert_ne!(e1.revision, e2.revision);
        // Prior revisions are never edited or unpublished by a later import.
        assert_eq!(
            core.catalog()
                .asset_candidate_state(&e1.asset_id, &e1.revision)
                .unwrap(),
            Some(CandidateState::Published)
        );
    }
    // Unchanged source bytes deduped at the blob layer: the v2 hull texture
    // manifest names the exact v1 blob.
    let stored = core
        .catalog()
        .asset_revision_manifest(&v2.entries[1].revision)
        .unwrap()
        .unwrap();
    let decoded = AssetManifest::from_canonical_bytes(&stored).unwrap();
    assert_eq!(decoded.files[0].blob, BlobId::hash_of(PACK_TEXTURE));
    // The alias head advanced to the new revision.
    let alias = pack2.alias_for(&pack2.assets[0].key).unwrap();
    assert_eq!(
        core.catalog().resolve_asset_alias(&alias).unwrap().unwrap().revision,
        v2.entries[0].revision
    );
}

#[test]
fn failed_import_publishes_nothing() {
    let (_root, core) = open_core("import_atomic");
    // Register the source but DO NOT upload the texture blob: the second
    // asset's admission must fail after the first asset already staged.
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW] {
        core.put_blob(bytes, NOW).unwrap();
    }
    let collection_bytes = kenney_collection().to_canonical_bytes().unwrap();
    core.imports().register_source(&collection_bytes, NOW).unwrap();
    let manifest = kenney_pack("1.0");
    let manifest_bytes = manifest.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.imports().run_import(&manifest_bytes, NOW),
        Err(ServerError::NotFound { what: "asset file blob" })
    ));

    // NOTHING is visible: no import row, no entries, no assets, no aliases.
    let import_rev = manifest.revision().unwrap();
    assert_eq!(core.imports().import_manifest_bytes(&import_rev).unwrap(), None);
    assert_eq!(core.imports().entries(&import_rev).unwrap(), vec![]);
    for asset in &manifest.assets {
        let id = manifest.asset_id_for(&asset.key);
        assert_eq!(core.catalog().asset_namespace(&id).unwrap(), None);
        let alias = manifest.alias_for(&asset.key).unwrap();
        assert_eq!(core.catalog().resolve_asset_alias(&alias).unwrap(), None);
    }

    // After the missing blob arrives, the same manifest imports cleanly.
    core.put_blob(PACK_TEXTURE, NOW + 1).unwrap();
    let report = core.imports().run_import(&manifest_bytes, NOW + 1).unwrap();
    assert!(report.created);
}

#[test]
fn import_refuses_unapproved_or_divergent_sources() {
    let (_root, core) = open_core("import_sources_gate");
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        core.put_blob(bytes, NOW).unwrap();
    }
    let manifest_bytes = kenney_pack("1.0").to_canonical_bytes().unwrap();
    // No registered source at all.
    assert!(matches!(
        core.imports().run_import(&manifest_bytes, NOW),
        Err(ServerError::NotFound { what: "source collection" })
    ));
    // A registered collection with the same id but different content: the
    // manifest's pinned digest no longer matches the approval.
    let mut other = kenney_collection();
    other.title = "Different approval".into();
    core.imports()
        .register_source(&other.to_canonical_bytes().unwrap(), NOW)
        .unwrap();
    assert!(matches!(
        core.imports().run_import(&manifest_bytes, NOW),
        Err(ServerError::Conflict { what: "source collection digest" })
    ));
}

#[test]
fn import_never_resurrects_quarantined_content() {
    let (_root, core) = open_core("import_quarantine");
    // Compute the exact revision the import WOULD produce for the
    // watchtower (pure content math), publish it through the ordinary
    // catalog path, then quarantine it — before any import has run.
    let manifest = kenney_pack("1.0");
    let import_rev = manifest.revision().unwrap();
    let watchtower = &manifest.assets[0];
    let produced = manifest
        .asset_manifest_for(watchtower, &import_rev)
        .unwrap();
    let produced_bytes = produced.to_canonical_bytes().unwrap();
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        core.put_blob(bytes, NOW).unwrap();
    }
    core.catalog()
        .register_asset(&produced.asset_id, "kenney", NOW)
        .unwrap();
    let revision = core
        .catalog()
        .stage_asset_revision(&produced_bytes, NOW)
        .unwrap();
    core.catalog().publish_asset(&produced.asset_id, &revision, NOW).unwrap();
    core.catalog()
        .quarantine_asset(&produced.asset_id, &revision, NOW + 1)
        .unwrap();

    // The import now refuses whole rather than resurrecting pulled content —
    // and atomicity means the OTHER entry publishes nothing either.
    core.imports()
        .register_source(&kenney_collection().to_canonical_bytes().unwrap(), NOW + 2)
        .unwrap();
    let manifest_bytes = manifest.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.imports().run_import(&manifest_bytes, NOW + 2),
        Err(ServerError::InvalidState { what: "imported revision", state: "quarantined" })
    ));
    assert_eq!(core.imports().import_manifest_bytes(&import_rev).unwrap(), None);
    let texture_id = manifest.asset_id_for(&manifest.assets[1].key);
    assert_eq!(core.catalog().asset_namespace(&texture_id).unwrap(), None);
}

#[test]
fn registered_source_terms_are_authoritative() {
    let (_root, core) = open_core("import_rights_gate");
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        core.put_blob(bytes, NOW).unwrap();
    }
    // Register a CC-BY-4.0 attribution-required source.
    let ccby = collection_with_terms("attributed", cc_by_terms());
    core.imports()
        .register_source(&ccby.to_canonical_bytes().unwrap(), NOW)
        .unwrap();

    // A manifest that claims CC0 for the CC-BY source refuses: license
    // laundering is a Conflict, not a warning.
    let mut laundered = pack_with_terms(&ccby);
    laundered.rights = cc0_rights("Example Author", "https://example.com/pack");
    let laundered_bytes = laundered.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.imports().run_import(&laundered_bytes, NOW),
        Err(ServerError::Conflict { what: "import rights vs registered source" })
    ));

    // Dropping the credits line refuses the same way (and could not even
    // encode under attribution-required policy — try weakening to Allowed
    // AND dropping credits, which encodes but diverges from the approval).
    let mut uncredited = pack_with_terms(&ccby);
    uncredited.rights.credits = String::new();
    uncredited.rights.redistribution = Redistribution::Allowed;
    uncredited.rights.derivatives = DerivativePolicy::Allowed;
    let uncredited_bytes = uncredited.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.imports().run_import(&uncredited_bytes, NOW),
        Err(ServerError::Conflict { what: "import rights vs registered source" })
    ));

    // Unpinning the terms digest refuses too: the pinned terms ARE the
    // approval.
    let mut unpinned = pack_with_terms(&ccby);
    unpinned.rights.terms_digest = None;
    let unpinned_bytes = unpinned.to_canonical_bytes().unwrap();
    assert!(matches!(
        core.imports().run_import(&unpinned_bytes, NOW),
        Err(ServerError::Conflict { what: "import rights vs registered source" })
    ));

    // The exact registered terms import cleanly, and every produced manifest
    // carries them verbatim: identifier+revision, terms digest/URL, credits,
    // upstream source, archive digest, and both policies survive.
    let exact = pack_with_terms(&ccby);
    let report = core
        .imports()
        .run_import(&exact.to_canonical_bytes().unwrap(), NOW)
        .unwrap();
    for entry in &report.entries {
        let stored = core
            .catalog()
            .asset_revision_manifest(&entry.revision)
            .unwrap()
            .unwrap();
        let decoded = AssetManifest::from_canonical_bytes(&stored).unwrap();
        assert_eq!(decoded.rights, cc_by_terms());
    }
}

#[test]
fn import_budget_is_enforced() {
    let root = test_root("import_budget");
    let mut budgets = Budgets::default_v1();
    budgets.max_import_assets = 1;
    let core = AssetServerCore::open(&root, budgets).unwrap();
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        core.put_blob(bytes, NOW).unwrap();
    }
    core.imports()
        .register_source(&kenney_collection().to_canonical_bytes().unwrap(), NOW)
        .unwrap();
    assert!(matches!(
        core.imports()
            .run_import(&kenney_pack("1.0").to_canonical_bytes().unwrap(), NOW),
        Err(ServerError::OverBudget { what: "import assets", .. })
    ));
}
