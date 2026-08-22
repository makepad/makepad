//! Import and derived-variant contract fixtures: deterministic goldens,
//! roundtrips, and fail-closed behavior for the pack-import and processing
//! document kinds added under schema version 1.
//!
//! The golden digests are frozen: a change means the encoding changed for
//! every peer — that is a schema bump, not a test update.

mod common;

use common::*;
use makepad_asset_data::*;

/// Frozen golden digests for the import/derived document kinds, in order:
/// source collection, import revision, watchtower asset id, watchtower
/// revision, LOD recipe, LOD variant, variant set, resolved map.
const GOLDENS: [&str; 8] = [
    "scol_44582f1165340930de7e2fae219368cfe76881a4e3450add375635d588eb2572",
    "irev_c1ca2284ec180d5b1a9d4baded72c3a0dfdc0029416f5b65967b05359b6f643c",
    "ast_chopwwsh6hze3ik23elgiamm24",
    "arev_d42a8fda380a647b1370dc9f9aebc0230a7647eca69ec3f8ec6112c66ae0a11a",
    "recp_0bab90f146b206a3cfa0a924d524684a39b2384ceeca25ad5a3fb8da6571c2f4",
    "dvar_eea6fdb11518c5816706698bcecc83f084744e61baabfe7f40ce2b26399b8a65",
    "vset_aa441791256de2141ec46e8dc1996fe743a74830c1747badb80f7627943757b5",
    "rmap_198e11276e6ada7dc33ac303135b2d2dd384b6008ef8b8440fc1f126c756359d",
];

#[test]
fn goldens_and_roundtrips() {
    let mut actual: Vec<String> = Vec::new();

    let collection = source_collection();
    actual.push(collection.digest().unwrap().to_string());
    let bytes = collection.to_canonical_bytes().unwrap();
    assert_eq!(
        SourceCollection::from_canonical_bytes(&bytes).unwrap(),
        collection
    );

    let import = import_manifest();
    let import_rev = import.revision().unwrap();
    actual.push(import_rev.to_string());
    let bytes = import.to_canonical_bytes().unwrap();
    let back = ImportManifest::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(back, import);
    assert_eq!(back.to_canonical_bytes().unwrap(), bytes);

    // Deterministic asset identity and produced revision.
    let key: PackEntryKey = "models/watchtower".parse().unwrap();
    let asset_id = import.asset_id_for(&key);
    actual.push(asset_id.to_string());
    let produced = import
        .asset_manifest_for(&import.assets[0], &import_rev)
        .unwrap();
    assert_eq!(produced.asset_id, asset_id);
    actual.push(produced.revision().unwrap().to_string());
    // Lineage is pinned: provenance carries the exact import revision plus
    // the collection/pack locator, while the rights record survives VERBATIM
    // — upstream source URL, terms digest, and archive digest included.
    let prov = produced.provenance.as_ref().unwrap();
    assert_eq!(prov.generator, "import");
    assert_eq!(prov.model, "kenney/space-kit");
    assert_eq!(prov.version, "1.0");
    assert_eq!(prov.params_digest, Some(*import_rev.as_bytes()));
    assert_eq!(produced.rights, import.rights);
    assert_eq!(produced.rights.source, "https://kenney.nl/assets/space-kit");
    assert_eq!(produced.rights.terms_digest, Some(sha256(b"CC0-1.0 legal text")));
    assert_eq!(
        produced.rights.source_archive,
        Some(sha256(b"space-kit-1.0.zip"))
    );

    let recipe = recipe_lod();
    actual.push(recipe.digest().unwrap().to_string());
    let bytes = recipe.to_canonical_bytes().unwrap();
    assert_eq!(ProcessingRecipe::from_canonical_bytes(&bytes).unwrap(), recipe);

    let base = produced.revision_ref().unwrap();
    let lod = lod_variant(base, 0xa1, 0xb1);
    actual.push(lod.id().unwrap().to_string());
    let bytes = lod.to_canonical_bytes().unwrap();
    assert_eq!(
        DerivedVariantManifest::from_canonical_bytes(&bytes).unwrap(),
        lod
    );
    recipe.validate_result(&lod).unwrap();

    let thumb = thumb_variant(base, 0xa1, 0xb2);
    recipe_thumbnail().validate_result(&thumb).unwrap();

    let mut set = VariantSetManifest {
        base,
        variants: vec![lod.id().unwrap(), thumb.id().unwrap()],
        policy_version: RESOLUTION_POLICY_V1,
    };
    set.canonicalize();
    actual.push(set.id().unwrap().to_string());
    let bytes = set.to_canonical_bytes().unwrap();
    assert_eq!(VariantSetManifest::from_canonical_bytes(&bytes).unwrap(), set);

    let profile = client_profile();
    let bytes = profile.to_canonical_bytes().unwrap();
    assert_eq!(ClientProfile::from_canonical_bytes(&bytes).unwrap(), profile);

    // Variant order handed to the resolver cannot change the result.
    let map_a = resolve_variants(&set, &[lod.clone(), thumb.clone()], &profile).unwrap();
    let map_b = resolve_variants(&set, &[thumb.clone(), lod.clone()], &profile).unwrap();
    assert_eq!(map_a, map_b);
    actual.push(map_a.digest().unwrap().to_string());
    let bytes = map_a.to_canonical_bytes().unwrap();
    assert_eq!(
        ResolvedVariantMap::from_canonical_bytes(&bytes).unwrap(),
        map_a
    );
    // One entry per role: thumbnail + lod1.
    assert_eq!(map_a.entries.len(), 2);
    assert_eq!(map_a.entries[0].role, VariantRole::Thumbnail);
    assert_eq!(map_a.entries[0].variant, thumb.id().unwrap());
    assert_eq!(map_a.entries[1].role, VariantRole::File(FileRole::Lod1Glb));
    assert_eq!(map_a.entries[1].variant, lod.id().unwrap());

    assert_eq!(
        actual, GOLDENS,
        "golden digests drifted: a change here is a schema bump, not a test update"
    );
}

#[test]
fn import_canonicalize_and_field_order() {
    let mut shuffled = import_manifest();
    shuffled.assets.reverse();
    shuffled.assets.iter_mut().for_each(|a| a.files.reverse());
    assert!(matches!(
        shuffled.to_canonical_bytes(),
        Err(AssetDataError::NotSorted { .. })
    ));
    shuffled.canonicalize();
    assert_eq!(
        shuffled.to_canonical_bytes().unwrap(),
        import_manifest().to_canonical_bytes().unwrap()
    );
}

#[test]
fn asset_identity_policy_is_version_independent() {
    let import = import_manifest();
    let mut newer = import.clone();
    newer.pack_version = "2.0".into();
    let key: PackEntryKey = "models/watchtower".parse().unwrap();
    // Same asset across pack versions; different import revision.
    assert_eq!(import.asset_id_for(&key), newer.asset_id_for(&key));
    assert_ne!(import.revision().unwrap(), newer.revision().unwrap());
    // Different pack or source forks the identity.
    let mut other_pack = import.clone();
    other_pack.pack_name = "castle-kit".into();
    assert_ne!(import.asset_id_for(&key), other_pack.asset_id_for(&key));
}

#[test]
fn import_fails_closed() {
    // Forbidden redistribution refuses at validation, before any publication.
    let mut forbidden = import_manifest();
    forbidden.rights.redistribution = Redistribution::Forbidden;
    assert!(matches!(
        forbidden.validate(),
        Err(AssetDataError::Mismatch { .. })
    ));

    // Missing license refuses.
    let mut unlicensed = import_manifest();
    unlicensed.rights.license.clear();
    assert!(matches!(
        unlicensed.validate(),
        Err(AssetDataError::Missing { what: "license" })
    ));

    // Case-colliding spellings refuse outright: the path charset is
    // lowercase-only, so an uppercase spelling is malformed, never a second
    // spelling of the same entry.
    let mut colliding = import_manifest();
    colliding.assets[1].files[0].path = "models/WATCHTOWER.glb".into();
    assert!(matches!(
        colliding.validate(),
        Err(AssetDataError::Malformed { .. })
    ));
    // The same path naming a DIFFERENT object refuses.
    colliding.assets[1].files[0].path = "models/watchtower.glb".into();
    assert!(matches!(
        colliding.validate(),
        Err(AssetDataError::Mismatch { what: "shared import path" })
    ));

    // Traversal and absolute paths refuse.
    for bad in ["../escape.glb", "/abs.glb", "a//b.glb", "a/./b.glb", ".hidden"] {
        let mut manifest = import_manifest();
        manifest.assets[0].files[0].file.role = FileRole::Source;
        manifest.assets[0].files[0].path = bad.into();
        assert!(
            matches!(manifest.validate(), Err(AssetDataError::Malformed { .. })),
            "path {bad:?} must refuse"
        );
    }

    // Unknown mapping policy refuses.
    let mut unknown_policy = import_manifest();
    unknown_policy.policy_version = 2;
    assert!(matches!(
        unknown_policy.validate(),
        Err(AssetDataError::UnsupportedSchema { .. })
    ));

    // A mesh entry without a thumbnail refuses through the produced manifest.
    let mut no_thumb = import_manifest();
    no_thumb.assets[0].thumbnail = None;
    no_thumb.assets[0].metrics.total_bytes -= 300;
    assert!(matches!(
        no_thumb.validate(),
        Err(AssetDataError::Missing { what: "mesh thumbnail" })
    ));

    // Lying byte totals refuse.
    let mut lying = import_manifest();
    lying.assets[0].metrics.total_bytes += 1;
    assert!(matches!(
        lying.validate(),
        Err(AssetDataError::Mismatch { .. })
    ));
}

#[test]
fn shared_pack_paths_only_as_the_exact_same_object() {
    // Kenney packs share texture sheets across models: the SAME canonical
    // object (path + identical file facts) may appear under several assets.
    let mut shared = import_manifest();
    let hull_panel = shared.assets[1].files[0].clone();
    shared.assets[0].files.push(ImportFile {
        path: hull_panel.path.clone(),
        file: AssetFile {
            role: FileRole::Albedo,
            ..hull_panel.file
        },
    });
    // Same path but a different role is a DIFFERENT object: refused.
    shared.canonicalize();
    let mut divergent_role = shared.clone();
    divergent_role.assets[0].metrics.total_bytes += 4000;
    assert!(matches!(
        divergent_role.validate(),
        Err(AssetDataError::Mismatch { what: "shared import path" })
    ));

    // Exact same object: allowed, and the import digest is well-defined.
    let mut ok = import_manifest();
    ok.assets[0].files.push(hull_panel.clone());
    ok.assets[0].metrics.total_bytes += 4000;
    ok.assets[0].metrics.max_texture_dim = 2048;
    ok.canonicalize();
    ok.validate().unwrap();
    ok.revision().unwrap();

    // Same path, same role, different byte length: refused.
    let mut lying = ok.clone();
    for f in &mut lying.assets[0].files {
        if f.path == hull_panel.path {
            f.file.byte_len += 1;
        }
    }
    lying.assets[0].metrics.total_bytes += 1;
    assert!(matches!(
        lying.validate(),
        Err(AssetDataError::Mismatch { what: "shared import path" })
    ));

    // Same path, different blob digest: refused.
    let mut forked = ok.clone();
    for f in &mut forked.assets[0].files {
        if f.path == hull_panel.path {
            f.file.blob = BlobId::hash_of(b"other bytes");
        }
    }
    assert!(matches!(
        forked.validate(),
        Err(AssetDataError::Mismatch { what: "shared import path" })
    ));

    // A file use can never alias a thumbnail use of the same path.
    let mut mixed = import_manifest();
    mixed.assets[1].files.push(ImportFile {
        path: "previews/watchtower.png".into(),
        file: AssetFile {
            role: FileRole::PreviewFront,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Png,
            blob: blob(0xa3),
            byte_len: 300,
            dims: Some(ImageDims {
                width: 512,
                height: 512,
            }),
        },
    });
    mixed.assets[1].metrics.total_bytes += 300;
    mixed.assets[1].metrics.max_texture_dim = 2048;
    mixed.canonicalize();
    assert!(matches!(
        mixed.validate(),
        Err(AssetDataError::Mismatch { what: "shared import path" })
    ));
}

#[test]
fn windows_hostile_path_segments_refuse() {
    for bad in [
        "models/tower.",       // trailing dot: Windows strips it
        "models/con.glb",      // reserved device stem
        "aux/texture.png",     // reserved device directory
        "models/com1.bin",     // reserved numbered device
        "lpt9/x.png",          // reserved numbered device directory
        "nul",                 // bare reserved device
    ] {
        let mut manifest = import_manifest();
        manifest.assets[0].files[0].file.role = FileRole::Source;
        manifest.assets[0].files[0].path = bad.into();
        assert!(
            matches!(manifest.validate(), Err(AssetDataError::Malformed { .. })),
            "path {bad:?} must refuse"
        );
    }
    // Names that merely contain a reserved word are fine. The render mesh
    // keeps its role; only its pack path moves.
    for good in ["console/model.glb", "connect.glb", "comix/1.png", "auxiliary.bin"] {
        let mut manifest = import_manifest();
        manifest.assets[0].files[0].path = good.into();
        manifest.validate().unwrap();
    }
}

#[test]
fn attribution_required_needs_credits() {
    let mut attributed = import_manifest();
    attributed.rights.redistribution = Redistribution::AttributionRequired;
    attributed.validate().unwrap();

    attributed.rights.credits.clear();
    assert!(matches!(
        attributed.validate(),
        Err(AssetDataError::Missing {
            what: "credits for attribution-required rights"
        })
    ));

    // Derivative attribution has the same law.
    let mut derivative = import_manifest();
    derivative.rights.derivatives = DerivativePolicy::AttributionRequired;
    derivative.validate().unwrap();
    derivative.rights.credits.clear();
    assert!(matches!(
        derivative.validate(),
        Err(AssetDataError::Missing {
            what: "credits for attribution-required rights"
        })
    ));
}

#[test]
fn import_alias_must_fit_alias_contract() {
    let mut long = import_manifest();
    long.source_id = "s".repeat(40);
    long.pack_name = "p".repeat(40);
    // 40 + 1 + 40 + 1 + 96 = 178 bytes > MAX_ALIAS_BYTES: the alias this
    // entry would publish under cannot exist, so the import refuses whole.
    let key: PackEntryKey = format!("{}/{}", "k".repeat(48), "m".repeat(47))
        .parse()
        .unwrap();
    long.assets[0].key = key;
    long.canonicalize();
    assert!(matches!(
        long.validate(),
        Err(AssetDataError::OverBudget { .. })
    ));
    // The fixture's own aliases fit and are exactly the deterministic form.
    let import = import_manifest();
    assert_eq!(
        import
            .alias_for(&import.assets[0].key)
            .unwrap()
            .as_str(),
        "kenney/space-kit/models/watchtower"
    );
}

#[test]
fn recipe_settings_fail_closed() {
    let mut recipe = recipe_lod();
    recipe.settings = RecipeSettings::MeshLod {
        lod: 3,
        target_triangles: 300,
    };
    assert!(matches!(
        recipe.validate(),
        Err(AssetDataError::Malformed { what: "recipe lod" })
    ));

    recipe.settings = RecipeSettings::ImageTranscode {
        source_role: FileRole::Texture,
        media: ThumbnailMedia::Png,
        quality: 50,
    };
    assert!(matches!(recipe.validate(), Err(AssetDataError::Mismatch { .. })));

    recipe.settings = RecipeSettings::ImageResize {
        source_role: FileRole::RenderGlb,
        width: 256,
        height: 256,
        media: ThumbnailMedia::Png,
    };
    assert!(matches!(recipe.validate(), Err(AssetDataError::Mismatch { .. })));

    let mut unknown_schema = recipe_lod();
    unknown_schema.output_schema = 9;
    assert!(matches!(
        unknown_schema.validate(),
        Err(AssetDataError::UnsupportedSchema { .. })
    ));
}

#[test]
fn validate_result_refuses_claim_mismatches() {
    let base = aref(1, 0x11);
    let recipe = recipe_lod();

    // Over-target triangle count refuses: measured beats claimed.
    let mut over = lod_variant(base, 0xa1, 0xb1);
    over.metrics.triangles = 400;
    assert!(matches!(
        recipe.validate_result(&over),
        Err(AssetDataError::Mismatch { .. })
    ));

    // Wrong recipe digest refuses.
    let mut wrong_recipe = lod_variant(base, 0xa1, 0xb1);
    wrong_recipe.recipe = RecipeDigest::hash_of(b"other");
    assert!(matches!(
        recipe.validate_result(&wrong_recipe),
        Err(AssetDataError::Mismatch { .. })
    ));

    // A thumbnail with wrong dimensions refuses against its recipe.
    let mut wrong_dims = thumb_variant(base, 0xa1, 0xb2);
    wrong_dims.thumbnail.as_mut().unwrap().width = 256;
    assert!(matches!(
        recipe_thumbnail().validate_result(&wrong_dims),
        Err(AssetDataError::Mismatch { .. })
    ));

    // Thumbnail-kind variants cannot smuggle file outputs.
    let mut smuggle = thumb_variant(base, 0xa1, 0xb2);
    smuggle.outputs = lod_variant(base, 0xa1, 0xb3).outputs;
    assert!(smuggle.validate().is_err());

    // A result that omits the recipe's own input role has untracked lineage.
    let mut no_input = lod_variant(base, 0xa1, 0xb1);
    no_input.inputs = vec![DerivedInput {
        role: FileRole::Albedo,
        blob: blob(0xa1),
    }];
    assert!(matches!(
        recipe.validate_result(&no_input),
        Err(AssetDataError::Missing {
            what: "recipe input role in variant inputs"
        })
    ));
}

#[test]
fn nondeterministic_tool_refuses_in_v1() {
    let mut recipe = recipe_lod();
    recipe.tool.deterministic = false;
    assert!(matches!(
        recipe.validate(),
        Err(AssetDataError::Malformed {
            what: "nondeterministic tool closure"
        })
    ));
    // And it cannot reach a derivation key either: the digest itself
    // validates first.
    assert!(recipe.digest().is_err());
}

#[test]
fn collider_result_enforces_hull_vertex_budget() {
    let base = aref(1, 0x11);
    let recipe = ProcessingRecipe {
        settings: RecipeSettings::MeshCollider {
            max_hull_vertices: 64,
        },
        tool: tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    };
    let collider = |vertices: u32| DerivedVariantManifest {
        base,
        kind: RecipeKind::MeshCollider,
        recipe: recipe.digest().unwrap(),
        inputs: vec![DerivedInput {
            role: FileRole::RenderGlb,
            blob: blob(0xa1),
        }],
        outputs: vec![AssetFile {
            role: FileRole::Collider,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Bin,
            blob: blob(0xc1),
            byte_len: 128,
            dims: None,
        }],
        thumbnail: None,
        metrics: Metrics {
            total_bytes: 128,
            vertices,
            ..Default::default()
        },
        rights: kenney_rights(),
    };
    recipe.validate_result(&collider(48)).unwrap();
    assert!(matches!(
        recipe.validate_result(&collider(65)),
        Err(AssetDataError::Mismatch {
            what: "collider vertices vs hull budget"
        })
    ));
    assert!(matches!(
        recipe.validate_result(&collider(0)),
        Err(AssetDataError::Mismatch { .. })
    ));
}

#[test]
fn mixed_output_roles_refuse() {
    let base = aref(1, 0x11);
    let mut mixed = lod_variant(base, 0xa1, 0xb1);
    mixed.outputs.push(AssetFile {
        role: FileRole::Collider,
        tier: DeviceTier::Any,
        lod: 0,
        media: MediaType::Bin,
        blob: blob(0xc2),
        byte_len: 100,
        dims: None,
    });
    mixed.canonicalize();
    mixed.metrics.total_bytes += 100;
    assert!(matches!(
        mixed.validate(),
        Err(AssetDataError::Mismatch {
            what: "mixed variant output roles"
        })
    ));
}

#[test]
fn resolved_map_is_atomic_overrides_over_base() {
    // The map overrides exactly the roles it names; every other base role is
    // retained unchanged from the base manifest.
    let import = import_manifest();
    let import_rev = import.revision().unwrap();
    let produced = import
        .asset_manifest_for(&import.assets[0], &import_rev)
        .unwrap();
    let base = produced.revision_ref().unwrap();

    let lod = lod_variant(base, 0xa1, 0xb1);
    let thumb = thumb_variant(base, 0xa1, 0xb2);
    let mut set = VariantSetManifest {
        base,
        variants: vec![lod.id().unwrap(), thumb.id().unwrap()],
        policy_version: RESOLUTION_POLICY_V1,
    };
    set.canonicalize();
    let map = resolve_variants(&set, &[lod.clone(), thumb.clone()], &client_profile()).unwrap();

    // Overridden roles.
    let lod_override = map.override_for(VariantRole::File(FileRole::Lod1Glb)).unwrap();
    assert_eq!(lod_override.variant, lod.id().unwrap());
    assert_eq!(lod_override.blobs, vec![blob(0xb1)]);
    let thumb_override = map.override_for(VariantRole::Thumbnail).unwrap();
    assert_eq!(thumb_override.variant, thumb.id().unwrap());

    // Roles the map does not name stay the base revision's own files: the
    // render mesh and collider are served exactly as the manifest pins them.
    assert!(map.override_for(VariantRole::File(FileRole::RenderGlb)).is_none());
    assert!(map.override_for(VariantRole::File(FileRole::Collider)).is_none());
    assert!(produced.files.iter().any(|f| f.role == FileRole::RenderGlb));
    assert!(produced.files.iter().any(|f| f.role == FileRole::Collider));
}

#[test]
fn derivation_key_is_order_independent_and_duplicate_closed() {
    let base = aref(1, 0x11);
    let recipe = recipe_lod().digest().unwrap();
    let a = DerivedInput {
        role: FileRole::RenderGlb,
        blob: blob(1),
    };
    let b = DerivedInput {
        role: FileRole::Albedo,
        blob: blob(2),
    };
    let k1 = derivation_key(&base, &recipe, &[a, b]).unwrap();
    let k2 = derivation_key(&base, &recipe, &[b, a]).unwrap();
    assert_eq!(k1, k2);
    assert!(k1.to_string().starts_with("dkey_"));

    // Any component change changes the key.
    let other_recipe = recipe_thumbnail().digest().unwrap();
    assert_ne!(k1, derivation_key(&base, &other_recipe, &[a, b]).unwrap());
    assert_ne!(
        k1,
        derivation_key(&aref(1, 0x12), &recipe, &[a, b]).unwrap()
    );
    assert_ne!(k1, derivation_key(&base, &recipe, &[a]).unwrap());

    assert!(matches!(
        derivation_key(&base, &recipe, &[a, a]),
        Err(AssetDataError::Duplicate { .. })
    ));
    assert!(matches!(
        derivation_key(&base, &recipe, &[]),
        Err(AssetDataError::Malformed { .. })
    ));
}

#[test]
fn resolution_fails_closed() {
    let base = aref(1, 0x11);
    let lod = lod_variant(base, 0xa1, 0xb1);
    let thumb = thumb_variant(base, 0xa1, 0xb2);
    let mut set = VariantSetManifest {
        base,
        variants: vec![lod.id().unwrap(), thumb.id().unwrap()],
        policy_version: RESOLUTION_POLICY_V1,
    };
    set.canonicalize();
    let profile = client_profile();

    // A manifest not in the set refuses.
    let stranger = lod_variant(base, 0xa1, 0xb9);
    assert!(matches!(
        resolve_variants(&set, &[stranger, thumb.clone()], &profile),
        Err(AssetDataError::Mismatch { .. })
    ));
    // Wrong count refuses.
    assert!(matches!(
        resolve_variants(&set, &[lod.clone()], &profile),
        Err(AssetDataError::Mismatch { .. })
    ));
    // Wrong base refuses.
    let mut foreign = lod_variant(aref(2, 0x21), 0xa1, 0xb1);
    foreign.canonicalize();
    assert!(matches!(
        resolve_variants(&set, &[foreign, thumb.clone()], &profile),
        Err(AssetDataError::Mismatch { .. })
    ));
    // Policy version mismatch refuses.
    let mut wrong_policy = profile;
    wrong_policy.policy_version = 7;
    assert!(resolve_variants(&set, &[lod.clone(), thumb.clone()], &wrong_policy).is_err());

    // A role whose only variant is incompatible refuses instead of
    // substituting: GLB not accepted, but the LOD role exists in the set.
    let mut no_glb = profile;
    no_glb.accept_glb = false;
    assert!(matches!(
        resolve_variants(&set, &[lod.clone(), thumb.clone()], &no_glb),
        Err(AssetDataError::Missing { what: "compatible variant for role" })
    ));

    // Resolution never invents content: an empty set cannot even exist.
    let empty = VariantSetManifest {
        base,
        variants: vec![],
        policy_version: RESOLUTION_POLICY_V1,
    };
    assert!(empty.validate().is_err());
}

#[test]
fn new_document_kinds_do_not_cross_decode() {
    let import_bytes = import_manifest().to_canonical_bytes().unwrap();
    assert!(matches!(
        SourceCollection::from_canonical_bytes(&import_bytes),
        Err(AssetDataError::BadDocKind { .. })
    ));
    let recipe_bytes = recipe_lod().to_canonical_bytes().unwrap();
    assert!(matches!(
        DerivedVariantManifest::from_canonical_bytes(&recipe_bytes),
        Err(AssetDataError::BadDocKind { .. })
    ));
    // And the frozen kinds refuse the new ones.
    assert!(matches!(
        AssetManifest::from_canonical_bytes(&import_bytes),
        Err(AssetDataError::BadDocKind { .. })
    ));
}
