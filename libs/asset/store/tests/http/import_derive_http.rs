//! The shipping vertical slice, end to end over real sockets: a licensed
//! multi-asset Kenney-style pack imports deterministically under registered
//! authoritative terms, typed worker jobs derive cached thumbnail/AO/LOD and
//! texture-resize variants, a variant set freezes, and a bounded client
//! profile resolves to exact digests — twice, on two clean servers, with
//! byte-identical identities.

mod common;
use common::*;
use makepad_asset_store::json::Value;
use makepad_asset_data::*;

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn tool() -> ToolClosure {
    ToolClosure {
        processor: "mp_derive".into(),
        version: "1.0".into(),
        build: "deadbeef".into(),
        deterministic: true,
    }
}

fn recipes() -> Vec<(&'static str, ProcessingRecipe)> {
    vec![
        (
            "thumb",
            ProcessingRecipe {
                settings: RecipeSettings::MeshThumbnail {
                    width: 512,
                    height: 512,
                    media: ThumbnailMedia::Png,
                },
                tool: tool(),
                output_schema: OUTPUT_SCHEMA_V1,
            },
        ),
        (
            "lod",
            ProcessingRecipe {
                settings: RecipeSettings::MeshLod {
                    lod: 1,
                    target_triangles: 200,
                },
                tool: tool(),
                output_schema: OUTPUT_SCHEMA_V1,
            },
        ),
        (
            "ao",
            ProcessingRecipe {
                settings: RecipeSettings::MeshAo { resolution: 256 },
                tool: tool(),
                output_schema: OUTPUT_SCHEMA_V1,
            },
        ),
        (
            "resize",
            ProcessingRecipe {
                settings: RecipeSettings::ImageResize {
                    source_role: FileRole::Texture,
                    width: 256,
                    height: 256,
                    media: ThumbnailMedia::Png,
                },
                tool: tool(),
                output_schema: OUTPUT_SCHEMA_V1,
            },
        ),
    ]
}

/// The deterministic bytes the fixture worker produces for one recipe tag —
/// a pure function standing in for the real kernel, so two clean servers
/// derive byte-identical variants.
fn worker_bytes(tag: &str, input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(tag.as_bytes());
    out.push(b':');
    out.extend_from_slice(&sha256(input));
    out
}

/// One server's complete run. Returns every identity the flow produced so
/// the determinism test can compare two clean servers.
fn run_slice(name: &str) -> (TestServer, Vec<String>) {
    let ts = start_server(name);
    let admin_tok = ts.admin_token();
    let mut admin = ts.control(Some(&admin_tok));
    let mut ids: Vec<String> = Vec::new();

    // Principals, least privilege each.
    let importer = principal_with(
        &mut admin,
        &[
            ("import_source", "kenney"),
            ("import_run", "kenney"),
            ("blob_write", "kenney"),
        ],
    );
    let deriver = principal_with(&mut admin, &[("derive_request", "kenney")]);
    let worker = principal_with(
        &mut admin,
        &[("job_worker", "kenney"), ("blob_write", "kenney")],
    );
    let publisher = principal_with(&mut admin, &[("asset_publish", "kenney")]);
    let reader = principal_with(&mut admin, &[]);

    // Upload the pack's pinned source bytes.
    let mut importer_data = ts.data(Some(&importer));
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        let r = importer_data.post_bytes("/v1/blobs?ns=kenney", bytes);
        assert_eq!(r.status, 201, "blob upload: {}", String::from_utf8_lossy(&r.body));
    }

    // Register the authoritative source collection, then import.
    let mut importer_ctl = ts.control(Some(&importer));
    let collection_bytes = kenney_collection().to_canonical_bytes().unwrap();
    let r = importer_ctl.request("PUT", "/v1/import-sources", &[], Some(&collection_bytes));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let r = importer_ctl.get("/v1/import-sources");
    assert_eq!(r.status, 200);
    let sources = r.json();
    let row = &sources.get("sources").unwrap().as_arr().unwrap()[0];
    assert_eq!(row.get("source_id").unwrap().as_str().unwrap(), "kenney");
    assert_eq!(row.get("license").unwrap().as_str().unwrap(), "CC0-1.0");

    let manifest = kenney_pack("1.0");
    let manifest_bytes = manifest.to_canonical_bytes().unwrap();
    let r = importer_ctl.request("POST", "/v1/imports", &[], Some(&manifest_bytes));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let report = r.json();
    assert_eq!(report.get("created").unwrap().as_bool().unwrap(), true);
    let import_revision = report.get("import_revision").unwrap().as_str().unwrap().to_string();
    ids.push(import_revision.clone());
    let entries = report.get("entries").unwrap().as_arr().unwrap().to_vec();
    assert_eq!(entries.len(), 2);
    for e in &entries {
        ids.push(e.get("asset_id").unwrap().as_str().unwrap().to_string());
        ids.push(e.get("revision").unwrap().as_str().unwrap().to_string());
    }
    // Idempotent replay over HTTP.
    let r = importer_ctl.request("POST", "/v1/imports", &[], Some(&manifest_bytes));
    assert_eq!(r.status, 200);
    assert_eq!(r.json().get("created").unwrap().as_bool().unwrap(), false);

    // The deterministic alias resolves for any authenticated reader, and the
    // manifest projection carries the full rights record.
    let mut reader_ctl = ts.control(Some(&reader));
    let r = reader_ctl.get("/v1/aliases/kenney/space-kit/models/watchtower");
    assert_eq!(r.status, 200);
    let watchtower_rev = r.str_field("head_revision");
    let r = reader_ctl.get(&format!("/v1/revisions/{watchtower_rev}/json"));
    assert_eq!(r.status, 200);
    let rights = r.json().get("rights").unwrap().clone();
    assert_eq!(rights.get("license").unwrap().as_str().unwrap(), "CC0-1.0");
    assert_eq!(rights.get("redistribution").unwrap().as_str().unwrap(), "allowed");
    assert!(rights.get("terms_digest").unwrap().as_str().is_some());
    assert!(rights.get("source_archive").unwrap().as_str().is_some());

    // Capability refusals before any derivation exists.
    let r = reader_ctl.request("POST", "/v1/imports", &[], Some(&manifest_bytes));
    assert_eq!(r.status, 403);
    assert_eq!(r.str_field("capability"), "import_run");
    let mut deriver_ctl = ts.control(Some(&deriver));
    let r = deriver_ctl.request("PUT", "/v1/import-sources", &[], Some(&collection_bytes));
    assert_eq!(r.status, 403);
    assert_eq!(r.str_field("capability"), "import_source");

    // Request all four derivations (three mesh, one texture).
    let watchtower = entries
        .iter()
        .find(|e| e.get("key").unwrap().as_str().unwrap() == "models/watchtower")
        .unwrap();
    let texture = entries
        .iter()
        .find(|e| e.get("key").unwrap().as_str().unwrap() == "textures/hull-panel")
        .unwrap();
    let mut dkeys: Vec<(String, String, String)> = Vec::new(); // (tag, dkey, job)
    for (tag, recipe) in recipes() {
        let target = if *tag == *"resize" { texture } else { watchtower };
        let body = json_obj(vec![
            ("base_asset", jstr(target.get("asset_id").unwrap().as_str().unwrap())),
            ("base_revision", jstr(target.get("revision").unwrap().as_str().unwrap())),
            ("recipe", jstr(&hex(&recipe.to_canonical_bytes().unwrap()))),
        ]);
        let r = deriver_ctl.post_json("/v1/derivations", &body);
        assert_eq!(r.status, 202, "{tag}: {}", String::from_utf8_lossy(&r.body));
        let v = r.json();
        assert_eq!(v.get("status").unwrap().as_str().unwrap(), "pending");
        assert_eq!(v.get("joined").unwrap().as_bool().unwrap(), false);
        let dkey = v.get("dkey").unwrap().as_str().unwrap().to_string();
        let job = v.get("job").unwrap().as_str().unwrap().to_string();
        // A concurrent identical request joins the SAME job: single flight
        // holds over HTTP.
        let r = deriver_ctl.post_json("/v1/derivations", &body);
        assert_eq!(r.status, 202);
        let joined = r.json();
        assert_eq!(joined.get("joined").unwrap().as_bool().unwrap(), true);
        assert_eq!(joined.get("job").unwrap().as_str().unwrap(), job);
        // Reader without derive_request refuses.
        let r = reader_ctl.post_json("/v1/derivations", &body);
        assert_eq!(r.status, 403);
        assert_eq!(r.str_field("capability"), "derive_request");
        dkeys.push((tag.to_string(), dkey, job));
    }

    // The deterministic worker: claim typed jobs, compute pure-function
    // outputs, upload them, complete against the derivation key.
    let mut worker_ctl = ts.control(Some(&worker));
    let mut worker_data = ts.data(Some(&worker));
    let kinds = Value::Arr(vec![
        jstr("derive.mesh_thumbnail"),
        jstr("derive.mesh_lod"),
        jstr("derive.mesh_ao"),
        jstr("derive.image_resize"),
    ]);
    let mut completed = 0;
    while completed < 4 {
        let r = worker_ctl.post_json(
            "/v1/worker/claim",
            &json_obj(vec![
                ("lease_ms", Value::Int(60_000)),
                ("kinds", kinds.clone()),
            ]),
        );
        assert_eq!(r.status, 200);
        let claim = r.json();
        let job = match claim.get("job") {
            Some(Value::Str(j)) => j.clone(),
            _ => panic!("expected a claimable job, got {}", claim.to_json()),
        };
        let kind = claim.get("kind").unwrap().as_str().unwrap().to_string();
        let body = claim.get("body").unwrap().clone();
        let dkey = body.get("dkey").unwrap().as_str().unwrap().to_string();

        let (result, tag) = match kind.as_str() {
            "derive.mesh_thumbnail" => {
                let bytes = worker_bytes("THUMB-512", PACK_GLB);
                let blob = upload(&mut worker_data, &bytes);
                (
                    json_obj(vec![
                        ("job", jstr(&job)),
                        ("thumbnail", json_obj(vec![
                            ("blob", jstr(&blob)),
                            ("media", jstr("png")),
                            ("width", Value::Int(512)),
                            ("height", Value::Int(512)),
                            ("byte_len", Value::Int(bytes.len() as i64)),
                        ])),
                        ("metrics", json_obj(vec![(
                            "total_bytes",
                            Value::Int(bytes.len() as i64),
                        )])),
                    ]),
                    "thumb",
                )
            }
            "derive.mesh_lod" => {
                let bytes = worker_bytes("LOD1-T200", PACK_GLB);
                let blob = upload(&mut worker_data, &bytes);
                (
                    json_obj(vec![
                        ("job", jstr(&job)),
                        ("outputs", Value::Arr(vec![json_obj(vec![
                            ("role", jstr("lod1_glb")),
                            ("tier", jstr("low")),
                            ("lod", Value::Int(1)),
                            ("media", jstr("glb")),
                            ("blob", jstr(&blob)),
                            ("byte_len", Value::Int(bytes.len() as i64)),
                        ])])),
                        ("metrics", json_obj(vec![
                            ("total_bytes", Value::Int(bytes.len() as i64)),
                            ("triangles", Value::Int(180)),
                            ("vertices", Value::Int(120)),
                        ])),
                    ]),
                    "lod",
                )
            }
            "derive.mesh_ao" => {
                let bytes = worker_bytes("AO-256", PACK_GLB);
                let blob = upload(&mut worker_data, &bytes);
                (
                    json_obj(vec![
                        ("job", jstr(&job)),
                        ("outputs", Value::Arr(vec![json_obj(vec![
                            ("role", jstr("ao_mesh")),
                            ("media", jstr("bin")),
                            ("blob", jstr(&blob)),
                            ("byte_len", Value::Int(bytes.len() as i64)),
                        ])])),
                        ("metrics", json_obj(vec![(
                            "total_bytes",
                            Value::Int(bytes.len() as i64),
                        )])),
                    ]),
                    "ao",
                )
            }
            "derive.image_resize" => {
                let bytes = worker_bytes("RESIZE-256", PACK_TEXTURE);
                let blob = upload(&mut worker_data, &bytes);
                (
                    json_obj(vec![
                        ("job", jstr(&job)),
                        ("outputs", Value::Arr(vec![json_obj(vec![
                            ("role", jstr("texture")),
                            ("media", jstr("png")),
                            ("blob", jstr(&blob)),
                            ("byte_len", Value::Int(bytes.len() as i64)),
                            ("dims", json_obj(vec![
                                ("width", Value::Int(256)),
                                ("height", Value::Int(256)),
                            ])),
                        ])])),
                        ("metrics", json_obj(vec![
                            ("total_bytes", Value::Int(bytes.len() as i64)),
                            ("max_texture_dim", Value::Int(256)),
                        ])),
                    ]),
                    "resize",
                )
            }
            other => panic!("unexpected kind {other}"),
        };
        let r = worker_ctl.post_json(&format!("/v1/derivations/{dkey}/complete"), &result);
        assert_eq!(r.status, 200, "{tag}: {}", String::from_utf8_lossy(&r.body));
        let variant = r.str_field("variant");
        // Record variants in the fixed recipe order, not claim order.
        let slot = dkeys.iter().position(|(t, _, _)| t == tag).unwrap();
        dkeys[slot].2 = variant;
        completed += 1;
    }
    // dkeys now holds (tag, dkey, variant) — record deterministically.
    for (_, dkey, variant) in &dkeys {
        ids.push(dkey.clone());
        ids.push(variant.clone());
    }

    // The cache answers ready now, and status agrees.
    let (_, thumb_dkey, thumb_variant) = &dkeys[0];
    let r = deriver_ctl.get(&format!("/v1/derivations/{thumb_dkey}"));
    assert_eq!(r.status, 200);
    let status = r.json();
    assert_eq!(status.get("state").unwrap().as_str().unwrap(), "ready");
    assert_eq!(status.get("variant").unwrap().as_str().unwrap(), thumb_variant);

    // Derived manifests inherit the registered terms exactly.
    let r = reader_ctl.get(&format!("/v1/derived-variants/{thumb_variant}"));
    assert_eq!(r.status, 200);
    let derived = DerivedVariantManifest::from_canonical_bytes(&r.body).unwrap();
    assert_eq!(derived.rights, kenney_terms());

    // Freeze the watchtower's three variants; idempotent by digest.
    let mut publisher_ctl = ts.control(Some(&publisher));
    let watchtower_variants: Vec<Value> = dkeys[..3].iter().map(|(_, _, v)| jstr(v)).collect();
    let freeze = json_obj(vec![
        ("base_asset", jstr(watchtower.get("asset_id").unwrap().as_str().unwrap())),
        ("base_revision", jstr(watchtower.get("revision").unwrap().as_str().unwrap())),
        ("variants", Value::Arr(watchtower_variants.clone())),
    ]);
    let r = publisher_ctl.post_json("/v1/variant-sets", &freeze);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let vset = r.str_field("variant_set");
    let r = publisher_ctl.post_json("/v1/variant-sets", &freeze);
    assert_eq!(r.status, 201);
    assert_eq!(r.str_field("variant_set"), vset);
    ids.push(vset.clone());
    // Reader cannot freeze.
    let r = reader_ctl.post_json("/v1/variant-sets", &freeze);
    assert_eq!(r.status, 403);
    assert_eq!(r.str_field("capability"), "asset_publish");

    // Deterministic profile resolution: three roles, stable digest.
    let resolve = json_obj(vec![
        ("variant_set", jstr(&vset)),
        ("profile", json_obj(vec![
            ("tier", jstr("high")),
            ("max_texture_dim", Value::Int(2048)),
            ("max_triangles", Value::Int(1_000_000)),
            ("max_variant_bytes", Value::Int(64 * 1024 * 1024)),
            ("accept", Value::Arr(vec![
                jstr("png"),
                jstr("jpeg"),
                jstr("glb"),
                jstr("bin"),
            ])),
        ])),
    ]);
    let r = reader_ctl.post_json("/v1/variant-resolutions", &resolve);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let map = r.json();
    let entries = map.get("entries").unwrap().as_arr().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].get("role").unwrap().as_str().unwrap(), "thumbnail");
    let digest = map.get("digest").unwrap().as_str().unwrap().to_string();
    let r2 = reader_ctl.post_json("/v1/variant-resolutions", &resolve);
    assert_eq!(r2.json().get("digest").unwrap().as_str().unwrap(), digest);
    ids.push(digest);

    (ts, ids)
}

fn upload(data: &mut Client, bytes: &[u8]) -> String {
    let r = data.post_bytes("/v1/blobs?ns=kenney", bytes);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    r.str_field("blob_id")
}

fn jstr(s_: &str) -> Value {
    Value::Str(s_.to_string())
}

fn json_obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

#[test]
fn licensed_pack_slice_end_to_end_and_deterministic_across_servers() {
    let (_a, ids_a) = run_slice("slice_a");
    let (_b, ids_b) = run_slice("slice_b");
    // Every identity — import revision, asset ids/revisions, derivation
    // keys, variant ids, the frozen set, and the resolved-map digest — is
    // byte-identical on a clean second server.
    assert_eq!(ids_a, ids_b);
    assert_eq!(ids_a.len(), 1 + 4 + 8 + 1 + 1);
}

#[test]
fn derive_recipes_advertise_resize_and_ao_and_require_auth() {
    let ts = start_server("derive_recipes");
    let admin = ts.admin_token();
    let mut control = ts.control(Some(&admin));
    let mut anon = ts.control(None);
    assert_eq!(anon.get("/v1/derive-recipes").status, 401);

    let r = control.get("/v1/derive-recipes");
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let recipes = r.json().get("recipes").unwrap().as_arr().unwrap().to_vec();
    assert_eq!(recipes.len(), 2);
    let kinds: Vec<&str> = recipes
        .iter()
        .map(|row| row.get("kind").unwrap().as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"derive.image_resize"));
    assert!(kinds.contains(&"derive.mesh_ao"));
    for row in &recipes {
        let hex = row.get("recipe").unwrap().as_str().unwrap();
        assert_eq!(hex.len() % 2, 0);
        assert!(hex.len() > 16);
        assert!(row.get("recipe_digest").unwrap().as_str().is_some());
    }
}

#[test]
fn derived_variant_lookup_refuses_missing_and_never_returns_original() {
    let ts = start_server("variant_lookup");
    let admin_tok = ts.admin_token();
    let mut admin = ts.control(Some(&admin_tok));
    let importer = principal_with(
        &mut admin,
        &[
            ("import_source", "kenney"),
            ("import_run", "kenney"),
            ("blob_write", "kenney"),
        ],
    );
    let mut importer_data = ts.data(Some(&importer));
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        assert_eq!(
            importer_data.post_bytes("/v1/blobs?ns=kenney", bytes).status,
            201
        );
    }
    let mut importer_ctl = ts.control(Some(&importer));
    let collection_bytes = kenney_collection().to_canonical_bytes().unwrap();
    assert_eq!(
        importer_ctl
            .request("PUT", "/v1/import-sources", &[], Some(&collection_bytes))
            .status,
        201
    );
    let manifest_bytes = kenney_pack("1.0").to_canonical_bytes().unwrap();
    let r = importer_ctl.request("POST", "/v1/imports", &[], Some(&manifest_bytes));
    assert_eq!(r.status, 201);
    let entries = r.json().get("entries").unwrap().as_arr().unwrap().to_vec();
    let texture = entries
        .iter()
        .find(|e| e.get("key").unwrap().as_str().unwrap() == "textures/hull-panel")
        .unwrap();
    let watchtower = entries
        .iter()
        .find(|e| e.get("key").unwrap().as_str().unwrap() == "models/watchtower")
        .unwrap();

    let reader = principal_with(&mut admin, &[]);
    let mut reader_ctl = ts.control(Some(&reader));
    let recipe = recipes()
        .into_iter()
        .find(|(tag, _)| *tag == "resize")
        .unwrap()
        .1;
    let body = json_obj(vec![
        (
            "base_asset",
            jstr(texture.get("asset_id").unwrap().as_str().unwrap()),
        ),
        (
            "base_revision",
            jstr(texture.get("revision").unwrap().as_str().unwrap()),
        ),
        ("recipe", jstr(&hex(&recipe.to_canonical_bytes().unwrap()))),
    ]);
    let r = reader_ctl.post_json("/v1/derived-variant-lookups", &body);
    assert_eq!(r.status, 404, "{}", String::from_utf8_lossy(&r.body));
    let err = r.json();
    assert_eq!(
        err.get("error").unwrap().as_str().unwrap(),
        "derived variant not ready"
    );
    // The original texture blob is not smuggled back as a substitute.
    assert!(err.get("blobs").is_none());
    assert_ne!(
        err.get("variant").and_then(Value::as_str),
        Some(texture.get("revision").unwrap().as_str().unwrap())
    );

    let ao = recipes()
        .into_iter()
        .find(|(tag, _)| *tag == "ao")
        .unwrap()
        .1;
    let ao_body = json_obj(vec![
        (
            "base_asset",
            jstr(watchtower.get("asset_id").unwrap().as_str().unwrap()),
        ),
        (
            "base_revision",
            jstr(watchtower.get("revision").unwrap().as_str().unwrap()),
        ),
        ("recipe", jstr(&hex(&ao.to_canonical_bytes().unwrap()))),
    ]);
    let r = reader_ctl.post_json("/v1/derived-variant-lookups", &ao_body);
    assert_eq!(r.status, 404);
    assert!(r.json().get("blobs").is_none());
}
