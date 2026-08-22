//! IMPORT CONTENT: walk a file or directory and catalogue what is in it,
//! WITHOUT COPYING the payloads.
//!
//! A VJ's video folder is the thing you least want duplicated — it is
//! already curated, already backed up, and already too big. So this importer
//! publishes normal catalog assets whose heavy file is a *reference*: the
//! store hashes the mp4 where it lies and records path + size + digest, and
//! the only bytes it ever owns for that asset are the derived ones — the
//! thumbnail, the manifest, the search row. Those are small, they are the
//! store's own product, and losing them costs nothing but a re-import.
//!
//! Everything else about the asset is ordinary. The manifest names a digest
//! and a length exactly as an uploaded one does, so the grids, the catalog
//! events, playback and every other client behave identically. The only
//! observable difference is the one the user asked for: their disk did not
//! fill up twice.
//!
//! ## The walk
//!
//! Sorted by name (two runs enumerate identically), hidden entries skipped
//! (`.DS_Store`, resource forks), symlinks skipped (a symlinked collection
//! is neither walked twice nor an escape from the root), and bounded by
//! depth and file count so a pathological tree cannot become an unbounded
//! job. Recognised-but-unsupported extensions are REPORTED rather than
//! silently dropped — "why is that clip not in my grid" deserves an answer.
//!
//! ## What is skipped on a re-import
//!
//! Identity is the alias, derived from the file's absolute path, so a second
//! import of the same directory resolves each alias, finds it, and skips.
//! That makes re-import cheap (one control-plane call per file, no hashing
//! of gigabytes). It also means a file EDITED IN PLACE is not noticed here —
//! that is what the store's reference re-scan is for, and it reports such a
//! file as `content_changed` rather than letting it play as something else.

use makepad_asset_client::{
    AssetClient, ClientError, PublishBundle, PublishBundleFile, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetKind, FileRole, MediaType, Sha256, ThumbnailMedia,
};
use makepad_asset_importer::thumbs;
use makepad_asset_importer::videothumb::probe_video;
use std::path::{Path, PathBuf};

/// Namespace imported media lands in. Its own, so a wipe or a search filter
/// can address "the stuff I imported from disk" without touching generated
/// content.
pub const MEDIA_NAMESPACE: &str = "vjmedia";

/// Walk bounds. Generous for a real library, finite for a hostile tree.
const MAX_DEPTH: usize = 12;
const MAX_FILES: usize = 100_000;

/// What a file is, once its extension is recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaClass {
    Video,
    Image,
    Audio,
}

impl MediaClass {
    pub fn label(self) -> &'static str {
        match self {
            MediaClass::Video => "video",
            MediaClass::Image => "image",
            MediaClass::Audio => "audio",
        }
    }
}

/// One recognised file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFile {
    pub path: PathBuf,
    pub class: MediaClass,
    pub media: MediaType,
    /// Filename without extension, used for the title.
    pub stem: String,
    /// Directory names between the scan root and the file — they become
    /// tags, so a folder tree is searchable without anyone typing metadata.
    pub dirs: Vec<String>,
    pub size: u64,
}

/// A file that was seen and deliberately not imported, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedFile {
    pub path: PathBuf,
    pub why: String,
}

#[derive(Clone, Debug, Default)]
pub struct MediaScan {
    pub files: Vec<MediaFile>,
    pub skipped: Vec<SkippedFile>,
    /// True when the walk stopped at a bound rather than at the end.
    pub truncated: bool,
}

/// Extensions we can actually publish. `.mov` publishes as `MediaType::Mp4`
/// because both platform demuxers read it and the content contract has no
/// separate tag — the same choice asset-ui's drop import makes.
fn classify(ext: &str) -> Option<(MediaClass, MediaType)> {
    Some(match ext {
        "mp4" | "mov" | "m4v" => (MediaClass::Video, MediaType::Mp4),
        "png" => (MediaClass::Image, MediaType::Png),
        "jpg" | "jpeg" => (MediaClass::Image, MediaType::Jpeg),
        "wav" | "wave" => (MediaClass::Audio, MediaType::Wav),
        "mp3" => (MediaClass::Audio, MediaType::Mp3),
        "ogg" | "oga" => (MediaClass::Audio, MediaType::Ogg),
        _ => return None,
    })
}

/// Extensions a user will reasonably expect to work and that we cannot
/// publish yet. Naming them is the difference between a bug report and an
/// informed choice about transcoding.
const KNOWN_UNSUPPORTED: &[&str] = &[
    "mkv", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "m2ts", "ts", "gif", "webp", "bmp",
    "tif", "tiff", "heic", "flac", "m4a", "aac", "aiff", "aif", "wma", "opus",
];

/// Scan a directory tree (or a single file) for importable media.
pub fn scan(root: &Path) -> MediaScan {
    let mut out = MediaScan::default();
    if root.is_file() {
        consider(root, &[], &mut out);
        return out;
    }
    walk(root, &mut Vec::new(), 0, &mut out);
    out
}

fn walk(dir: &Path, trail: &mut Vec<String>, depth: usize, out: &mut MediaScan) {
    if depth > MAX_DEPTH {
        out.skipped.push(SkippedFile {
            path: dir.to_path_buf(),
            why: format!("deeper than {MAX_DEPTH} levels"),
        });
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        out.skipped.push(SkippedFile {
            path: dir.to_path_buf(),
            why: "unreadable directory".to_string(),
        });
        return;
    };
    // Sort so two runs of the same tree produce the same order — an import
    // that is deterministic is one whose progress bar means something.
    let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    names.sort();
    for path in names {
        if out.files.len() >= MAX_FILES {
            out.truncated = true;
            return;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        // Hidden entries are metadata, not content.
        if name.starts_with('.') {
            continue;
        }
        // A symlink is either a second path to something already walked or a
        // way out of the root; neither is wanted.
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_symlink() => continue,
            Err(_) => continue,
            _ => {}
        }
        if path.is_dir() {
            trail.push(name.to_string());
            walk(&path, trail, depth + 1, out);
            trail.pop();
        } else {
            consider(&path, trail, out);
        }
    }
}

fn consider(path: &Path, trail: &[String], out: &mut MediaScan) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some((class, media)) = classify(&ext) else {
        if KNOWN_UNSUPPORTED.contains(&ext.as_str()) {
            out.skipped.push(SkippedFile {
                path: path.to_path_buf(),
                why: format!("{ext} is not a format the catalog publishes yet"),
            });
        }
        return;
    };
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        out.skipped.push(SkippedFile {
            path: path.to_path_buf(),
            why: "empty file".to_string(),
        });
        return;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    out.files.push(MediaFile {
        path: path.to_path_buf(),
        class,
        media,
        stem,
        dirs: trail.to_vec(),
        size,
    });
}

// ---------------------------------------------------------------------------
// publication
// ---------------------------------------------------------------------------

/// Lowercase-alphanumeric-and-dash, bounded — the alias segment charset.
fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in text.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && out.len() < max {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= max {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "clip".to_string()
    } else {
        trimmed
    }
}

fn hex8(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// The stable alias for a file at this exact path.
///
/// Path-derived rather than content-derived on purpose: content-derived
/// would mean hashing gigabytes just to learn a NAME, and hashing is the one
/// cost this importer exists to avoid paying twice. Two different files with
/// the same name in different folders get different aliases; the same file
/// re-imported gets the same one and is skipped.
pub fn alias_for(path: &Path) -> Option<AssetAlias> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let text = format!(
        "{MEDIA_NAMESPACE}/{}-{}",
        slug(stem, 48),
        hex8(abs.to_string_lossy().as_bytes())
    );
    AssetAlias::new(text).ok()
}

/// The rights an import of the user's OWN files carries: no license is
/// asserted about content whose provenance we do not know, and the source
/// records where it came from. Nothing here claims a grant the user does not
/// have — that is the whole content-rights discipline, applied honestly to
/// "some mp4s on a disk".
pub fn personal_rights(root: &Path) -> PublishRights {
    use makepad_asset_data::{DerivativePolicy, Redistribution};
    PublishRights {
        license: "LicenseRef-Personal-Library".to_string(),
        license_revision: String::new(),
        terms_digest: None,
        terms_url: String::new(),
        credits: String::new(),
        source: format!("local import: {}", root.display()),
        source_archive: None,
        // The operator's own library: nothing is cleared for redistribution
        // by this import, and saying otherwise would be a lie the catalog
        // then carries forever inside an immutable revision.
        redistribution: Redistribution::Forbidden,
        derivatives: DerivativePolicy::Allowed,
    }
}

/// The picture for one file, plus its playback length in ms.
///
/// Video costs one hardware-decoded frame and no full read — `probe_video`
/// takes a PATH, which is exactly the shape reference mode wants. Images and
/// audio are read (they are small, and an audio spectrogram needs the
/// samples), but only to DERIVE the thumbnail: those bytes are never
/// uploaded as the payload.
fn thumbnail_of(file: &MediaFile) -> Result<(PublishThumbnail, u32), String> {
    match file.class {
        MediaClass::Video => {
            let probe = probe_video(&file.path)?;
            if probe.duration_ms == 0 {
                return Err("no measurable duration (unreadable video)".to_string());
            }
            Ok((
                PublishThumbnail::plain(
                    probe.thumbnail_jpeg,
                    ThumbnailMedia::Jpeg,
                    thumbs::THUMB_DIM as u32,
                    thumbs::THUMB_DIM as u32,
                ),
                probe.duration_ms,
            ))
        }
        MediaClass::Image => {
            let bytes = std::fs::read(&file.path).map_err(|e| format!("read: {e}"))?;
            match makepad_asset_importer::import::usable_image_thumb(&bytes) {
                Some((thumb, media, w, h)) => {
                    Ok((PublishThumbnail::plain(thumb, media, w, h), 0))
                }
                // Outside the contract's 256..4096 window: the picture is
                // the honest placeholder rather than a stretched lie.
                None => {
                    let jpeg = thumbs::encode_jpeg_bgra(
                        &thumbs::placeholder_bgra_512(),
                        thumbs::THUMB_DIM,
                        thumbs::THUMB_DIM,
                    )?;
                    Ok((
                        PublishThumbnail::plain(
                            jpeg,
                            ThumbnailMedia::Jpeg,
                            thumbs::THUMB_DIM as u32,
                            thumbs::THUMB_DIM as u32,
                        ),
                        0,
                    ))
                }
            }
        }
        MediaClass::Audio => {
            let bytes = std::fs::read(&file.path).map_err(|e| format!("read: {e}"))?;
            let pcm = thumbs::decode_audio(&bytes, file.media)?;
            let millis = thumbs::audio_millis(&bytes, file.media).unwrap_or(0);
            let thumb = thumbs::audio_thumbnail_jpeg(&pcm)?;
            Ok((
                PublishThumbnail {
                    bytes: thumb.bytes,
                    media: ThumbnailMedia::Jpeg,
                    width: thumb.width,
                    height: thumb.height,
                    views: thumb.views,
                },
                millis,
            ))
        }
    }
}

fn kind_of(class: MediaClass) -> AssetKind {
    match class {
        MediaClass::Video => AssetKind::Video,
        MediaClass::Image => AssetKind::Texture,
        MediaClass::Audio => AssetKind::Audio,
    }
}

fn role_of(class: MediaClass) -> FileRole {
    match class {
        MediaClass::Video => FileRole::Video,
        MediaClass::Image => FileRole::Texture,
        MediaClass::Audio => FileRole::Audio,
    }
}

/// What happened to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOutcome {
    Published,
    /// The alias already points at something: already in the library.
    AlreadyPresent,
    Failed(String),
}

/// Import ONE file by reference. Returns quickly for a file already present.
pub fn import_file(
    client: &mut AssetClient,
    root: &Path,
    file: &MediaFile,
    rights: &PublishRights,
) -> FileOutcome {
    let Some(alias) = alias_for(&file.path) else {
        return FileOutcome::Failed("cannot derive an alias for this path".to_string());
    };
    // Cheap presence probe BEFORE anything expensive. One control-plane
    // round trip beats decoding a frame of every clip in a folder we
    // already imported last week.
    match client.resolve_alias(&alias) {
        Ok(_) => return FileOutcome::AlreadyPresent,
        Err(ClientError::NotFound { .. }) => {}
        Err(error) => return FileOutcome::Failed(format!("alias probe: {error}")),
    }

    let (thumbnail, media_millis) = match thumbnail_of(file) {
        Ok(v) => v,
        Err(error) => return FileOutcome::Failed(error),
    };
    let abs = match std::path::absolute(&file.path) {
        Ok(p) => p,
        Err(error) => return FileOutcome::Failed(format!("absolute path: {error}")),
    };
    // Images declare their pixel dims in the manifest; other media must not.
    let dims = if matches!(file.class, MediaClass::Image) {
        match std::fs::read(&file.path)
            .ok()
            .and_then(|b| thumbs::png_dims(&b).or_else(|| thumbs::jpeg_dims(&b)))
        {
            Some(d) => Some(d),
            None => return FileOutcome::Failed("unreadable image header".to_string()),
        }
    } else {
        None
    };

    let mut bundle = PublishBundle::new(
        MEDIA_NAMESPACE,
        kind_of(file.class),
        file.stem.clone(),
        // THE POINT: a reference, not bytes. The store hashes this path in
        // place; nothing is uploaded and nothing is duplicated.
        vec![PublishBundleFile::reference(
            role_of(file.class),
            file.media,
            abs.clone(),
            dims,
        )],
        thumbnail,
        rights.clone(),
    );
    bundle.alias = Some(alias);
    bundle.media_millis = media_millis;
    bundle.categories = vec!["imported".to_string(), file.class.label().to_string()];
    // Folder names become tags: a tree of "sets/2024/opener" is searchable
    // the moment it lands, with nobody typing metadata.
    bundle.tags = file.dirs.iter().map(|d| slug(d, 32)).collect();
    bundle.tags.push("local".to_string());
    bundle.generator = "makepad-vj import".to_string();
    bundle.provenance = format!("referenced in place from {}", abs.display());
    bundle.description = format!("Imported from {}", root.display());

    match client.publish_bundle(&bundle) {
        Ok(_) => FileOutcome::Published,
        Err(error) => FileOutcome::Failed(format!("{error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vj-mediascan-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_extension_filter_admits_media_and_names_what_it_refuses() {
        let dir = tmp("filter");
        for name in ["a.mp4", "b.MOV", "c.png", "d.wav", "e.mkv", "f.txt", "g.jpeg"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let scan = scan(&dir);
        let mut got: Vec<&str> = scan
            .files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_str().unwrap())
            .collect();
        got.sort();
        assert_eq!(got, vec!["a.mp4", "b.MOV", "c.png", "d.wav", "g.jpeg"]);
        // mkv is named as unsupported; txt is simply not media and is silent.
        let skipped: Vec<&str> = scan
            .skipped
            .iter()
            .map(|s| s.path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(skipped.contains(&"e.mkv"), "a video format we cannot publish must be reported");
        assert!(!skipped.contains(&"f.txt"), "a text file is not a failed import");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_walk_recurses_tags_by_folder_and_skips_hidden() {
        let dir = tmp("walk");
        std::fs::create_dir_all(dir.join("sets/2024")).unwrap();
        std::fs::write(dir.join("sets/2024/opener.mp4"), b"x").unwrap();
        std::fs::write(dir.join("top.mp4"), b"x").unwrap();
        std::fs::write(dir.join(".hidden.mp4"), b"x").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/sneaky.mp4"), b"x").unwrap();

        let scan = scan(&dir);
        assert_eq!(scan.files.len(), 2, "hidden files and hidden dirs stay out");
        let opener = scan.files.iter().find(|f| f.stem == "opener").unwrap();
        assert_eq!(opener.dirs, vec!["sets".to_string(), "2024".to_string()]);
        let top = scan.files.iter().find(|f| f.stem == "top").unwrap();
        assert!(top.dirs.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_single_file_scans_as_itself() {
        let dir = tmp("single");
        let f = dir.join("one.mp4");
        std::fs::write(&f, b"x").unwrap();
        let scan = scan(&f);
        assert_eq!(scan.files.len(), 1);
        assert_eq!(scan.files[0].class, MediaClass::Video);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_files_are_refused_with_a_reason() {
        let dir = tmp("empty");
        std::fs::write(dir.join("zero.mp4"), b"").unwrap();
        let scan = scan(&dir);
        assert!(scan.files.is_empty());
        assert_eq!(scan.skipped.len(), 1);
        assert!(scan.skipped[0].why.contains("empty"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn aliases_are_stable_per_path_and_distinct_across_folders() {
        let dir = tmp("alias");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let pa = dir.join("a/clip.mp4");
        let pb = dir.join("b/clip.mp4");
        std::fs::write(&pa, b"x").unwrap();
        std::fs::write(&pb, b"x").unwrap();
        let a1 = alias_for(&pa).unwrap();
        let a2 = alias_for(&pa).unwrap();
        let b1 = alias_for(&pb).unwrap();
        assert_eq!(a1, a2, "the same path must always produce the same alias");
        assert_ne!(a1, b1, "same name in another folder is another asset");
        assert!(a1.as_str().starts_with("vjmedia/clip-"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dedup_is_by_digest_when_two_paths_hold_one_file() {
        // Two paths, identical bytes: they are two ASSETS (the user has two
        // clips) but the store admits ONE blob, because admission is
        // content-addressed. This test pins the naming half of that; the
        // store's own suite pins the blob half.
        let dir = tmp("dedup");
        let pa = dir.join("one.mp4");
        let pb = dir.join("two.mp4");
        std::fs::write(&pa, b"SAME").unwrap();
        std::fs::write(&pb, b"SAME").unwrap();
        assert_ne!(alias_for(&pa).unwrap(), alias_for(&pb).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }
}
