//! The fully-local content path, end to end, with no UI.
//!
//! This is the VJ's standalone mode reduced to its load-bearing parts, so it
//! can be RUN and BELIEVED on a machine with no window: host the Asset Server
//! in-process on loopback, walk a directory of media, catalogue every file
//! WITHOUT COPYING it, and then read the bytes back out of the store and
//! check them against the originals.
//!
//! It exists because "it compiles on Windows" is not evidence. Media
//! Foundation decoding a first frame, a loopback bind, SQLite under a WAL,
//! reference blobs hashing files in place — each of those is a place where a
//! platform can differ, and this walks all of them and prints what happened.
//!
//! ```text
//! cargo run --release --example local_media_import -- <media-dir> [store-root]
//! ```
//!
//! Exit code is 0 only if every published asset read back byte-identical to
//! the file on disk that it references.

use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClientConfig, ClientError, PublishBundle, PublishBundleFile,
    PublishRights, PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetKind, DerivativePolicy, FileRole, MediaType, Redistribution, Sha256,
    ThumbnailMedia,
};
use makepad_asset_importer::thumbs;
use makepad_asset_importer::videothumb::probe_video;
use makepad_asset_store::{AssetServer, BlobRefPolicy, ServerConfig};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

const NS: &str = "vjmedia";

fn classify(ext: &str) -> Option<(AssetKind, FileRole, MediaType)> {
    Some(match ext {
        "mp4" | "mov" | "m4v" => (AssetKind::Video, FileRole::Video, MediaType::Mp4),
        "png" => (AssetKind::Texture, FileRole::Texture, MediaType::Png),
        "jpg" | "jpeg" => (AssetKind::Texture, FileRole::Texture, MediaType::Jpeg),
        "wav" | "wave" => (AssetKind::Audio, FileRole::Audio, MediaType::Wav),
        "mp3" => (AssetKind::Audio, FileRole::Audio, MediaType::Mp3),
        "ogg" | "oga" => (AssetKind::Audio, FileRole::Audio, MediaType::Ogg),
        _ => return None,
    })
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 || out.len() > 5_000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else {
            out.push(path);
        }
    }
}

fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut dash = true;
    for ch in text.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && out.len() < max {
            out.push('-');
            dash = true;
        }
        if out.len() >= max {
            break;
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() { "clip".to_string() } else { t }
}

fn hex8(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize()[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn rights() -> PublishRights {
    PublishRights {
        license: "LicenseRef-Personal-Library".to_string(),
        license_revision: String::new(),
        terms_digest: None,
        terms_url: String::new(),
        credits: String::new(),
        source: "local import".to_string(),
        source_archive: None,
        redistribution: Redistribution::Forbidden,
        derivatives: DerivativePolicy::Allowed,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: local_media_import <media-dir> [store-root]");
        std::process::exit(2);
    }
    let media_dir = PathBuf::from(&args[0]);
    let root = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("vj-local-store-proof"));

    println!("== FULLY-LOCAL CONTENT PATH ==");
    println!("media dir : {}", media_dir.display());
    println!("store root: {}", root.display());

    // ---- 1. host the store, loopback only -------------------------------
    let t0 = Instant::now();
    std::fs::create_dir_all(&root).expect("create store root");
    let mut cfg = ServerConfig::new(root.clone());
    cfg.control_addr = SocketAddr::from(([127, 0, 0, 1], 0));
    cfg.data_addr = SocketAddr::from(([127, 0, 0, 1], 0));
    cfg.bootstrap_admin = true;
    cfg.discovery = None;
    cfg.blob_refs = BlobRefPolicy::local_host();
    cfg.log = false;
    let server = match AssetServer::start(cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FAIL: could not host the store: {e}");
            std::process::exit(1);
        }
    };
    let token = std::fs::read_to_string(root.join("admin-token"))
        .expect("admin token")
        .trim()
        .to_string();
    println!(
        "[1] store up on control {} / data {}  ({} ms)",
        server.control_addr(),
        server.data_addr(),
        t0.elapsed().as_millis()
    );
    assert!(server.control_addr().ip().is_loopback(), "control plane must be loopback");
    assert!(server.data_addr().ip().is_loopback(), "data plane must be loopback");

    // ---- 2. connect a normal client -------------------------------------
    let mut client_cfg = ClientConfig::new(root.join("proof-cache"));
    client_cfg.token = Some(token);
    let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
    let mut client = match AssetClient::connect(client_cfg, endpoints, Some(server.server_id())) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FAIL: client could not attach: {e}");
            std::process::exit(1);
        }
    };
    println!("[2] client attached over HTTP");

    // ---- 3. walk + import by reference -----------------------------------
    let mut paths = Vec::new();
    if media_dir.is_file() {
        paths.push(media_dir.clone());
    } else {
        walk(&media_dir, 0, &mut paths);
    }
    let mut published = 0usize;
    let mut present = 0usize;
    let mut failed = 0usize;
    let mut imported: Vec<(PathBuf, makepad_asset_data::BlobId, u64)> = Vec::new();
    let rights = rights();
    let t_import = Instant::now();

    for path in &paths {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Some((kind, role, media)) = classify(&ext) else { continue };
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or("clip").to_string();
        let alias_text =
            format!("{NS}/{}-{}", slug(&stem, 48), hex8(abs.to_string_lossy().as_bytes()));
        let Ok(alias) = AssetAlias::new(alias_text) else {
            failed += 1;
            continue;
        };
        if !matches!(client.resolve_alias(&alias), Err(ClientError::NotFound { .. })) {
            present += 1;
            continue;
        }

        // The thumbnail is the one thing the store DOES own. For video it
        // costs one hardware-decoded frame and no full read.
        let (thumb, millis, dims) = match kind {
            AssetKind::Video => match probe_video(&abs) {
                Ok(p) => {
                    if !p.real_frame {
                        println!("    ! {}: no decodable first frame", stem);
                    }
                    (
                        PublishThumbnail::plain(
                            p.thumbnail_jpeg,
                            ThumbnailMedia::Jpeg,
                            thumbs::THUMB_DIM as u32,
                            thumbs::THUMB_DIM as u32,
                        ),
                        p.duration_ms,
                        None,
                    )
                }
                Err(e) => {
                    println!("    x {}: video probe failed: {e}", stem);
                    failed += 1;
                    continue;
                }
            },
            AssetKind::Texture => {
                let bytes = std::fs::read(&abs).unwrap_or_default();
                let d = thumbs::png_dims(&bytes).or_else(|| thumbs::jpeg_dims(&bytes));
                match (makepad_asset_importer::import::usable_image_thumb(&bytes), d) {
                    (Some((t, m, w, h)), Some(d)) => {
                        (PublishThumbnail::plain(t, m, w, h), 0, Some(d))
                    }
                    _ => {
                        println!("    x {}: unusable image", stem);
                        failed += 1;
                        continue;
                    }
                }
            }
            _ => {
                let bytes = std::fs::read(&abs).unwrap_or_default();
                match thumbs::decode_audio(&bytes, media)
                    .and_then(|pcm| thumbs::audio_thumbnail_jpeg(&pcm))
                {
                    Ok(t) => (
                        PublishThumbnail {
                            bytes: t.bytes,
                            media: ThumbnailMedia::Jpeg,
                            width: t.width,
                            height: t.height,
                            views: t.views,
                        },
                        thumbs::audio_millis(&bytes, media).unwrap_or(0),
                        None,
                    ),
                    Err(e) => {
                        println!("    x {}: audio decode failed: {e}", stem);
                        failed += 1;
                        continue;
                    }
                }
            }
        };

        let mut bundle = PublishBundle::new(
            NS,
            kind,
            stem.clone(),
            vec![PublishBundleFile::reference(role, media, abs.clone(), dims)],
            thumb,
            rights.clone(),
        );
        bundle.alias = Some(alias);
        bundle.media_millis = millis;
        bundle.categories = vec!["imported".to_string()];
        match client.publish_bundle(&bundle) {
            Ok(p) => {
                published += 1;
                imported.push((abs.clone(), p.files[0].blob, p.files[0].byte_len));
            }
            Err(e) => {
                println!("    x {}: publish failed: {e}", stem);
                failed += 1;
            }
        }
    }
    println!(
        "[3] imported by reference: {published} published, {present} already present, {failed} failed  ({} ms)",
        t_import.elapsed().as_millis()
    );

    // ---- 4. the store did NOT copy the payloads --------------------------
    let cas_bytes = dir_bytes(&root.join("cas"));
    let source_bytes: u64 = imported.iter().map(|(_, _, n)| *n).sum();
    println!(
        "[4] cas holds {:.2} MB; the media it catalogues is {:.2} MB",
        cas_bytes as f64 / 1e6,
        source_bytes as f64 / 1e6
    );
    if source_bytes > 0 && cas_bytes >= source_bytes {
        eprintln!("FAIL: the store copied the payloads (cas >= source bytes)");
        std::process::exit(1);
    }

    // ---- 5. read every payload back and check it -------------------------
    let mut verified = 0usize;
    let mut bad = 0usize;
    for (path, blob, len) in &imported {
        let want = std::fs::read(path).unwrap_or_default();
        match client.fetch_blob_bytes(blob, Some(*len)) {
            Ok(got) if got == want => verified += 1,
            Ok(_) => {
                eprintln!("    MISMATCH {}", path.display());
                bad += 1;
            }
            Err(e) => {
                eprintln!("    UNREADABLE {}: {e}", path.display());
                bad += 1;
            }
        }
    }
    println!("[5] served back and byte-compared: {verified} ok, {bad} bad");

    // ---- 6. the catalog can be browsed like any other --------------------
    match client.assets_page(Some(NS), None, 50) {
        Ok(page) => println!("[6] catalog lists {} assets in {NS}", page.assets.len()),
        Err(e) => println!("[6] catalog listing failed: {e}"),
    }

    // ---- 7. the reference re-scan sees them all as present ---------------
    match client.blob_refs_page(None, 64) {
        Ok(page) => {
            let ok = page.refs.iter().filter(|r| r.ok).count();
            println!("[7] {} reference blobs, {} present", page.total, ok);
            for r in page.refs.iter().filter(|r| !r.ok) {
                println!("    ! {} is {}", r.path, r.state);
            }
        }
        Err(e) => println!("[7] rescan failed: {e}"),
    }

    // ---- 8. drift is caught, not served ----------------------------------
    if let Some((path, blob, len)) = imported.first().cloned() {
        let original = std::fs::read(&path).unwrap_or_default();
        let mut tampered = original.clone();
        if !tampered.is_empty() {
            tampered[0] ^= 0xff;
            std::fs::write(&path, &tampered).ok();
            let mut fresh_cfg = ClientConfig::new(root.join("proof-cache-2"));
            fresh_cfg.token = std::fs::read_to_string(root.join("admin-token"))
                .ok()
                .map(|t| t.trim().to_string());
            let refused = match AssetClient::connect(fresh_cfg, endpoints, Some(server.server_id()))
            {
                Ok(mut c) => c.fetch_blob_bytes(&blob, Some(len)).is_err(),
                Err(_) => false,
            };
            std::fs::write(&path, &original).ok();
            println!(
                "[8] a file changed underneath is {}",
                if refused { "REFUSED (correct)" } else { "SERVED ANYWAY (WRONG)" }
            );
            if !refused {
                std::process::exit(1);
            }
        }
    }

    if bad > 0 || (published == 0 && present == 0) {
        eprintln!("RESULT: FAIL");
        std::process::exit(1);
    }
    println!("RESULT: PASS  ({} ms total)", t0.elapsed().as_millis());
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}
