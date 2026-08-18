//! Shared fixtures: one fully populated instance of every canonical document,
//! built from fixed bytes so golden digests are stable across restart and
//! platform.

use makepad_asset_data::*;

pub fn blob(n: u8) -> BlobId {
    BlobId::from_bytes([n; 32])
}
pub fn arev(n: u8) -> AssetRevisionId {
    AssetRevisionId::from_bytes([n; 32])
}
pub fn asset_id(n: u8) -> AssetId {
    AssetId::from_bytes([n; 16])
}
pub fn aref(id: u8, rev: u8) -> AssetRevisionRef {
    AssetRevisionRef {
        asset_id: asset_id(id),
        revision: arev(rev),
    }
}
pub fn game_id() -> GameId {
    GameId::from_bytes([0x77; 16])
}
pub fn txn(n: u8) -> TransactionId {
    TransactionId::from_bytes([n; 16])
}

pub fn rmap(n: u8) -> ResolvedMapDigest {
    ResolvedMapDigest::from_bytes([n; 32])
}
pub fn rres(n: u8) -> RealmResolutionDigest {
    RealmResolutionDigest::from_bytes([n; 32])
}
pub fn vset_id(n: u8) -> VariantSetId {
    VariantSetId::from_bytes([n; 32])
}

pub fn readiness() -> ReadinessPolicy {
    ReadinessPolicy {
        on_unready: UnreadyPeerPolicy::DemoteToSpectator,
        deadline_millis: 30_000,
    }
}

pub fn ticket(n: u8) -> JoinTicket {
    JoinTicket {
        transaction_id: txn(n),
        realm_epoch: RealmEpoch(3),
        game_revision: GameRevisionId::hash_of(b"g"),
        content_set: ContentSetId::hash_of(b"s"),
    }
}

/// Full v3 rights record fixtures. Terms digest and archive digest are
/// pinned so tests prove they survive into produced and derived manifests.
pub fn cc0_rights(credits: &str, source: &str) -> Rights {
    Rights {
        license: "CC0-1.0".into(),
        license_revision: String::new(),
        terms_digest: Some(sha256(b"CC0-1.0 legal text")),
        terms_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        credits: credits.into(),
        source: source.into(),
        source_archive: None,
        redistribution: Redistribution::Allowed,
        derivatives: DerivativePolicy::Allowed,
    }
}

pub fn kenney_rights() -> Rights {
    Rights {
        source_archive: Some(sha256(b"space-kit-1.0.zip")),
        ..cc0_rights("Kenney (kenney.nl)", "https://kenney.nl/assets/space-kit")
    }
}

pub fn thumbnail() -> ThumbnailMeta {
    ThumbnailMeta {
        blob: blob(9),
        media: ThumbnailMedia::Png,
        width: 512,
        height: 512,
        byte_len: 300,
    }
}

/// A complete generated rocket-launcher-style weapon manifest exercising every
/// field: LODs, collider, texture, thumbnail, anchors, recipe, provenance.
pub fn weapon_manifest() -> AssetManifest {
    AssetManifest {
        asset_id: asset_id(1),
        kind: AssetKind::Weapon,
        files: vec![
            AssetFile {
                role: FileRole::RenderGlb,
                tier: DeviceTier::Any,
                lod: 0,
                media: MediaType::Glb,
                blob: blob(1),
                byte_len: 1000,
                dims: None,
            },
            AssetFile {
                role: FileRole::Lod1Glb,
                tier: DeviceTier::Low,
                lod: 1,
                media: MediaType::Glb,
                blob: blob(2),
                byte_len: 800,
                dims: None,
            },
            AssetFile {
                role: FileRole::Collider,
                tier: DeviceTier::Any,
                lod: 0,
                media: MediaType::Bin,
                blob: blob(3),
                byte_len: 200,
                dims: None,
            },
            AssetFile {
                role: FileRole::Albedo,
                tier: DeviceTier::Any,
                lod: 0,
                media: MediaType::Png,
                blob: blob(4),
                byte_len: 4000,
                dims: Some(ImageDims {
                    width: 1024,
                    height: 1024,
                }),
            },
        ],
        dependencies: vec![aref(2, 0x21), aref(3, 0x31)],
        thumbnail: Some(thumbnail()),
        metrics: Metrics {
            total_bytes: 1000 + 800 + 200 + 4000 + 300,
            triangles: 1200,
            vertices: 900,
            joints: 0,
            clips: 0,
            max_texture_dim: 1024,
            media_millis: 0,
        },
        coordinate_system: CoordinateSystem {
            units_per_meter: 1.0,
            up: Axis::YPos,
            forward: Axis::ZNeg,
            pivot: Pivot::Origin,
        },
        bounds: Bounds {
            min: Vec3::new(-1.5, -0.25, -0.5),
            max: Vec3::new(1.5, 0.25, 0.5),
        },
        anchors: vec![
            Anchor {
                name: "weapon_grip".into(),
                transform: Transform {
                    pos: Vec3::new(0.0, -0.1, 0.2),
                    rot: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
            },
            Anchor {
                name: "weapon_muzzle".into(),
                transform: Transform {
                    pos: Vec3::new(0.0, 0.0, -0.5),
                    rot: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
            },
        ],
        capabilities: Capabilities {
            rigged: false,
            animated: false,
            collidable: true,
            loopable: false,
            spawnable: true,
        },
        spawn_recipe: Some(SpawnRecipe {
            class: PrefabClass::Weapon,
            params: vec![
                SpawnParam {
                    name: "damage".into(),
                    value: Value::I64(10),
                },
                SpawnParam {
                    name: "zoom".into(),
                    value: Value::F32(1.5),
                },
            ],
        }),
        provenance: Some(Provenance {
            generator: "trellis".into(),
            model: "trellis_xl".into(),
            version: "1".into(),
            seed: 42,
            parents: vec![arev(0x21)],
            params_digest: Some([7; 32]),
        }),
        rights: cc0_rights("generated", ""),
    }
}

pub fn lock() -> ContentLock {
    ContentLock {
        game_id: game_id(),
        entries: vec![
            LockEntry {
                alias: "rik2/props/crate".parse().unwrap(),
                asset_id: asset_id(4),
                revision: arev(0x41),
            },
            LockEntry {
                alias: "rik2/weapons/fancy-rocket-launcher".parse().unwrap(),
                asset_id: asset_id(1),
                revision: arev(0x11),
            },
        ],
        closure: vec![aref(1, 0x11), aref(2, 0x21), aref(3, 0x31), aref(4, 0x41)],
        // Release truth: the launcher and the crate ship frozen variant sets.
        variant_sets: vec![
            LockVariantSet {
                base: aref(1, 0x11),
                set: vset_id(0xd1),
            },
            LockVariantSet {
                base: aref(4, 0x41),
                set: vset_id(0xd4),
            },
        ],
    }
}

pub fn game_revision_manifest() -> GameRevisionManifest {
    GameRevisionManifest {
        game_id: game_id(),
        name: "Rocket Arena".into(),
        description: "A playable test arena.".into(),
        author: "rik".into(),
        splash_blob: blob(0x51),
        manifest_blob: blob(0x52),
        lock_blob: blob(0x53),
        thumbnail: thumbnail(),
        catalog_snapshot: Some(blob(0x54)),
        search_algorithm_version: 1,
        engine_version: 1,
        protocol_version: 6,
        splash_byte_len: 4096,
    }
}

pub fn scene_plan(game_revision: GameRevisionId, content_set: ContentSetId) -> ScenePlan {
    ScenePlan {
        game_revision,
        required_content_set: content_set,
        callback_module: blob(0x61),
        terrain: Some(TerrainDecl { blob: blob(0x62) }),
        objects: vec![
            SceneObject {
                key: "arena/main_gate".parse().unwrap(),
                asset: Some(aref(1, 0x11)),
                transform: Transform {
                    pos: Vec3::new(0.0, 2.0, -18.0),
                    rot: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                fixed: true,
                components: vec![ComponentConfig {
                    name: "door".into(),
                    params: vec![
                        Param {
                            name: "linked_switch".into(),
                            value: Value::Key("arena/switch".parse().unwrap()),
                        },
                        Param {
                            name: "open_speed".into(),
                            value: Value::F32(2.0),
                        },
                    ],
                }],
            },
            SceneObject {
                key: "arena/switch".parse().unwrap(),
                asset: None,
                transform: Transform::IDENTITY,
                fixed: true,
                components: vec![],
            },
        ],
        state_schemas: vec![StateSchema {
            key: "match/blue_score".parse().unwrap(),
            version: 1,
            value_type: ValueType::I64,
            default: Value::I64(0),
            lifetime: StateLifetime::Match,
        }],
    }
}

pub fn source_collection() -> SourceCollection {
    SourceCollection {
        id: "kenney".into(),
        title: "Kenney game assets".into(),
        origin: SourceOrigin::Upload,
        terms: kenney_rights(),
    }
}

/// A tiny pinned Kenney-style pack: one mesh prop (render GLB + collider +
/// preview thumbnail) and one texture, all pinned to exact source digests.
pub fn import_manifest() -> ImportManifest {
    ImportManifest {
        source_collection: source_collection().digest().unwrap(),
        source_id: "kenney".into(),
        pack_name: "space-kit".into(),
        pack_version: "1.0".into(),
        policy_version: IMPORT_ASSET_ID_POLICY_V1,
        assets: vec![
            ImportAsset {
                key: "models/watchtower".parse().unwrap(),
                kind: AssetKind::Prop,
                files: vec![
                    ImportFile {
                        path: "models/watchtower.glb".into(),
                        file: AssetFile {
                            role: FileRole::RenderGlb,
                            tier: DeviceTier::Any,
                            lod: 0,
                            media: MediaType::Glb,
                            blob: blob(0xa1),
                            byte_len: 1000,
                            dims: None,
                        },
                    },
                    ImportFile {
                        path: "colliders/watchtower.bin".into(),
                        file: AssetFile {
                            role: FileRole::Collider,
                            tier: DeviceTier::Any,
                            lod: 0,
                            media: MediaType::Bin,
                            blob: blob(0xa2),
                            byte_len: 200,
                            dims: None,
                        },
                    },
                ],
                thumbnail: Some(ImportThumbnail {
                    path: "previews/watchtower.png".into(),
                    meta: ThumbnailMeta {
                        blob: blob(0xa3),
                        media: ThumbnailMedia::Png,
                        width: 512,
                        height: 512,
                        byte_len: 300,
                    },
                }),
                metrics: Metrics {
                    total_bytes: 1000 + 200 + 300,
                    triangles: 500,
                    vertices: 300,
                    joints: 0,
                    clips: 0,
                    max_texture_dim: 512,
                    media_millis: 0,
                },
                coordinate_system: CoordinateSystem {
                    units_per_meter: 1.0,
                    up: Axis::YPos,
                    forward: Axis::ZNeg,
                    pivot: Pivot::BoundsBottom,
                },
                bounds: Bounds {
                    min: Vec3::new(-1.0, 0.0, -1.0),
                    max: Vec3::new(1.0, 3.0, 1.0),
                },
                anchors: vec![],
                capabilities: Capabilities {
                    collidable: true,
                    ..Default::default()
                },
                spawn_recipe: None,
            },
            ImportAsset {
                key: "textures/hull-panel".parse().unwrap(),
                kind: AssetKind::Texture,
                files: vec![ImportFile {
                    path: "textures/hull_panel.png".into(),
                    file: AssetFile {
                        role: FileRole::Texture,
                        tier: DeviceTier::Any,
                        lod: 0,
                        media: MediaType::Png,
                        blob: blob(0xa4),
                        byte_len: 4000,
                        dims: Some(ImageDims {
                            width: 2048,
                            height: 2048,
                        }),
                    },
                }],
                thumbnail: None,
                metrics: Metrics {
                    total_bytes: 4000,
                    max_texture_dim: 2048,
                    ..Default::default()
                },
                coordinate_system: CoordinateSystem {
                    units_per_meter: 1.0,
                    up: Axis::YPos,
                    forward: Axis::ZNeg,
                    pivot: Pivot::Origin,
                },
                bounds: Bounds {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                anchors: vec![],
                capabilities: Capabilities::default(),
                spawn_recipe: None,
            },
        ],
        rights: kenney_rights(),
    }
}

pub fn tool() -> ToolClosure {
    ToolClosure {
        processor: "mp_derive".into(),
        version: "1.0".into(),
        build: "deadbeef".into(),
        deterministic: true,
    }
}

pub fn recipe_thumbnail() -> ProcessingRecipe {
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

pub fn recipe_lod() -> ProcessingRecipe {
    ProcessingRecipe {
        settings: RecipeSettings::MeshLod {
            lod: 1,
            target_triangles: 300,
        },
        tool: tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    }
}

pub fn recipe_resize() -> ProcessingRecipe {
    ProcessingRecipe {
        settings: RecipeSettings::ImageResize {
            source_role: FileRole::Texture,
            width: 256,
            height: 256,
            media: ThumbnailMedia::Png,
        },
        tool: tool(),
        output_schema: OUTPUT_SCHEMA_V1,
    }
}

/// A LOD variant of `base`, produced by [`recipe_lod`] from input blob `inp`.
pub fn lod_variant(base: AssetRevisionRef, inp: u8, out: u8) -> DerivedVariantManifest {
    DerivedVariantManifest {
        base,
        kind: RecipeKind::MeshLod,
        recipe: recipe_lod().digest().unwrap(),
        inputs: vec![DerivedInput {
            role: FileRole::RenderGlb,
            blob: blob(inp),
        }],
        outputs: vec![AssetFile {
            role: FileRole::Lod1Glb,
            tier: DeviceTier::Low,
            lod: 1,
            media: MediaType::Glb,
            blob: blob(out),
            byte_len: 600,
            dims: None,
        }],
        thumbnail: None,
        metrics: Metrics {
            total_bytes: 600,
            triangles: 280,
            vertices: 200,
            ..Default::default()
        },
        // Derived variants inherit their base's exact rights record.
        rights: kenney_rights(),
    }
}

/// A thumbnail variant of `base`, produced by [`recipe_thumbnail`].
pub fn thumb_variant(base: AssetRevisionRef, inp: u8, out: u8) -> DerivedVariantManifest {
    DerivedVariantManifest {
        base,
        kind: RecipeKind::MeshThumbnail,
        recipe: recipe_thumbnail().digest().unwrap(),
        inputs: vec![DerivedInput {
            role: FileRole::RenderGlb,
            blob: blob(inp),
        }],
        outputs: vec![],
        thumbnail: Some(ThumbnailMeta {
            blob: blob(out),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: 450,
        }),
        metrics: Metrics {
            total_bytes: 450,
            ..Default::default()
        },
        // Derived variants inherit their base's exact rights record.
        rights: kenney_rights(),
    }
}

pub fn client_profile() -> ClientProfile {
    ClientProfile {
        policy_version: RESOLUTION_POLICY_V1,
        tier: DeviceTier::High,
        max_texture_dim: 2048,
        max_triangles: 1_000_000,
        max_variant_bytes: 64 * 1024 * 1024,
        accept_png: true,
        accept_jpeg: true,
        accept_glb: true,
        accept_bin: true,
    }
}

pub fn migration_plan(from: GameRevisionId, to: GameRevisionId) -> SceneMigrationPlan {
    SceneMigrationPlan {
        from_game_revision: from,
        to_game_revision: to,
        activation_mode: ActivationMode::Migrate,
        ops: vec![
            SceneOp {
                key: "arena/main_gate".parse().unwrap(),
                kind: SceneOpKind::Update {
                    components: vec![ComponentRule {
                        component: "door".into(),
                        rule: PreserveRule::Preserve,
                    }],
                },
            },
            SceneOp {
                key: "arena/old_wall".parse().unwrap(),
                kind: SceneOpKind::Remove,
            },
            SceneOp {
                key: "arena/turret".parse().unwrap(),
                kind: SceneOpKind::Create,
            },
        ],
        state_migrations: vec![StateMigration {
            key: "match/blue_score".parse().unwrap(),
            from_version: 1,
            to_version: 2,
            op: StateMigrationOp::WidenNumeric,
        }],
        terrain_policy: TerrainPolicy::Keep,
        rebuild_scopes: RebuildScopes {
            renderer_static: true,
            physics_static: true,
            ..Default::default()
        },
        reasons: vec![
            MigrationReason {
                code: MigrationReasonCode::KeyAdded,
                key: Some("arena/turret".parse().unwrap()),
                detail: "new turret".into(),
            },
            MigrationReason {
                code: MigrationReasonCode::KeyRemoved,
                key: Some("arena/old_wall".parse().unwrap()),
                detail: "wall removed".into(),
            },
        ],
        requires_user_confirmation: false,
    }
}
