//! End-to-end service tests over real sockets: publication lifecycle,
//! verified blob serving with conditionals and ranges, search + annotations,
//! games, discovery, restart and shutdown.

mod common;

use common::*;
use makepad_asset_client::{Api, ApiEndpoints, HttpLimits};
use makepad_asset_store::discovery::DiscoveryListener;
use makepad_asset_store::json::Value;
use makepad_asset_store::{AssetServer, DiscoveryConfig};
use makepad_asset_data::{AssetId, AssetRevisionId, AssetRevisionRef, BlobId, GameId};

#[test]
fn health_bootstrap_and_whoami() {
    let ts = start_server("health");
    // Health requires no credentials and reports the real schema versions.
    let mut anon = ts.control(None);
    let r = anon.get("/v1/health");
    assert_eq!(r.status, 200);
    assert_eq!(r.json().get("status").unwrap().as_str(), Some("ok"));
    assert!(r.json().get("schema_version").unwrap().as_i64().unwrap() >= 1);
    assert!(r.json().get("limits").unwrap().get("max_blob_bytes").is_some());
    // The identity handshake fields the asset client pins against beacons.
    assert_eq!(r.json().get("protocol_version").unwrap().as_i64(), Some(1));
    assert_eq!(r.str_field("server_id").len(), 32);
    // Health is symmetric on the data plane.
    let r = ts.data(None).get("/v1/health");
    assert_eq!(r.status, 200);
    // Everything else is not anonymous.
    assert_eq!(anon.get("/v1/auth/whoami").status, 401);
    // The bootstrap admin token from <root>/admin-token authenticates.
    let token = ts.admin_token();
    assert!(token.starts_with("mpat_"));
    let mut admin = ts.control(Some(&token));
    let r = admin.get("/v1/auth/whoami");
    assert_eq!(r.status, 200);
    assert!(r.str_field("principal").starts_with("prin_"));
    // Unknown paths and wrong methods refuse cleanly.
    assert_eq!(admin.get("/v1/nothing").status, 404);
    assert_eq!(admin.post_json("/v1/health", &jobj(vec![])).status, 405);
}

#[test]
fn assets_query_accepts_select_refuses_writes_and_requires_auth() {
    let ts = start_server("assets_query");
    let admin_token = ts.admin_token();
    let mut admin = ts.control(Some(&admin_token));
    let reader_token = principal_with(&mut admin, &[]);
    let mut reader = ts.control(Some(&reader_token));

    let select = jobj(vec![("sql", jstr("SELECT 7 AS answer"))]);
    let r = reader.post_json("/v1/assets/query", &select);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let result = r.json();
    assert_eq!(
        result.get("columns").and_then(Value::as_arr).unwrap()[0].as_str(),
        Some("answer")
    );
    assert_eq!(
        result.get("rows").and_then(Value::as_arr).unwrap()[0]
            .as_arr()
            .unwrap()[0]
            .as_str(),
        Some("7")
    );
    assert_eq!(result.get("truncated").and_then(Value::as_bool), Some(false));

    let api = Api::new(
        ApiEndpoints {
            control: ts.server.control_addr(),
            data: ts.server.data_addr(),
        },
        HttpLimits::default_v1(),
        Some(reader_token.clone()),
    )
    .unwrap();
    let typed = api.assets_query("SELECT 8 AS typed_answer").unwrap();
    assert_eq!(typed.columns, vec!["typed_answer"]);
    assert_eq!(typed.rows, vec![vec!["8"]]);

    let limited = jobj(vec![
        (
            "sql",
            jstr("SELECT name FROM principals ORDER BY name"),
        ),
        ("limit", Value::Int(1)),
    ]);
    let r = reader.post_json("/v1/assets/query", &limited);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json().get("rows").and_then(Value::as_arr).unwrap().len(), 1);
    assert_eq!(r.json().get("truncated").and_then(Value::as_bool), Some(true));
    assert_eq!(
        reader
            .post_json(
                "/v1/assets/query",
                &jobj(vec![
                    ("sql", jstr("SELECT 1")),
                    ("limit", Value::Int(201)),
                ]),
            )
            .status,
        400
    );

    let write = jobj(vec![("sql", jstr("INSERT INTO assets DEFAULT VALUES"))]);
    assert_eq!(reader.post_json("/v1/assets/query", &write).status, 400);

    let mut anonymous = ts.control(None);
    assert_eq!(anonymous.post_json("/v1/assets/query", &select).status, 401);
}

#[test]
fn blob_roundtrip_conditionals_and_ranges() {
    let ts = start_server("blobs");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let writer_token = principal_with(&mut admin, &[("blob_write", "demo")]);
    let mut data = ts.data(Some(&writer_token));

    let payload: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
    let blob_id = BlobId::hash_of(&payload);

    // Upload, then dedup on re-upload.
    let r = data.post_bytes("/v1/blobs?ns=demo", &payload);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.str_field("blob_id"), blob_id.to_string());
    assert_eq!(r.json().get("deduped").unwrap().as_bool(), Some(false));
    let r = data.post_bytes("/v1/blobs?ns=demo", &payload);
    assert_eq!(r.json().get("deduped").unwrap().as_bool(), Some(true));

    // Pre-declared digest must match the streamed bytes.
    let hex = blob_id.to_string().replace("sha256:", "");
    let r = data.request(
        "POST",
        &format!("/v1/blobs?ns=demo&sha256={hex}"),
        &[],
        Some(&payload),
    );
    assert_eq!(r.status, 201);
    let wrong = "0".repeat(64);
    let r = data.request(
        "POST",
        &format!("/v1/blobs?ns=demo&sha256={wrong}"),
        &[],
        Some(&payload),
    );
    assert_eq!(r.status, 422);

    // Full read, digest-verified server-side, with a strong ETag.
    let target = format!("/v1/blobs/{blob_id}");
    let r = data.get(&target);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, payload);
    let etag = r.header("ETag").expect("etag").to_string();
    assert_eq!(etag, format!("\"{blob_id}\""));
    assert_eq!(r.header("Accept-Ranges"), Some("bytes"));

    // HEAD: identical metadata, no body.
    let r = data.head(&target);
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Length"), Some("2000"));
    assert!(r.body.is_empty());

    // If-None-Match -> 304 with no body.
    let r = data.request("GET", &target, &[("If-None-Match", &etag)], None);
    assert_eq!(r.status, 304);
    assert!(r.body.is_empty());

    // Byte ranges: bounded, suffix, open-ended, over-long clamps.
    let cases: &[(&str, u16, &[u8])] = &[
        ("bytes=100-199", 206, &payload[100..200]),
        ("bytes=-100", 206, &payload[1900..]),
        ("bytes=1900-", 206, &payload[1900..]),
        ("bytes=1990-4000", 206, &payload[1990..]),
        ("bytes=0-0", 206, &payload[0..1]),
    ];
    for (range, status, expect) in cases {
        let r = data.request("GET", &target, &[("Range", range)], None);
        assert_eq!(r.status, *status, "range {range}");
        assert_eq!(&r.body, expect, "range {range}");
        assert!(r.header("Content-Range").unwrap().starts_with("bytes "));
    }
    let r = data.request("GET", &target, &[("Range", "bytes=100-199")], None);
    assert_eq!(r.header("Content-Range"), Some("bytes 100-199/2000"));

    // Unsatisfiable start -> 416 with the total size.
    let r = data.request("GET", &target, &[("Range", "bytes=2000-")], None);
    assert_eq!(r.status, 416);
    assert_eq!(r.header("Content-Range"), Some("bytes */2000"));

    // Multi-range and malformed ranges are ignored: full 200.
    for bad in ["bytes=0-1,5-6", "bytes=a-b", "bites=0-1"] {
        let r = data.request("GET", &target, &[("Range", bad)], None);
        assert_eq!(r.status, 200, "range {bad}");
        assert_eq!(r.body, payload);
    }

    // If-Range: matching validator keeps the range; anything else serves full.
    let r = data.request(
        "GET",
        &target,
        &[("Range", "bytes=0-9"), ("If-Range", &etag)],
        None,
    );
    assert_eq!(r.status, 206);
    assert_eq!(r.body, &payload[0..10]);
    let r = data.request(
        "GET",
        &target,
        &[("Range", "bytes=0-9"), ("If-Range", "\"sha256:ffff\"")],
        None,
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.body, payload);

    // Unknown digest is a 404; malformed one a 400.
    let missing = BlobId::hash_of(b"never uploaded");
    assert_eq!(data.get(&format!("/v1/blobs/{missing}")).status, 404);
    assert_eq!(data.get("/v1/blobs/sha256:zzzz").status, 400);
}

#[test]
fn asset_publication_manifest_alias_thumbnail() {
    let ts = start_server("publish");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let publisher = principal_with(
        &mut admin,
        &[
            ("blob_write", "demo"),
            ("asset_register", "demo"),
            ("asset_publish", "demo"),
            ("alias_write", "demo"),
        ],
    );
    let mut control = ts.control(Some(&publisher));
    let mut data = ts.data(Some(&publisher));

    let glb = b"glTF-not-really-but-bytes".to_vec();
    let thumb = b"\x89PNG-thumb-bytes".to_vec();
    for b in [&glb, &thumb] {
        assert_eq!(data.post_bytes("/v1/blobs?ns=demo", b).status, 201);
    }

    // Register + stage; the revision id is the canonical manifest digest.
    let r = control.post_json("/v1/assets", &jobj(vec![("namespace", jstr("demo"))]));
    assert_eq!(r.status, 201);
    let asset_id = r.str_field("asset_id");
    let ast: AssetId = asset_id.parse().unwrap();
    let manifest_bytes = prop_manifest(ast, &glb, &thumb).to_canonical_bytes().unwrap();
    let expect_rev = AssetRevisionId::hash_of(&manifest_bytes);
    let r = control.post_bytes(&format!("/v1/assets/{asset_id}/revisions"), &manifest_bytes);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let revision = r.str_field("revision");
    assert_eq!(revision, expect_rev.to_string());

    // Candidate state reads back.
    let r = control.get(&format!("/v1/assets/{asset_id}/revisions/{revision}"));
    assert_eq!(r.str_field("state"), "staged");

    // Staging under a different asset id is a consistency refusal.
    let other = control.post_json("/v1/assets", &jobj(vec![("namespace", jstr("demo"))]));
    let other_id = other.str_field("asset_id");
    let r = control.post_bytes(&format!("/v1/assets/{other_id}/revisions"), &manifest_bytes);
    assert_eq!(r.status, 409);

    // Manifest raw bytes round-trip with immutable caching semantics.
    let r = control.get(&format!("/v1/revisions/{revision}"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body, manifest_bytes);
    let etag = r.header("ETag").unwrap().to_string();
    let r = control.request(
        "GET",
        &format!("/v1/revisions/{revision}"),
        &[("If-None-Match", &etag)],
        None,
    );
    assert_eq!(r.status, 304);
    // And the browsing projection.
    let r = control.get(&format!("/v1/revisions/{revision}/json"));
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v.get("kind").unwrap().as_str(), Some("prop"));
    assert_eq!(v.get("asset_id").unwrap().as_str(), Some(asset_id.as_str()));

    // Aliases may only point at published revisions.
    let alias_target = jobj(vec![
        ("asset_id", jstr(asset_id.clone())),
        ("revision", jstr(revision.clone())),
    ]);
    let r = control.put_json("/v1/aliases/demo/props/crate", &alias_target);
    assert_eq!(r.status, 409, "alias to staged must refuse");
    let r = control.post_json(
        &format!("/v1/assets/{asset_id}/revisions/{revision}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("state"), "published");
    // Publish is idempotent.
    let r = control.post_json(
        &format!("/v1/assets/{asset_id}/revisions/{revision}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 200);
    let r = control.put_json("/v1/aliases/demo/props/crate", &alias_target);
    assert_eq!(r.status, 200);
    let r = control.get("/v1/aliases/demo/props/crate");
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("asset_id"), asset_id);
    assert_eq!(r.str_field("head_revision"), revision);

    // Thumbnails serve the manifest's typed thumbnail with its own ETag.
    let r = data.get("/v1/thumbnails/alias/demo/props/crate");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, thumb);
    assert_eq!(r.header("Content-Type"), Some("image/png"));
    let r = data.get(&format!("/v1/thumbnails/revision/{revision}"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body, thumb);
    let r = data.request(
        "GET",
        &format!("/v1/thumbnails/revision/{revision}"),
        &[("Range", "bytes=0-3")],
        None,
    );
    assert_eq!(r.status, 206);
    assert_eq!(r.body, thumb[0..4]);

    // Alias delete reports whether it existed.
    let r = control.delete("/v1/aliases/demo/props/crate");
    assert_eq!(r.json().get("existed").unwrap().as_bool(), Some(true));
    let r = control.delete("/v1/aliases/demo/props/crate");
    assert_eq!(r.json().get("existed").unwrap().as_bool(), Some(false));
}

#[test]
fn data_source_publishes_without_thumbnail_or_mesh() {
    let ts = start_server("publish_data");
    let token = ts.admin_token();
    let mut control = ts.control(Some(&token));
    let mut data = ts.data(Some(&token));
    let source = b"dataset: city-boundaries-v1\nformat: geojson\n";

    assert_eq!(data.post_bytes("/v1/blobs?ns=demo", source).status, 201);
    let r = control.post_json("/v1/assets", &jobj(vec![("namespace", jstr("demo"))]));
    assert_eq!(r.status, 201);
    let asset_id = r.str_field("asset_id");
    let asset: AssetId = asset_id.parse().unwrap();
    let manifest = data_manifest(asset, source);
    assert!(!manifest.kind.has_mesh());
    assert!(manifest.thumbnail.is_none());
    let bytes = manifest.to_canonical_bytes().expect("Source/Text-only Data manifest validates");

    let r = control.post_bytes(&format!("/v1/assets/{asset_id}/revisions"), &bytes);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let revision = r.str_field("revision");
    let r = control.post_json(
        &format!("/v1/assets/{asset_id}/revisions/{revision}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));

    let r = control.get(&format!("/v1/revisions/{revision}/json"));
    assert_eq!(r.status, 200);
    assert_eq!(r.json().get("kind").and_then(Value::as_str), Some("data"));
    assert_eq!(data.get(&format!("/v1/thumbnails/revision/{revision}")).status, 404);

    let annotation = jobj(vec![("title", jstr("City boundaries")), ("kind", jstr("data"))]);
    assert_eq!(
        control.put_json(&format!("/v1/assets/{asset_id}/annotation"), &annotation).status,
        204
    );
    let r = control.get("/v1/search?ns=demo&kind=data");
    assert_eq!(r.status, 200);
    let page = r.json();
    assert_eq!(page.get("total").and_then(Value::as_i64), Some(1));
    let hit = &page.get("hits").and_then(Value::as_arr).unwrap()[0];
    assert_eq!(hit.get("kind").and_then(Value::as_str), Some("data"));
}

#[test]
fn quarantine_pulls_content_transactionally() {
    let ts = start_server("quarantine");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let publisher = principal_with(
        &mut admin,
        &[
            ("blob_write", "demo"),
            ("asset_register", "demo"),
            ("asset_publish", "demo"),
            ("asset_quarantine", "demo"),
            ("alias_write", "demo"),
        ],
    );
    let mut control = ts.control(Some(&publisher));
    let mut data = ts.data(Some(&publisher));
    let (asset_id, revision) =
        publish_prop_http(&mut control, &mut data, "demo", "demo/props/pulled", b"glb-q", b"thumb-q");

    assert_eq!(control.get("/v1/aliases/demo/props/pulled").status, 200);
    let r = control.post_json(
        &format!("/v1/assets/{asset_id}/revisions/{revision}/quarantine"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("state"), "quarantined");

    // The alias head dropped in the same transaction; revision-addressed
    // reads refuse; the candidate row says why.
    assert_eq!(control.get("/v1/aliases/demo/props/pulled").status, 404);
    assert_eq!(control.get(&format!("/v1/revisions/{revision}")).status, 404);
    assert_eq!(control.get(&format!("/v1/revisions/{revision}/json")).status, 404);
    assert_eq!(data.get(&format!("/v1/thumbnails/revision/{revision}")).status, 404);
    let r = control.get(&format!("/v1/assets/{asset_id}/revisions/{revision}"));
    assert_eq!(r.str_field("state"), "quarantined");

    // Quarantine is terminal: publish refuses, re-staging refuses.
    let r = control.post_json(
        &format!("/v1/assets/{asset_id}/revisions/{revision}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 409);
    let manifest_bytes = prop_manifest(asset_id.parse().unwrap(), b"glb-q", b"thumb-q")
        .to_canonical_bytes()
        .unwrap();
    let r = control.post_bytes(&format!("/v1/assets/{asset_id}/revisions"), &manifest_bytes);
    assert_eq!(r.status, 409);
}

#[test]
fn game_lifecycle_refs_and_aliases() {
    let ts = start_server("games");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let publisher = principal_with(
        &mut admin,
        &[
            ("blob_write", "demo"),
            ("asset_register", "demo"),
            ("asset_publish", "demo"),
            ("alias_write", "demo"),
            ("game_register", "demo"),
            ("game_publish", "demo"),
        ],
    );
    let mut control = ts.control(Some(&publisher));
    let mut data = ts.data(Some(&publisher));

    // Two published assets to pin.
    let (a1, r1) = publish_prop_http(&mut control, &mut data, "demo", "demo/props/one", b"glb1", b"th1");
    let (a2, r2) = publish_prop_http(&mut control, &mut data, "demo", "demo/props/two", b"glb2", b"th2");
    let ref1 = AssetRevisionRef { asset_id: a1.parse().unwrap(), revision: r1.parse().unwrap() };
    let ref2 = AssetRevisionRef { asset_id: a2.parse().unwrap(), revision: r2.parse().unwrap() };

    // Game blobs + canonical documents.
    let splash = b"splash-source".to_vec();
    let toml = b"[game]\nname=\"t\"".to_vec();
    let thumb = b"png-game-thumb".to_vec();
    let r = control.post_json("/v1/games", &jobj(vec![("namespace", jstr("demo"))]));
    assert_eq!(r.status, 201);
    let game_id = r.str_field("game_id");
    let gam: GameId = game_id.parse().unwrap();
    let lock = lock_for(gam, &[("demo/props/one", ref1), ("demo/props/two", ref2)]);
    for b in [&splash, &toml, &lock, &thumb] {
        assert_eq!(data.post_bytes("/v1/blobs?ns=demo", b).status, 201);
    }
    let manifest = game_manifest(gam, &splash, &toml, &lock, &thumb)
        .to_canonical_bytes()
        .unwrap();

    // Malformed framing refuses before touching the catalog.
    let r = control.post_bytes(&format!("/v1/games/{game_id}/revisions"), b"tooshort");
    assert_eq!(r.status, 400);
    let r = control.post_bytes(&format!("/v1/games/{game_id}/revisions"), &framed(&manifest, &lock));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let revision = r.str_field("revision");

    // The pinned refs read back in canonical order.
    let r = control.get(&format!("/v1/games/{game_id}/revisions/{revision}/refs"));
    assert_eq!(r.status, 200);
    let refs = r.json();
    let refs = refs.get("refs").unwrap().as_arr().unwrap().to_vec();
    assert_eq!(refs.len(), 2);
    let got: Vec<String> = refs
        .iter()
        .map(|v| v.get("asset_id").unwrap().as_str().unwrap().to_string())
        .collect();
    // Canonical ref order is by asset-id BYTES; display strings are base32
    // and do not sort identically, so compare through the parsed ids.
    let mut want: Vec<AssetId> = vec![a1.parse().unwrap(), a2.parse().unwrap()];
    want.sort();
    let want: Vec<String> = want.iter().map(|a| a.to_string()).collect();
    assert_eq!(got, want, "refs come back sorted by asset id bytes");

    // Publish, then alias.
    let r = control.post_json(
        &format!("/v1/games/{game_id}/revisions/{revision}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 200);
    let r = control.put_json(
        "/v1/game-aliases/demo/games/arena",
        &jobj(vec![
            ("game_id", jstr(game_id.clone())),
            ("revision", jstr(revision.clone())),
        ]),
    );
    assert_eq!(r.status, 200);
    let r = control.get("/v1/game-aliases/demo/games/arena");
    assert_eq!(r.str_field("game_id"), game_id);
    assert_eq!(r.str_field("head_revision"), revision);
    let r = control.delete("/v1/game-aliases/demo/games/arena");
    assert_eq!(r.json().get("existed").unwrap().as_bool(), Some(true));

    // A lock pinning an unpublished revision refuses at stage time.
    let r = control.post_json("/v1/games", &jobj(vec![("namespace", jstr("demo"))]));
    let game2 = r.str_field("game_id");
    let gam2: GameId = game2.parse().unwrap();
    let manifest_bytes2 = prop_manifest(AssetId::from_bytes([9u8; 16]), b"glb3", b"th3");
    // (staged but never published)
    assert_eq!(data.post_bytes("/v1/blobs?ns=demo", b"glb3").status, 201);
    assert_eq!(data.post_bytes("/v1/blobs?ns=demo", b"th3").status, 201);
    let r = control.post_json(
        "/v1/assets",
        &jobj(vec![
            ("namespace", jstr("demo")),
            ("asset_id", jstr(AssetId::from_bytes([9u8; 16]).to_string())),
        ]),
    );
    assert_eq!(r.status, 201);
    let mb = manifest_bytes2.to_canonical_bytes().unwrap();
    let staged_rev = control
        .post_bytes(&format!("/v1/assets/{}/revisions", AssetId::from_bytes([9u8; 16])), &mb)
        .str_field("revision");
    let bad_ref = AssetRevisionRef {
        asset_id: AssetId::from_bytes([9u8; 16]),
        revision: staged_rev.parse().unwrap(),
    };
    let lock2 = lock_for(gam2, &[("demo/props/three", bad_ref)]);
    let manifest2 = game_manifest(gam2, &splash, &toml, &lock2, &thumb)
        .to_canonical_bytes()
        .unwrap();
    assert_eq!(data.post_bytes("/v1/blobs?ns=demo", &lock2).status, 201);
    let r = control.post_bytes(&format!("/v1/games/{game2}/revisions"), &framed(&manifest2, &lock2));
    assert_eq!(r.status, 409, "{}", String::from_utf8_lossy(&r.body));
}

#[test]
fn search_annotations_visibility_and_cursors() {
    let ts = start_server("search");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let owner_token = principal_with(
        &mut admin,
        &[
            ("blob_write", "demo"),
            ("asset_register", "demo"),
            ("asset_publish", "demo"),
            ("alias_write", "demo"),
        ],
    );
    let other_token = principal_with(&mut admin, &[("asset_register", "demo")]);
    let mut owner = ts.control(Some(&owner_token));
    let mut other = ts.control(Some(&other_token));
    let mut data = ts.data(Some(&owner_token));

    let (asset_id, _rev) =
        publish_prop_http(&mut owner, &mut data, "demo", "demo/props/launcher", b"glb-s", b"th-s");

    // Private annotation with an owner-only prompt.
    let ann = jobj(vec![
        ("title", jstr("Rocket Launcher")),
        ("description", jstr("A fancy launcher of rockets")),
        ("kind", jstr("prop")),
        ("tags", Value::Arr(vec![jstr("weapon"), jstr("rocket")])),
        ("prompt", jstr("zebra striped launcher concept")),
        ("visibility", jstr("private")),
    ]);
    assert_eq!(owner.put_json(&format!("/v1/assets/{asset_id}/annotation"), &ann).status, 204);

    // Owner finds it; another principal sees nothing at all.
    let r = owner.get("/v1/search?q=rocket");
    assert_eq!(r.status, 200);
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(1));
    let hit = r.json().get("hits").unwrap().as_arr().unwrap()[0].clone();
    assert_eq!(hit.get("title").unwrap().as_str(), Some("Rocket Launcher"));
    assert_eq!(hit.get("live").unwrap().as_bool(), Some(true));
    let r = other.get("/v1/search?q=rocket");
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(0));
    // The private annotation read is indistinguishable from absence.
    assert_eq!(other.get(&format!("/v1/assets/{asset_id}/annotation")).status, 404);
    let r = owner.get(&format!("/v1/assets/{asset_id}/annotation"));
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("prompt"), "zebra striped launcher concept");

    // Public visibility: found by others, but prompt terms and the prompt
    // field itself stay owner-only.
    let mut public_ann = ann.clone();
    if let Value::Obj(pairs) = &mut public_ann {
        for (k, v) in pairs.iter_mut() {
            if k == "visibility" {
                *v = jstr("public");
            }
        }
    }
    assert_eq!(owner.put_json(&format!("/v1/assets/{asset_id}/annotation"), &public_ann).status, 204);
    let r = other.get("/v1/search?q=rocket");
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(1));
    assert_eq!(other.get("/v1/search?q=zebra").json().get("total").unwrap().as_i64(), Some(0));
    assert_eq!(owner.get("/v1/search?q=zebra").json().get("total").unwrap().as_i64(), Some(1));
    let r = other.get(&format!("/v1/assets/{asset_id}/annotation"));
    assert_eq!(r.status, 200);
    assert!(r.json().get("prompt").is_none(), "prompt is owner-only");
    assert!(r.json().get("owner").is_none());

    // Only the owner (or root) may replace or delete an owned annotation.
    assert_eq!(other.put_json(&format!("/v1/assets/{asset_id}/annotation"), &public_ann).status, 403);
    assert_eq!(other.delete(&format!("/v1/assets/{asset_id}/annotation")).status, 403);

    // POST search accepts free text; browse mode lists by filters only.
    let r = owner.post_json("/v1/catalog", &jobj(vec![("q", jstr("rocket launcher"))]));
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(1));
    let r = owner.get("/v1/search?ns=demo&kind=prop");
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(1));
    let r = owner.get("/v1/search?tag=rocket");
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(1));
    let r = owner.get("/v1/search?ns=elsewhere");
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(0));

    // Server-side tag exclusion, same wire shape as `tag` on both forms.
    // The asset carries tags [weapon, rocket].
    for (tag, total) in [("rocket", 0i64), ("weapon", 0), ("nosuch", 1)] {
        let r = owner.post_json(
            "/v1/catalog",
            &jobj(vec![("q", jstr("rocket launcher")), ("exclude_tag", jstr(tag))]),
        );
        assert_eq!(r.status, 200, "exclude_tag {tag}");
        assert_eq!(r.json().get("total").unwrap().as_i64(), Some(total), "exclude_tag {tag}");
    }
    // The exclusion beats a matching positive tag on the same request.
    let r = owner.post_json(
        "/v1/catalog",
        &jobj(vec![
            ("tag", jstr("weapon")),
            ("exclude_tag", jstr("rocket")),
        ]),
    );
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(0));
    let r = owner.get("/v1/search?tag=weapon&exclude_tag=rocket");
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(0));
    let r = owner.get("/v1/search?exclude_tag=nosuch");
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(1));
    // Facets travel on both request forms and are absent unless asked for.
    // The asset carries kind `prop` and tags [weapon, rocket].
    let r = owner.get("/v1/search?q=rocket");
    assert_eq!(
        r.json().get("facets").unwrap().as_arr().map(<[Value]>::len),
        Some(0),
        "no facets unless the query asks"
    );
    let r = owner.get("/v1/search?q=rocket&facets=8");
    let facets = r.json().get("facets").unwrap().as_arr().unwrap().to_vec();
    let mut seen: Vec<(String, String, i64)> = facets
        .iter()
        .map(|f| {
            (
                f.get("kind").unwrap().as_str().unwrap().to_string(),
                f.get("label").unwrap().as_str().unwrap().to_string(),
                f.get("count").unwrap().as_i64().unwrap(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("tag".to_string(), "rocket".to_string(), 1),
            ("tag".to_string(), "weapon".to_string(), 1),
        ]
    );
    let r = owner.post_json(
        "/v1/catalog",
        &jobj(vec![("q", jstr("rocket launcher")), ("facets", Value::Int(8))]),
    );
    assert_eq!(r.json().get("facets").unwrap().as_arr().map(<[Value]>::len), Some(2));
    // A facet count over the budget is clamped, not refused, exactly like
    // `limit`; a malformed one is a 400.
    assert_eq!(owner.get("/v1/search?q=rocket&facets=100000").status, 200);
    assert_eq!(
        owner
            .post_json(
                "/v1/catalog",
                &jobj(vec![("q", jstr("rocket")), ("facets", jstr("many"))])
            )
            .status,
        400
    );

    // Bad values refuse with 400, byte-for-byte like a bad `tag`.
    for bad in ["", "Bad Tag", "bad\u{7}"] {
        let a = owner.post_json("/v1/catalog", &jobj(vec![("tag", jstr(bad))]));
        let b = owner.post_json("/v1/catalog", &jobj(vec![("exclude_tag", jstr(bad))]));
        assert_eq!((a.status, b.status), (400, 400), "tag/exclude_tag {bad:?}");
    }
    // A non-string value is a malformed search field.
    let r = owner.post_json("/v1/catalog", &jobj(vec![("exclude_tag", Value::Int(1))]));
    assert_eq!(r.status, 400);

    // Keyset pagination: three annotated assets, one per page, cursor bound
    // to the exact query shape.
    for i in 0..2u8 {
        let (aid, _)= publish_prop_http(
            &mut owner,
            &mut data,
            "demo",
            &format!("demo/props/extra-{i}"),
            format!("glb-x{i}").as_bytes(),
            format!("th-x{i}").as_bytes(),
        );
        let a = jobj(vec![("title", jstr(format!("Extra {i}"))), ("visibility", jstr("public"))]);
        assert_eq!(owner.put_json(&format!("/v1/assets/{aid}/annotation"), &a).status, 204);
    }
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let target = match &cursor {
            None => "/v1/search?ns=demo&limit=1".to_string(),
            Some(c) => format!("/v1/search?ns=demo&limit=1&cursor={c}"),
        };
        let r = owner.get(&target);
        assert_eq!(r.status, 200);
        let v = r.json();
        for h in v.get("hits").unwrap().as_arr().unwrap() {
            seen.push(h.get("asset_id").unwrap().as_str().unwrap().to_string());
        }
        match v.get("cursor").unwrap().as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 3);
    // A cursor replayed against a different query shape is stale.
    let probe = cursor_probe(&mut owner);
    let stale = owner.get(&format!("/v1/search?limit=1&cursor={probe}"));
    assert_eq!(stale.status, 400);
}

/// Over the wire, `q=` widens and `exact=1` does not — with annotations
/// shaped like the ones the live catalog holds.
#[test]
fn search_expands_synonyms_unless_the_request_asks_for_exact_words() {
    let ts = start_server("search_synonyms");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let owner_token = principal_with(
        &mut admin,
        &[
            ("blob_write", "demo"),
            ("asset_register", "demo"),
            ("asset_publish", "demo"),
            ("alias_write", "demo"),
        ],
    );
    let mut owner = ts.control(Some(&owner_token));
    let mut data = ts.data(Some(&owner_token));

    for (i, (title, description)) in [
        ("Dog", "dog pet; standalone; 1x1; brown/grey; cube-shaped dog with floppy ears"),
        ("Race Car", "race car vehicle; standalone; 2x1; red; sports styling"),
        ("Blaster Rifle", "blaster rifle weapon; standalone; handheld; grey"),
    ]
    .iter()
    .enumerate()
    {
        let (asset_id, _rev) = publish_prop_http(
            &mut owner,
            &mut data,
            "demo",
            &format!("demo/props/syn-{i}"),
            format!("glb-syn{i}").as_bytes(),
            format!("th-syn{i}").as_bytes(),
        );
        let ann = jobj(vec![
            ("title", jstr(*title)),
            ("description", jstr(*description)),
            ("kind", jstr("prop")),
            ("visibility", jstr("public")),
        ]);
        assert_eq!(owner.put_json(&format!("/v1/assets/{asset_id}/annotation"), &ann).status, 204);
    }
    let total = |c: &mut Client, target: &str| -> i64 {
        let r = c.get(target);
        assert_eq!(r.status, 200, "{target}");
        r.json().get("total").unwrap().as_i64().unwrap()
    };

    // The words nothing in the catalog says.
    assert_eq!(total(&mut owner, "/v1/search?q=puppy"), 1);
    assert_eq!(total(&mut owner, "/v1/search?q=automobile"), 1);
    assert_eq!(total(&mut owner, "/v1/search?q=gun"), 1);
    // The `-` join is still a conjunction, each term through its own group.
    assert_eq!(total(&mut owner, "/v1/search?q=red-sports-automobile"), 1);
    assert_eq!(total(&mut owner, "/v1/search?q=puppy-automobile"), 0);

    // `exact=1` is the escape hatch, on GET and on POST.
    assert_eq!(total(&mut owner, "/v1/search?q=puppy&exact=1"), 0);
    assert_eq!(total(&mut owner, "/v1/search?q=dog&exact=1"), 1);
    let r = owner.post_json(
        "/v1/catalog",
        &jobj(vec![("q", jstr("puppy")), ("exact", Value::Bool(true))]),
    );
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(0));
    let r = owner.post_json("/v1/catalog", &jobj(vec![("q", jstr("puppy"))]));
    assert_eq!(r.json().get("total").unwrap().as_i64(), Some(1));
    // A malformed flag refuses like every other flag.
    assert_eq!(owner.get("/v1/search?q=dog&exact=maybe").status, 400);
    assert_eq!(
        owner
            .post_json("/v1/catalog", &jobj(vec![("q", jstr("dog")), ("exact", Value::Int(1))]))
            .status,
        400
    );
}

/// Fetch a fresh cursor for one query shape (used to prove shape binding).
fn cursor_probe(owner: &mut Client) -> String {
    let r = owner.get("/v1/search?ns=demo&limit=1");
    r.json().get("cursor").unwrap().as_str().unwrap().to_string()
}

#[test]
fn discovery_beacon_and_listener() {
    // Listener first: its ephemeral port becomes the beacon target.
    let mut listener = DiscoveryListener::start(0, 10_000).expect("listener");
    let port = listener.port();
    let ts = start_server_with("discovery", |cfg| {
        cfg.discovery = Some(DiscoveryConfig {
            port,
            target_ip: "127.0.0.1".parse().unwrap(),
            interval_ms: 50,
            capability_bits: makepad_asset_store::discovery::caps::ALL_V1,
        });
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let found = loop {
        let now = makepad_asset_store::util::now_ms();
        let snap = listener.snapshot(now);
        if let Some(srv) = snap.iter().find(|s| s.server_id == ts.server.server_id()) {
            break srv.clone();
        }
        assert!(std::time::Instant::now() < deadline, "no beacon received");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    // Endpoints derive from the beacon ports and the SENDER address.
    assert_eq!(found.control_port, ts.server.control_addr().port());
    assert_eq!(found.data_port, ts.server.data_addr().port());
    assert!(found.auth_required);
    assert_eq!(found.ip.to_string(), "127.0.0.1");
    // The discovered endpoint answers health.
    let mut probe = Client::new(found.control_endpoint(), None);
    assert_eq!(probe.get("/v1/health").status, 200);
    listener.stop();
}

#[test]
fn shutdown_restart_and_root_lock() {
    let ts = start_server("restart");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let publisher = principal_with(
        &mut admin,
        &[
            ("blob_write", "demo"),
            ("asset_register", "demo"),
            ("asset_publish", "demo"),
            ("alias_write", "demo"),
        ],
    );
    let mut control = ts.control(Some(&publisher));
    let mut data = ts.data(Some(&publisher));
    let (asset_id, revision) =
        publish_prop_http(&mut control, &mut data, "demo", "demo/props/durable", b"glb-r", b"th-r");

    // A second server on the SAME root must refuse while this one lives.
    let mut cfg = base_config(ts.root.clone());
    cfg.bootstrap_admin = false;
    assert!(AssetServer::start(cfg).is_err(), "root lock must exclude a second server");

    let control_addr = ts.server.control_addr();
    let root = ts.root.clone();
    drop(ts.server);

    // All ports are really closed after shutdown.
    assert!(std::net::TcpStream::connect(control_addr).is_err());

    // Restart on the same root: durable state (catalog, alias, CAS, tokens)
    // is intact and the admin token file still authenticates.
    let mut cfg = base_config(root);
    cfg.bootstrap_admin = true;
    let server2 = AssetServer::start(cfg).expect("restart");
    let mut c2 = Client::new(server2.control_addr(), Some(&publisher));
    let r = c2.get("/v1/aliases/demo/props/durable");
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("asset_id"), asset_id);
    assert_eq!(r.str_field("head_revision"), revision);
    let mut a2 = Client::new(server2.control_addr(), Some(&token));
    assert_eq!(a2.get("/v1/auth/whoami").status, 200);
}
