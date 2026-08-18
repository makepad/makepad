//! Idempotent importer for an existing ai-content library directory.
//!
//! Reads `index.json` (the app's persisted `{items:[{file,label,domain,
//! content_type,prompt,…}], next_id}` format) and the payload files —
//! NOTHING in the library directory is ever written or trusted for
//! previews. Per item:
//!
//! - text payloads are skipped,
//! - PNG publishes as `Texture` with its real pixel dimensions (contract-
//!   mandatory) and itself as the thumbnail,
//! - WAV publishes as `Audio` in the `sfx` category with a FRESHLY rendered
//!   canonical 512×512 waveform thumbnail (the on-disk `.thumb` sidecars
//!   are a known provenance bug — byte-copies of unrelated pipeline images
//!   — and are never read),
//! - GLB publishes as `Character` (skinned+animated) or `Mesh` with
//!   MEASURED vertex/triangle/joint/clip metadata; its thumbnail is the
//!   library's rendered `<file>.thumb` when that is a valid in-bounds PNG,
//!   else the GLB's embedded base-color image, else the honest placeholder,
//! - MP4 publishes as `Video` with measured duration + first-frame thumb.
//!
//! Identity/idempotency: the artifact digest derives BOTH the asset id
//! (first 16 digest bytes) and the stable two-segment alias
//! `<namespace>/history-<first 32 digest hex>`. A rerun resolves the alias
//! first and skips anything already published — same bytes, same alias, no
//! duplicates. Legacy provenance is typed-honest: the index records only
//! the prompt, so `manifest_provenance` stays `None` (never fabricated).

use makepad_asset_importer::glb::inspect_glb;
use makepad_asset_importer::thumbs::{
    encode_jpeg_bgra, jpeg_dims, parse_wav, placeholder_bgra_512, png_dims, waveform_bgra_512,
    THUMB_DIM,
};
use makepad_asset_importer::videothumb::probe_video;
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::util::{sanitize_text, to_hex};
use makepad_asset_client::{
    AssetClient, ClientError, PublishFile, PublishRequest, PublishRights, PublishStats,
    PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, BlobId, FileRole, MediaType, ThumbnailMedia,
};
use std::path::Path;
use std::str::FromStr;

/// Largest payload the importer will lift (library items are ≤ a few MB).
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ImportReport {
    pub published: Vec<(String, String)>,
    pub skipped_existing: Vec<String>,
    pub skipped_kind: Vec<String>,
    pub failed: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct IndexItem {
    pub(crate) file: String,
    pub(crate) label: String,
    pub(crate) domain: String,
    pub(crate) content_type: String,
    pub(crate) prompt: String,
}

pub(crate) fn parse_index(bytes: &[u8]) -> Result<Vec<IndexItem>, String> {
    let value = json::parse(bytes).map_err(|e| format!("index.json: {e}"))?;
    let items = value
        .get("items")
        .and_then(Value::as_arr)
        .ok_or("index.json: no items array")?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let text = |key: &str| {
            item.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let file = text("file");
        if file.is_empty()
            || file.contains('/')
            || file.contains('\\')
            || file.starts_with('.')
        {
            return Err(format!("index.json: refusing file name {file:?}"));
        }
        out.push(IndexItem {
            file,
            label: text("label"),
            domain: text("domain"),
            content_type: text("content_type"),
            prompt: text("prompt"),
        });
    }
    Ok(out)
}

/// Stable identities from the artifact digest. The alias's first segment is
/// deliberately the asset namespace: the catalog rejects aliases that point
/// across namespaces.
pub(crate) fn derived_identity(
    bytes: &[u8],
    namespace: &str,
) -> Result<(AssetId, AssetAlias), String> {
    let digest = BlobId::hash_of(bytes);
    let raw = digest.as_bytes();
    let asset = AssetId::from_bytes(raw[..16].try_into().expect("16 bytes"));
    let alias_text = format!("{namespace}/history-{}", &to_hex(raw)[..32]);
    let alias = AssetAlias::from_str(&alias_text)
        .map_err(|_| "namespace cannot form a catalog alias".to_string())?;
    Ok((asset, alias))
}

/// A candidate 512-class thumbnail image (PNG/JPEG bytes) if its declared
/// dimensions sit inside the content contract's 256..=4096 window.
fn usable_image_thumb(bytes: &[u8]) -> Option<(Vec<u8>, ThumbnailMedia, u32, u32)> {
    if let Some((w, h)) = png_dims(bytes) {
        if (256..=4096).contains(&w) && (256..=4096).contains(&h) {
            return Some((bytes.to_vec(), ThumbnailMedia::Png, w, h));
        }
    }
    if let Some((w, h)) = jpeg_dims(bytes) {
        if (256..=4096).contains(&w) && (256..=4096).contains(&h) {
            return Some((bytes.to_vec(), ThumbnailMedia::Jpeg, w, h));
        }
    }
    None
}

fn placeholder_thumb() -> Result<PublishThumbnail, String> {
    Ok(PublishThumbnail {
        bytes: encode_jpeg_bgra(&placeholder_bgra_512(), THUMB_DIM, THUMB_DIM)?,
        media: ThumbnailMedia::Jpeg,
        width: THUMB_DIM as u32,
        height: THUMB_DIM as u32,
    })
}

/// Import one library directory. Never writes into it.
pub fn import_library(
    client: &mut AssetClient,
    dir: &Path,
    namespace: &str,
    rights: &PublishRights,
    log: bool,
) -> Result<ImportReport, String> {
    let items = read_index(dir)?;
    Ok(import_items(client, dir, namespace, rights, &items, log))
}

/// Read one atomically committed library index snapshot. Watch mode treats
/// errors as transient (a foreign writer might not use the library's normal
/// rename protocol) and retries on its next bounded poll.
pub(crate) fn read_index(dir: &Path) -> Result<Vec<IndexItem>, String> {
    let index_bytes =
        std::fs::read(dir.join("index.json")).map_err(|e| format!("index.json: {e}"))?;
    parse_index(&index_bytes)
}

/// Import only the selected index rows. This is the continuous watcher's
/// new/changed-only seam; the one-shot importer simply passes every row.
pub(crate) fn import_items(
    client: &mut AssetClient,
    dir: &Path,
    namespace: &str,
    rights: &PublishRights,
    items: &[IndexItem],
    log: bool,
) -> ImportReport {
    let mut report = ImportReport::default();
    for item in items {
        let outcome = import_item(client, dir, namespace, rights, item);
        match outcome {
            ItemOutcome::Published(asset) => {
                if log {
                    eprintln!("[asset-worker] imported {} -> {asset}", item.file);
                }
                report.published.push((item.file.clone(), asset));
            }
            ItemOutcome::AlreadyPublished => {
                if log {
                    eprintln!("[asset-worker] skip (already published) {}", item.file);
                }
                report.skipped_existing.push(item.file.clone());
            }
            ItemOutcome::SkippedKind => {
                if log {
                    eprintln!("[asset-worker] skip (kind) {}", item.file);
                }
                report.skipped_kind.push(item.file.clone());
            }
            ItemOutcome::Failed(error) => {
                if log {
                    eprintln!("[asset-worker] FAILED {}: {error}", item.file);
                }
                report.failed.push((item.file.clone(), error));
            }
        }
    }
    report
}

enum ItemOutcome {
    Published(String),
    AlreadyPublished,
    SkippedKind,
    Failed(String),
}

fn import_item(
    client: &mut AssetClient,
    dir: &Path,
    namespace: &str,
    rights: &PublishRights,
    item: &IndexItem,
) -> ItemOutcome {
    let content_type = item.content_type.to_ascii_lowercase();
    let is_glb = item.file.ends_with(".glb") || content_type.contains("gltf");
    if content_type.starts_with("text/") || content_type == "application/json" {
        return ItemOutcome::SkippedKind;
    }
    let path = dir.join(&item.file);
    let (bytes, before) = match std::fs::metadata(&path).and_then(|meta| {
        if meta.len() > MAX_IMPORT_BYTES {
            return Err(std::io::Error::other("payload over import budget"));
        }
        let bytes = std::fs::read(&path)?;
        if bytes.len() as u64 != meta.len() {
            return Err(std::io::Error::other("payload changed while reading"));
        }
        Ok((bytes, meta))
    }) {
        Ok(snapshot) => snapshot,
        Err(error) => return ItemOutcome::Failed(error.to_string()),
    };
    if bytes.is_empty() {
        return ItemOutcome::Failed("empty payload".to_string());
    }

    // Idempotency: the digest-derived alias is the publication marker.
    let (asset_id, alias) = match derived_identity(&bytes, namespace) {
        Ok(identity) => identity,
        Err(error) => return ItemOutcome::Failed(error),
    };
    match client.resolve_alias(&alias) {
        Ok(_) => return ItemOutcome::AlreadyPublished,
        Err(ClientError::NotFound { .. }) => {}
        Err(error) => return ItemOutcome::Failed(format!("alias probe: {error}")),
    }

    let built = if content_type == "image/png" {
        build_png(item, bytes)
    } else if content_type.starts_with("audio/") {
        build_wav(item, bytes)
    } else if is_glb {
        build_glb(item, dir, bytes)
    } else if content_type.starts_with("video/") {
        build_video(item, &path, bytes)
    } else {
        return ItemOutcome::SkippedKind;
    };
    let mut request = match built {
        Ok(request) => request,
        Err(error) => return ItemOutcome::Failed(error),
    };
    request.namespace = namespace.to_string();
    request.asset_id = Some(asset_id);
    request.alias = Some(alias);
    request.prompt = item.prompt.clone();
    request.creator = "ai-content-library".to_string();
    // The operator's explicit declaration for this library — the index
    // format records no rights, and this importer NEVER invents any.
    request.rights = rights.clone();
    if !item.domain.is_empty() {
        request.tags.push(item.domain.clone());
    }
    // A writer that does not use the AI library's normal payload-then-index
    // commit order may still race this read/probe. Never publish a torn
    // snapshot: the watcher will observe the changed metadata and retry.
    match std::fs::metadata(&path) {
        Ok(after)
            if after.len() == before.len()
                && after.modified().ok() == before.modified().ok() => {}
        Ok(_) => return ItemOutcome::Failed("payload changed while importing".to_string()),
        Err(error) => return ItemOutcome::Failed(format!("payload recheck: {error}")),
    }
    // Legacy provenance is prompt-only: typed provenance stays honest-None.
    match client.publish_artifact(&request) {
        Ok(published) => ItemOutcome::Published(published.asset_id.to_string()),
        Err(error) => ItemOutcome::Failed(format!("publish: {error}")),
    }
}

fn title_of(item: &IndexItem) -> String {
    let source = if item.label.is_empty() { &item.file } else { &item.label };
    let title = sanitize_text(source, 120);
    if title.is_empty() { "Imported asset".to_string() } else { title }
}

fn build_png(item: &IndexItem, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let (width, height) = png_dims(&bytes).ok_or("png: malformed header")?;
    let thumbnail = match usable_image_thumb(&bytes) {
        Some((thumb, media, w, h)) => {
            PublishThumbnail { bytes: thumb, media, width: w, height: h }
        }
        None => placeholder_thumb()?,
    };
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Texture,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Png,
            role: FileRole::Texture,
            media_millis: 0,
            dims: Some((width, height)),
        },
        thumbnail,
    );
    request.categories = vec!["image".to_string()];
    Ok(request)
}

fn build_wav(item: &IndexItem, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let pcm = parse_wav(&bytes)?;
    // ALWAYS a fresh canonical waveform — the on-disk sidecars are stale.
    let strip = waveform_bgra_512(&pcm);
    let thumbnail = PublishThumbnail {
        bytes: encode_jpeg_bgra(&strip, THUMB_DIM, THUMB_DIM)?,
        media: ThumbnailMedia::Jpeg,
        width: THUMB_DIM as u32,
        height: THUMB_DIM as u32,
    };
    let millis = pcm.millis();
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Audio,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Wav,
            role: FileRole::Audio,
            media_millis: millis,
            dims: None,
        },
        thumbnail,
    );
    // VJ has deliberately separate long-form DJ tracks and one-shot pads.
    // The AI library's domain is authoritative for that behavior: Music3
    // writes `music`, while SA3/Woosh/MOSS write `audio` and remain SFX.
    request.categories = vec![audio_category(item).to_string()];
    Ok(request)
}

fn audio_category(item: &IndexItem) -> &'static str {
    if item.domain.eq_ignore_ascii_case("music") {
        "music"
    } else {
        "sfx"
    }
}

fn build_glb(item: &IndexItem, dir: &Path, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let stats = inspect_glb(&bytes)?;
    // Rendered library thumbnail when a VALID one exists; the importer only
    // reads it, never regenerates or writes sidecars.
    let rendered = std::fs::read(dir.join(format!("{}.thumb", item.file)))
        .ok()
        .and_then(|thumb| usable_image_thumb(&thumb));
    let thumbnail = match rendered.or_else(|| {
        stats.base_color.as_deref().and_then(usable_image_thumb)
    }) {
        Some((thumb, media, w, h)) => {
            PublishThumbnail { bytes: thumb, media, width: w, height: h }
        }
        None => placeholder_thumb()?,
    };
    let kind = if stats.skinned { AssetKind::Character } else { AssetKind::Mesh };
    let mut request = PublishRequest::new(
        "gen",
        kind,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Glb,
            role: FileRole::RenderGlb,
            media_millis: 0,
            dims: None,
        },
        thumbnail,
    );
    request.stats = PublishStats {
        triangles: stats.triangles,
        vertices: stats.vertices,
        joints: stats.joints,
        clips: stats.clips,
    };
    request.categories = vec![if stats.skinned { "dancer" } else { "prop" }.to_string()];
    Ok(request)
}

fn build_video(item: &IndexItem, path: &Path, bytes: Vec<u8>) -> Result<PublishRequest, String> {
    let probe = probe_video(path)?;
    let mut request = PublishRequest::new(
        "gen",
        AssetKind::Video,
        title_of(item),
        PublishFile {
            bytes,
            media: MediaType::Mp4,
            role: FileRole::Video,
            media_millis: probe.duration_ms,
            dims: None,
        },
        PublishThumbnail {
            bytes: probe.thumbnail_jpeg,
            media: ThumbnailMedia::Jpeg,
            width: THUMB_DIM as u32,
            height: THUMB_DIM as u32,
        },
    );
    request.categories = vec!["generated".to_string()];
    if !probe.real_frame {
        request.tags.push("no-preview-frame".to_string());
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mp_asset_import_{}_{}_{}",
            std::process::id(),
            n,
            name
        ))
    }

    #[test]
    fn identity_is_digest_stable_and_two_segments() {
        let (asset_a, alias_a) = derived_identity(b"payload one", "gen").unwrap();
        let (asset_b, alias_b) = derived_identity(b"payload one", "gen").unwrap();
        let (asset_c, alias_c) = derived_identity(b"payload two", "gen").unwrap();
        assert_eq!(asset_a, asset_b);
        assert_eq!(alias_a, alias_b);
        assert_ne!(asset_a, asset_c);
        assert_ne!(alias_a.as_str(), alias_c.as_str());
        let segments: Vec<&str> = alias_a.as_str().split('/').collect();
        assert_eq!(segments.len(), 2, "two-segment alias: {}", alias_a.as_str());
        assert_eq!(segments[0], "gen");
        assert!(segments[1].starts_with("history-"));
        assert_eq!(segments[1].len(), "history-".len() + 32);
        assert!(derived_identity(b"payload", "bad namespace").is_err());
    }

    #[test]
    fn index_parses_and_refuses_hostile_file_names() {
        let good = br#"{"items":[{"file":"lib-1.png","label":"a","domain":"image",
            "content_type":"image/png","prompt":"p"}],"next_id":2}"#;
        let items = parse_index(good).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file, "lib-1.png");
        for hostile in [
            br#"{"items":[{"file":"../../etc/passwd","content_type":"image/png"}]}"#.as_slice(),
            br#"{"items":[{"file":".hidden","content_type":"image/png"}]}"#.as_slice(),
        ] {
            assert!(parse_index(hostile).is_err());
        }
        assert!(parse_index(b"junk").is_err());
    }

    #[test]
    fn imported_titles_are_utf8_safe_and_byte_bounded() {
        let item = IndexItem {
            file: "fallback.png".to_string(),
            label: "é".repeat(100),
            domain: "image".to_string(),
            content_type: "image/png".to_string(),
            prompt: String::new(),
        };
        let title = title_of(&item);
        assert!(title.len() <= 120);
        assert!(title.is_char_boundary(title.len()));
        assert_eq!(title, "é".repeat(60));
    }

    #[test]
    fn audio_domain_routes_music_to_decks_and_other_audio_to_sfx() {
        let mut item = IndexItem {
            file: "track.wav".to_string(),
            label: "track".to_string(),
            domain: "music".to_string(),
            content_type: "audio/wav".to_string(),
            prompt: String::new(),
        };
        assert_eq!(audio_category(&item), "music");
        item.domain = "audio".to_string();
        assert_eq!(audio_category(&item), "sfx");
        item.domain = "speech".to_string();
        assert_eq!(audio_category(&item), "sfx");
    }

    #[test]
    fn importer_recovers_published_without_alias_then_skips_exact_rerun() {
        use makepad_asset_store::{AssetServer, ServerConfig};
        use makepad_asset_client::{ApiEndpoints, ClientConfig};

        let root = test_root("server");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(test_root("cache"));
        client_config.token = Some(token);
        let mut client = AssetClient::connect(
            client_config,
            ApiEndpoints {
                control: server.control_addr(),
                data: server.data_addr(),
            },
            Some(server.server_id()),
        )
        .expect("connect");

        let library = test_root("library");
        std::fs::create_dir_all(&library).unwrap();
        let mut png = vec![0u8; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&512u32.to_be_bytes());
        png[20..24].copy_from_slice(&512u32.to_be_bytes());
        std::fs::write(library.join("lib-1.png"), &png).unwrap();
        std::fs::write(
            library.join("index.json"),
            br#"{"items":[{"file":"lib-1.png","label":"Recovered PNG","domain":"image","content_type":"image/png","prompt":"test prompt"}],"next_id":2}"#,
        )
        .unwrap();

        let item = IndexItem {
            file: "lib-1.png".to_string(),
            label: "Recovered PNG".to_string(),
            domain: "image".to_string(),
            content_type: "image/png".to_string(),
            prompt: "test prompt".to_string(),
        };
        let (asset_id, alias) = derived_identity(&png, "gen").unwrap();
        let mut partial = build_png(&item, png).unwrap();
        partial.namespace = "gen".to_string();
        partial.asset_id = Some(asset_id);
        partial.prompt = item.prompt.clone();
        partial.creator = "ai-content-library".to_string();
        partial.tags.push(item.domain.clone());
        client
            .publish_artifact(&partial)
            .expect("land revision without the importer alias");
        assert!(matches!(
            client.resolve_alias(&alias),
            Err(ClientError::NotFound { .. })
        ));

        let recovered = import_library(
            &mut client,
            &library,
            "gen",
            &PublishRights::declared(
                "CC0-1.0",
                "",
                "",
                makepad_asset_data::Redistribution::Allowed,
                makepad_asset_data::DerivativePolicy::Allowed,
            ),
            false,
        )
        .unwrap();
        assert!(recovered.failed.is_empty(), "{:?}", recovered.failed);
        assert_eq!(recovered.published.len(), 1);
        let resolved = client.resolve_alias(&alias).expect("importer recovered alias");
        assert_eq!(resolved.asset_id, asset_id);

        let rerun = import_library(
            &mut client,
            &library,
            "gen",
            &PublishRights::declared(
                "CC0-1.0",
                "",
                "",
                makepad_asset_data::Redistribution::Allowed,
                makepad_asset_data::DerivativePolicy::Allowed,
            ),
            false,
        )
        .unwrap();
        assert!(rerun.failed.is_empty(), "{:?}", rerun.failed);
        assert_eq!(rerun.skipped_existing, vec!["lib-1.png".to_string()]);
        assert!(rerun.published.is_empty());
    }
}
