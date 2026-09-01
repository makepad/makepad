//! The real game Asset Client against an in-process real Asset Server for the
//! licensed import route family. No fixture protocol and no prebuilt binary:
//! this exercises the same library server started by the standalone process.

mod common;

use common::{kenney_collection, kenney_pack, start_server, PACK_COLLIDER, PACK_GLB, PACK_PREVIEW, PACK_TEXTURE};
use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig};
use makepad_asset_data::{ImportRevisionId, SourceCollection};

fn connect(ts: &common::TestServer, token: &str, name: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(common::test_root(name));
    cfg.token = Some(token.to_string());
    AssetClient::connect(
        cfg,
        ApiEndpoints {
            control: ts.server.control_addr(),
            data: ts.server.data_addr(),
        },
        Some(ts.server.server_id()),
    )
    .expect("real client connects to real in-process server")
}

#[test]
fn real_client_import_roundtrip_and_bounded_source_pages() {
    let ts = start_server("real_import_client");
    let token = ts.admin_token();
    let client = connect(&ts, &token, "real_import_client_cache");

    // Populate enough canonically ordered collections to prove that the
    // continuation is the exact last source identity, not an offset or an
    // implementation-specific database token.
    for (id, title) in [
        ("alpha", "Alpha assets"),
        ("bravo", "Bravo assets"),
        ("charlie", "Charlie assets"),
    ] {
        let mut collection: SourceCollection = kenney_collection();
        collection.id = id.into();
        collection.title = title.into();
        client
            .register_source_collection(&collection.to_canonical_bytes().unwrap())
            .expect("register source through real client");
    }
    let collection_bytes = kenney_collection().to_canonical_bytes().unwrap();
    client
        .register_source_collection(&collection_bytes)
        .expect("register import source through real client");

    // Existing clients remain compatible: the original no-query method sees
    // the complete small first page and ignores the additive cursor field.
    let listed = client.list_source_collections().expect("legacy list projection");
    assert_eq!(
        listed.iter().map(|row| row.source_id.as_str()).collect::<Vec<_>>(),
        vec!["alpha", "bravo", "charlie", "kenney"]
    );

    let mut raw = ts.control(Some(&token));
    let first = raw.get("/v1/import-sources?limit=2");
    assert_eq!(first.status, 200);
    let first = first.json();
    let first_rows = first.get("sources").unwrap().as_arr().unwrap();
    assert_eq!(first_rows.len(), 2);
    assert_eq!(first_rows[0].get("source_id").unwrap().as_str(), Some("alpha"));
    assert_eq!(first_rows[1].get("source_id").unwrap().as_str(), Some("bravo"));
    assert_eq!(first.get("cursor").unwrap().as_str(), Some("bravo"));

    let second = raw.get("/v1/import-sources?cursor=bravo&limit=2");
    assert_eq!(second.status, 200);
    let second = second.json();
    let second_rows = second.get("sources").unwrap().as_arr().unwrap();
    assert_eq!(second_rows.len(), 2);
    assert_eq!(second_rows[0].get("source_id").unwrap().as_str(), Some("charlie"));
    assert_eq!(second_rows[1].get("source_id").unwrap().as_str(), Some("kenney"));
    assert!(matches!(second.get("cursor"), Some(makepad_asset_store::json::Value::Null)));

    // Paging inputs fail closed rather than becoming an unbounded fallback.
    assert_eq!(raw.get("/v1/import-sources?limit=0").status, 400);
    assert_eq!(raw.get("/v1/import-sources?cursor=NotCanonical&limit=2").status, 400);

    // The actual import uses only public client methods over real sockets.
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        client.upload_blob("kenney", bytes).expect("upload import blob");
    }
    let manifest = kenney_pack("1.0");
    let manifest_bytes = manifest.to_canonical_bytes().unwrap();
    let expected = ImportRevisionId::hash_of(&manifest_bytes);
    let first = client.run_import(&manifest_bytes).expect("run real import");
    assert!(first.created);
    assert_eq!(first.import_revision, expected);
    assert_eq!(first.entries.len(), manifest.assets.len());

    let replay = client.run_import(&manifest_bytes).expect("idempotent import replay");
    assert!(!replay.created);
    assert_eq!(replay.import_revision, first.import_revision);
    assert_eq!(replay.entries, first.entries);

    let status = client.import_status(&expected).expect("real import status");
    assert_eq!(status.import_revision, expected);
    assert_eq!(status.source_id, "kenney");
    assert_eq!(status.entries, first.entries);
}

#[test]
fn legacy_unpaged_source_list_refuses_above_its_existing_client_ceiling() {
    let ts = start_server("legacy_source_bound");
    let token = ts.admin_token();
    let mut raw = ts.control(Some(&token));

    // 513 is the one-row lookahead for the legacy 512-row contract. The
    // storage query itself is capped at exactly this many rows; the route does
    // not materialize the rest of the table before deciding to refuse.
    for i in 0..513 {
        let mut collection = kenney_collection();
        collection.id = format!("source{i:04}");
        collection.title = format!("Source {i:04}");
        let bytes = collection.to_canonical_bytes().unwrap();
        let response = raw.request("PUT", "/v1/import-sources", &[], Some(&bytes));
        assert_eq!(
            response.status,
            201,
            "source {i}: {}",
            String::from_utf8_lossy(&response.body)
        );
    }

    let legacy = raw.get("/v1/import-sources");
    assert_eq!(legacy.status, 413);

    // Explicit paging remains usable over the same dataset and carries the
    // exact last emitted source id as its continuation.
    let page = raw.get("/v1/import-sources?limit=500");
    assert_eq!(page.status, 200);
    let page = page.json();
    assert_eq!(page.get("sources").unwrap().as_arr().unwrap().len(), 500);
    assert_eq!(page.get("cursor").unwrap().as_str(), Some("source0499"));
}

/// The client-driven derivation protocol end to end over real HTTP (aicore
/// P7): POST /v1/derivations answers the CALLER with the deterministic job
/// identity, the caller runs the kernel, uploads the output blob, and posts
/// the completion under that identity — no queue, no worker claim loop
/// anywhere. This is the one generation-era protocol the store kept, so it
/// gets a route-level proof, not just the core-seam one in variants.rs.
#[test]
fn client_driven_derivation_completes_over_http() {
    use common::{jobj, jstr, publish_prop_http, PACK_PREVIEW};
    use makepad_asset_data::{
        sha256, BlobId, ProcessingRecipe, RecipeSettings, ThumbnailMedia, ToolClosure,
        OUTPUT_SCHEMA_V1,
    };

    let ts = start_server("derive_http");
    let token = ts.admin_token();
    let mut control = ts.control(Some(&token));
    let mut data = ts.data(Some(&token));

    let (asset_id, revision) =
        publish_prop_http(&mut control, &mut data, "gen", "gen/derive-base", PACK_GLB, PACK_PREVIEW);

    let recipe = ProcessingRecipe {
        settings: RecipeSettings::MeshThumbnail {
            width: 512,
            height: 512,
            media: ThumbnailMedia::Png,
        },
        tool: ToolClosure {
            processor: "mp_derive".into(),
            version: "1.0".into(),
            build: "deadbeef".into(),
            deterministic: true,
        },
        output_schema: OUTPUT_SCHEMA_V1,
    };
    let recipe_hex: String = recipe
        .to_canonical_bytes()
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // Request: the answer is "pending" with OUR job identity — the store
    // armed nothing, and a second request joins the same in-flight row.
    let r = control.post_json(
        "/v1/derivations",
        &jobj(vec![
            ("base_asset", jstr(asset_id.clone())),
            ("base_revision", jstr(revision.clone())),
            ("recipe", jstr(recipe_hex.clone())),
        ]),
    );
    assert_eq!(r.status, 202, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("status"), "pending");
    let dkey = r.str_field("dkey");
    let job = r.str_field("job");
    let r2 = control.post_json(
        "/v1/derivations",
        &jobj(vec![
            ("base_asset", jstr(asset_id.clone())),
            ("base_revision", jstr(revision.clone())),
            ("recipe", jstr(recipe_hex)),
        ]),
    );
    assert_eq!(r2.status, 202);
    assert_eq!(r2.str_field("job"), job, "one in-flight identity, joined");

    // The caller runs the kernel and uploads the product bytes itself.
    let mut thumb = b"THUMB-512:".to_vec();
    thumb.extend_from_slice(&sha256(PACK_GLB));
    let r = data.post_bytes("/v1/blobs?ns=gen", &thumb);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));

    // Completion under the answered job identity publishes the variant.
    let r = control.post_json(
        &format!("/v1/derivations/{dkey}/complete"),
        &jobj(vec![
            ("job", jstr(job)),
            (
                "thumbnail",
                jobj(vec![
                    ("blob", jstr(BlobId::hash_of(&thumb).to_string())),
                    ("media", jstr("png")),
                    ("width", makepad_asset_store::json::Value::Int(512)),
                    ("height", makepad_asset_store::json::Value::Int(512)),
                    (
                        "byte_len",
                        makepad_asset_store::json::Value::Int(thumb.len() as i64),
                    ),
                ]),
            ),
            (
                "metrics",
                jobj(vec![(
                    "total_bytes",
                    makepad_asset_store::json::Value::Int(thumb.len() as i64),
                )]),
            ),
        ]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let variant = r.str_field("variant");
    assert!(!variant.is_empty());

    // The derivation row is READY and names the same variant.
    let r = control.get(&format!("/v1/derivations/{dkey}"));
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("state"), "ready");
    assert_eq!(r.str_field("variant"), variant);
}
