//! The asset UI is a thin client on top of the data in the asset store.
//!
//! Content is fetched from the server by digest and held only in RAM, under
//! a byte budget, evicted least-recently-used. Nothing here writes a file:
//! an app-owned copy is exactly what went stale under us — a fixed Doom map
//! sat in the catalog for hours while the viewer kept opening a local
//! `lib-13501.glb` from the night before.
//!
//! Staleness is impossible by construction: a blob is named by the hash of
//! its bytes, so a cached entry can never be the wrong content for its key,
//! and a new revision means a new digest, so a catalog event invalidates by
//! simply resolving the asset again.
//!
//! There is exactly ONE copy of a blob in memory, and it lives in the
//! client's budgeted, LRU-evicted RAM cache (`AssetClient::fetch_blob_bytes`
//! / `forget_blob` / `ram_cache_bytes`). This module deliberately keeps no
//! cache of its own — a second copy is a second thing to go wrong.

use makepad_asset_client::{dto::AssetDetailDto, AssetClient, ClientConfig};
use makepad_asset_data::{AssetId, BlobId, FileRole};

/// What a viewer needs to draw one store asset: which blob it came from
/// (the cache key and the staleness proof) and its bytes.
#[derive(Debug)]
pub struct StorePayload {
    pub blob: BlobId,
    pub bytes: Vec<u8>,
    /// The revision the bytes belong to, for the UI to show and to compare
    /// against later catalog events.
    pub revision: String,
}

/// Resolve an asset to its renderable payload and fetch it: detail → newest
/// PUBLISHED revision → that revision's manifest → the file role a viewer
/// can draw → blob bytes.
///
/// Fetching by digest is what makes this safe to cache: the server can hand
/// back the same bytes forever and they are still, provably, this revision's.
pub fn fetch_viewable(
    client: &mut AssetClient,
    asset: &AssetId,
    prefer: &[FileRole],
) -> Result<StorePayload, String> {
    let detail: AssetDetailDto = client
        .asset_detail(asset)
        .map_err(|e| format!("asset detail: {e}"))?;
    let head = detail
        .latest_published()
        .ok_or("asset has no published revision")?;
    // `fetch_asset_manifest` verifies the document against the revision id,
    // so the file list cannot belong to another revision.
    let manifest = client
        .fetch_asset_manifest(&head.revision)
        .map_err(|e| format!("revision manifest: {e}"))?;
    let file = prefer
        .iter()
        .find_map(|role| manifest.files.iter().find(|f| f.role == *role))
        .ok_or_else(|| {
            format!(
                "revision has no {:?} file (has {:?})",
                prefer,
                manifest.files.iter().map(|f| f.role).collect::<Vec<_>>()
            )
        })?;
    let bytes = fetch_blob(client, &file.blob, file.byte_len)?;
    Ok(StorePayload {
        blob: file.blob,
        bytes,
        revision: head.revision.to_string(),
    })
}

/// One blob, whole, by digest — verified against that digest by the client,
/// streamed into memory, never written to an app-owned file.
pub fn fetch_blob(client: &mut AssetClient, blob: &BlobId, byte_len: u64) -> Result<Vec<u8>, String> {
    client
        .fetch_blob_bytes(blob, Some(byte_len).filter(|n| *n > 0))
        .map_err(|e| format!("blob {blob}: {e}"))
}

/// What the Create surface needs to WORK with a catalog asset: the payload
/// materialised as a file, plus what it is. The path is the client's
/// verified cache object — digest-named, re-hashed before it is handed out
/// — so the tools that take a file (AO bake, rig, drag-out, decoders) can
/// keep taking one without the app ever owning a copy that could drift.
#[derive(Clone, Debug)]
pub struct StoreFile {
    pub blob: BlobId,
    pub path: std::path::PathBuf,
    pub role: FileRole,
    pub revision: String,
    /// The manifest's declared thumbnail views ([`ThumbnailMeta::views`]) —
    /// what the picture IS, carried WITH the picture so no consumer ever
    /// has to guess (or worse, measure). Empty for payload files and for
    /// revisions baked before the views contract.
    pub views: Vec<makepad_asset_data::ThumbnailView>,
}

/// Same resolution as [`fetch_viewable`] — detail → newest PUBLISHED
/// revision → manifest → preferred role — but materialised to a path
/// instead of read into memory.
pub fn materialize(
    client: &mut AssetClient,
    asset: &AssetId,
    prefer: &[FileRole],
) -> Result<StoreFile, String> {
    let (head, manifest) = head_manifest(client, asset)?;
    let file = prefer
        .iter()
        .find_map(|role| manifest.files.iter().find(|f| f.role == *role))
        .ok_or_else(|| {
            format!(
                "revision has no {:?} file (has {:?})",
                prefer,
                manifest.files.iter().map(|f| f.role).collect::<Vec<_>>()
            )
        })?;
    let path = client
        .blob_path(&file.blob, Some(file.byte_len).filter(|n| *n > 0))
        .map_err(|e| format!("blob {}: {e}", file.blob))?;
    Ok(StoreFile { blob: file.blob, path, role: file.role, revision: head, views: Vec::new() })
}

/// The asset's own thumbnail, materialised the same way. Small, and the
/// only thing a gallery row needs before anyone clicks it — a rail must not
/// pull whole payloads to draw a wall of cards.
pub fn materialize_thumbnail(
    client: &mut AssetClient,
    asset: &AssetId,
) -> Result<StoreFile, String> {
    let (head, manifest) = head_manifest(client, asset)?;
    let thumbnail = manifest
        .thumbnail
        .ok_or("revision has no thumbnail")?;
    let path = client
        .blob_path(&thumbnail.blob, Some(thumbnail.byte_len).filter(|n| *n > 0))
        .map_err(|e| format!("thumbnail {}: {e}", thumbnail.blob))?;
    Ok(StoreFile {
        blob: thumbnail.blob,
        path,
        role: FileRole::Texture,
        revision: head,
        views: thumbnail.views,
    })
}

/// Detail → newest published revision → its verified manifest.
fn head_manifest(
    client: &mut AssetClient,
    asset: &AssetId,
) -> Result<(String, makepad_asset_data::AssetManifest), String> {
    let detail: AssetDetailDto = client
        .asset_detail(asset)
        .map_err(|e| format!("asset detail: {e}"))?;
    let head = detail
        .latest_published()
        .ok_or("asset has no published revision")?;
    let manifest = client
        .fetch_asset_manifest(&head.revision)
        .map_err(|e| format!("revision manifest: {e}"))?;
    Ok((head.revision.to_string(), manifest))
}

/// Roles a viewer can draw, best first.
pub fn default_viewable_roles() -> Vec<FileRole> {
    vec![
        FileRole::RenderGlb,
        FileRole::Splat,
        FileRole::Texture,
        FileRole::Albedo,
        FileRole::Audio,
    ]
}

/// Client for the session a viewer request carries. Connecting is cheap
/// against the embedded server, but a cached one keeps the keep-alive pool
/// warm across opens.
pub fn connect(session: &crate::import::ServerSession, cache: &std::path::Path) -> Option<AssetClient> {
    let mut config = ClientConfig::new(cache.to_path_buf());
    config.token = Some(session.token.clone());
    AssetClient::connect(config, session.endpoints, Some(session.server_id)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_client::{ApiEndpoints, PublishFile, PublishRequest, PublishThumbnail};
    use makepad_asset_data::{AssetKind, MediaType, ThumbnailMedia};
    use std::str::FromStr;

    fn test_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mp_store_content_{}_{name}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A REAL asset server in-process, with a real client publishing real
    /// bytes — the same path the app uses, no mocks.
    fn live_server() -> (makepad_asset_store::AssetServer, String, ApiEndpoints) {
        let root = test_root("server");
        let mut cfg = makepad_asset_store::ServerConfig::new(root.clone());
        cfg.control_addr = "127.0.0.1:0".parse().unwrap();
        cfg.data_addr = "127.0.0.1:0".parse().unwrap();
        cfg.bootstrap_admin = true;
        cfg.log = false;
        cfg.gc_janitor_steps = 0;
        cfg.gc_grace_ms = 0;
        let server = makepad_asset_store::AssetServer::start(cfg).expect("server start");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let endpoints = ApiEndpoints {
            control: server.control_addr(),
            data: server.data_addr(),
        };
        (server, token, endpoints)
    }

    fn client_for(token: &str, endpoints: ApiEndpoints, server_id: [u8; 16]) -> AssetClient {
        let mut cfg = ClientConfig::new(test_root("client-cache"));
        cfg.token = Some(token.to_string());
        AssetClient::connect(cfg, endpoints, Some(server_id)).expect("client connect")
    }

    /// Publish a drawable asset: a PNG under the `Texture` role (a kind
    /// with no mesh invariants — this test is about RESOLUTION, not about
    /// what a valid world manifest looks like). `asset` re-publishes an
    /// existing id as a NEW revision.
    fn publish_drawable(
        client: &mut AssetClient,
        alias: &str,
        body: &[u8],
        asset: Option<AssetId>,
    ) -> AssetId {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(body);
        let mut request = PublishRequest::new(
            "gen",
            AssetKind::Texture,
            "store_content drawable",
            PublishFile {
                bytes: png,
                media: MediaType::Png,
                role: FileRole::Texture,
                media_millis: 0,
                dims: Some((512, 512)),
            },
            PublishThumbnail {
                bytes: vec![0x5A; 1_024],
                media: ThumbnailMedia::Png,
                width: 512,
                height: 512,
                views: Vec::new(),
            },
        );
        request.alias = Some(makepad_asset_data::AssetAlias::from_str(alias).unwrap());
        request.asset_id = asset;
        match client.publish_artifact(&request) {
            Ok(p) => p.asset_id,
            Err(e) => panic!("publish {alias} (revision of {asset:?}): {e}"),
        }
    }

    /// One live server, the whole contract: resolve → draw → re-publish →
    /// the NEXT open shows the new bytes. (One server per test process: the
    /// data root takes a process lock, so these scenarios share it.)
    #[test]
    fn the_viewer_reads_the_catalog_and_never_an_older_copy() {
        let (server, token, endpoints) = live_server();
        let mut client = client_for(&token, endpoints, server.server_id());

        // 1. A published world resolves to its drawable file.
        let asset = publish_drawable(&mut client, "gen/store-content-one", b"-first", None);
        let first =
            fetch_viewable(&mut client, &asset, &default_viewable_roles()).expect("viewable");
        assert!(first.bytes.starts_with(b"\x89PNG"), "the drawable file came back");
        assert!(first.bytes.ends_with(b"-first"));
        assert_eq!(
            first.blob,
            makepad_asset_data::BlobId::hash_of(&first.bytes),
            "bytes are exactly the blob they were named by"
        );
        assert!(!first.revision.is_empty());

        // 2. Re-publishing the SAME asset makes the next open show the new
        //    mesh — the app holds nothing that can be older than the catalog.
        let same = publish_drawable(&mut client, "gen/store-content-one", b"-new-and-fixed", Some(asset));
        assert_eq!(same, asset, "re-publish is a new revision of one asset");
        let second = fetch_viewable(&mut client, &asset, &default_viewable_roles()).unwrap();
        assert!(
            second.bytes.ends_with(b"-new-and-fixed"),
            "re-opening after a publish shows the NEW content"
        );
        assert_ne!(second.blob, first.blob, "a new revision is a new digest");
        assert_ne!(second.revision, first.revision);

        // 3. Two assets never bleed into each other.
        let other = publish_drawable(&mut client, "gen/store-content-two", b"-other", None);
        let other_payload =
            fetch_viewable(&mut client, &other, &default_viewable_roles()).unwrap();
        assert!(other_payload.bytes.ends_with(b"-other"));

        // 4. An asset with no file the viewer can draw says so, by name.
        let err = fetch_viewable(&mut client, &asset, &[FileRole::ShadowSdf]).unwrap_err();
        assert!(err.contains("ShadowSdf"), "{err}");

        // 5. The single RAM copy stays inside its budget.
        let (used, budget) = client.ram_cache_bytes();
        assert!(used <= budget && budget > 0, "{used} / {budget}");
    }

    #[test]
    fn viewable_roles_are_ordered_best_first() {
        // The order the viewer asks for is the order it can DRAW: a mesh
        // before a still, a still before audio. A world published with both
        // a render mesh and a preview texture must open as the mesh.
        let prefer = default_viewable_roles();
        let index = |role: FileRole| prefer.iter().position(|r| *r == role);
        assert!(index(FileRole::RenderGlb) < index(FileRole::Texture));
        assert!(index(FileRole::Splat) < index(FileRole::Texture));
        assert!(index(FileRole::Texture) < index(FileRole::Audio));
        assert!(index(FileRole::RenderGlb).is_some());
    }
}
