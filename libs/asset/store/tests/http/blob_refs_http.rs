//! Reference blobs over the wire: the real client publishing a real file it
//! never reads, against a real server on a real socket.
//!
//! What this proves that the core-level suite cannot:
//! - the route DOES NOT EXIST unless the embedder turned the policy on —
//!   an untouched server answers 404, so no deployment gains a file-read
//!   surface by upgrading,
//! - a bundle published by reference produces a manifest that a normal
//!   client fetches normally: same digest, same length, same bytes,
//! - the store's directory gained no copy of the payload,
//! - and when the file underneath changes, the fetch REFUSES rather than
//!   serving whatever is there now.

use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClientConfig, ClientError, PublishBundle, PublishBundleFile,
    PublishRights, PublishThumbnail,
};
use makepad_asset_data::{AssetKind, BlobId, FileRole, MediaType, ThumbnailMedia};
use makepad_asset_store::{AssetServer, BlobRefPolicy, ServerConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mp_blobref_http_{}_{}_{}", std::process::id(), n, name))
}

fn start_server(name: &str, refs_on: bool) -> (AssetServer, String, PathBuf) {
    let root = test_root(name);
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    if refs_on {
        cfg.blob_refs = BlobRefPolicy::local_host();
    }
    let server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    (server, token, root)
}

fn connect(server: &AssetServer, token: &str, cache: &str) -> AssetClient {
    let mut cfg = ClientConfig::new(test_root(cache));
    cfg.token = Some(token.to_string());
    let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
    AssetClient::connect(cfg, endpoints, Some(server.server_id())).expect("connect")
}

fn thumbnail() -> PublishThumbnail {
    PublishThumbnail {
        bytes: vec![9u8; 1200],
        media: ThumbnailMedia::Jpeg,
        width: 512,
        height: 512,
        views: Vec::new(),
    }
}

/// Count regular files under a directory — used to show the CAS did not grow.
fn file_count(root: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn reference_admission_is_absent_until_an_embedder_enables_it() {
    let (server, token, _root) = start_server("policy_off", false);
    let client = connect(&server, &token, "policy_off_cache");
    let dir = test_root("policy_off_media");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clip.mp4");
    std::fs::write(&path, b"UNREACHABLE").unwrap();

    match client.admit_blob_ref("gen", path.to_str().unwrap()) {
        Err(ClientError::NotFound { .. }) => {}
        other => panic!("a server with the policy off must not offer the route: {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_bundle_published_by_reference_fetches_like_any_other() {
    let (server, token, root) = start_server("publish_ref", true);
    let mut client = connect(&server, &token, "publish_ref_cache");

    // The user's media directory, entirely outside the store.
    let media = test_root("publish_ref_media");
    std::fs::create_dir_all(&media).unwrap();
    let clip: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    let path = media.join("opener.mp4");
    std::fs::write(&path, &clip).unwrap();

    let cas_before = file_count(&root.join("cas"));

    let mut bundle = PublishBundle::new(
        "gen",
        AssetKind::Video,
        "Opener",
        vec![PublishBundleFile::reference(
            FileRole::Video,
            MediaType::Mp4,
            path.clone(),
            None,
        )],
        thumbnail(),
        PublishRights::generated_cc0(),
    );
    bundle.media_millis = 4000;
    let published = client.publish_bundle(&bundle).expect("publish by reference");

    // The manifest pins the file's REAL digest and length, measured by the
    // server on a file the client never opened.
    assert_eq!(published.files.len(), 1);
    assert_eq!(published.files[0].blob, BlobId::hash_of(&clip));
    assert_eq!(published.files[0].byte_len, clip.len() as u64);

    // The CAS gained exactly ONE object — the thumbnail. The 40 KB payload
    // was not copied.
    let cas_after = file_count(&root.join("cas"));
    assert_eq!(
        cas_after - cas_before,
        1,
        "the payload should not have been copied into the CAS"
    );

    // And it fetches over the wire like anything else, digest-verified by
    // the client's own cache on the way in.
    let fetched = client.fetch_blob_bytes(&published.files[0].blob, None).expect("fetch");
    assert_eq!(fetched, clip);

    // Break the file underneath: the store refuses rather than serving
    // whatever is at that path now.
    let mut tampered = clip.clone();
    tampered[0] ^= 0xff;
    std::fs::write(&path, &tampered).unwrap();
    let mut fresh = connect(&server, &token, "publish_ref_cache2");
    match fresh.fetch_blob_bytes(&published.files[0].blob, None) {
        Ok(_) => panic!("a drifted reference must never serve"),
        Err(_) => {}
    }

    // Restore, and it works again: the reference was unavailable, not wrong.
    std::fs::write(&path, &clip).unwrap();
    let mut again = connect(&server, &token, "publish_ref_cache3");
    assert_eq!(
        again.fetch_blob_bytes(&published.files[0].blob, None).expect("restored"),
        clip
    );

    std::fs::remove_dir_all(&media).ok();
}

#[test]
fn the_prefix_allowlist_is_enforced() {
    let root = test_root("allowlist");
    let allowed = test_root("allowlist_allowed");
    let forbidden = test_root("allowlist_forbidden");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&forbidden).unwrap();
    std::fs::write(allowed.join("ok.mp4"), b"INSIDE").unwrap();
    std::fs::write(forbidden.join("no.mp4"), b"OUTSIDE").unwrap();

    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    cfg.blob_refs = BlobRefPolicy { roots: vec![allowed.clone()], ..BlobRefPolicy::local_host() };
    let server = AssetServer::start(cfg).expect("server start");
    let token = std::fs::read_to_string(root.join("admin-token")).unwrap().trim().to_string();
    let client = connect(&server, &token, "allowlist_cache");

    client
        .admit_blob_ref("gen", allowed.join("ok.mp4").to_str().unwrap())
        .expect("a path under an allowed root is admitted");

    assert!(
        client
            .admit_blob_ref("gen", forbidden.join("no.mp4").to_str().unwrap())
            .is_err(),
        "a path outside every allowed root must be refused"
    );

    // …including one that tries to walk out of an allowed root.
    let escape = allowed.join("..").join("allowlist_forbidden").join("no.mp4");
    assert!(
        client.admit_blob_ref("gen", escape.to_str().unwrap()).is_err(),
        "a traversal out of an allowed root must be refused"
    );

    std::fs::remove_dir_all(&allowed).ok();
    std::fs::remove_dir_all(&forbidden).ok();
}

#[test]
fn rescan_over_http_reports_what_went_stale() {
    let (server, token, _root) = start_server("rescan_http", true);
    let client = connect(&server, &token, "rescan_http_cache");
    let media = test_root("rescan_http_media");
    std::fs::create_dir_all(&media).unwrap();
    let good = media.join("good.mp4");
    let gone = media.join("gone.mp4");
    std::fs::write(&good, b"STILL-HERE").unwrap();
    std::fs::write(&gone, b"NOT-FOR-LONG").unwrap();
    client.admit_blob_ref("gen", good.to_str().unwrap()).unwrap();
    client.admit_blob_ref("gen", gone.to_str().unwrap()).unwrap();
    std::fs::remove_file(&gone).unwrap();

    let page = client.blob_refs_page(None, 32).expect("rescan");
    assert_eq!(page.total, 2);
    assert_eq!(page.refs.len(), 2);
    let state_of = |name: &str| {
        page.refs
            .iter()
            .find(|r| r.path.ends_with(name))
            .map(|r| r.state.clone())
            .unwrap_or_default()
    };
    assert_eq!(state_of("good.mp4"), "present");
    assert_eq!(state_of("gone.mp4"), "missing");

    std::fs::remove_dir_all(&media).ok();
}
