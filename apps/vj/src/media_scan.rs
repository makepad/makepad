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
//!
//! ## The one exception: VARIABLE-FRAMERATE VIDEO IMPORT
//!
//! With [`ImportCtx::convert`] set, video files are not referenced — they are
//! CONVERTED and the result is stored as an owned blob. The conversion (see
//! `makepad_video_flow`) measures the motion between consecutive frames,
//! re-encodes the clip all-intra so it decodes in any order, and embeds the
//! motion field as the `mkfl` box. That is what lets the deck play the clip
//! at any rate, forwards or backwards, through the flow-warp path instead of
//! stepping frames at whatever rate the file was shot at.
//!
//! It is the opposite trade from the rest of this module and it is made
//! deliberately, on the operator's explicit tick: the library gains a second
//! copy of that clip, and gains the ability to scratch it. Images and audio
//! are untouched either way.

use makepad_asset_client::{
    AssetClient, ClientError, PublishBundle, PublishBundleFile, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetKind, FileRole, MediaType, Sha256, ThumbnailMedia,
};
use makepad_asset_importer::thumbs;
use makepad_asset_importer::videothumb::probe_video;
use makepad_video_flow::{convert_video, ConvertError, ConvertOptions};
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

/// The tags an imported file carries into the catalog. Folder names become
/// tags — a tree of "sets/2024/opener" is searchable the moment it lands,
/// with nobody typing metadata — plus `local` for the lane, `flow` when the
/// clip was converted, and for AUDIO the music tag: the DJ deck explorer
/// narrows to `catalog::MUSIC_TAG`, so an import without it lands in a
/// store the deck browser cannot see.
pub fn import_tags(file: &MediaFile, converted: bool) -> Vec<String> {
    let mut tags: Vec<String> = file.dirs.iter().map(|d| slug(d, 32)).collect();
    tags.push("local".to_string());
    if file.class == MediaClass::Audio {
        tags.push(crate::catalog::MUSIC_TAG.to_string());
    }
    if converted {
        tags.push("flow".to_string());
    }
    tags
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
        // Decoded on import (libs/webp) and published as OWNED png bytes:
        // the catalog has no webp media type and the draw path no webp
        // decode, so the conversion happens once, here.
        "webp" => (MediaClass::Image, MediaType::Png),
        "wav" | "wave" => (MediaClass::Audio, MediaType::Wav),
        "mp3" => (MediaClass::Audio, MediaType::Mp3),
        "ogg" | "oga" => (MediaClass::Audio, MediaType::Ogg),
        _ => return None,
    })
}

/// Decode a webp file into png bytes (RGB and RGBA both; animation takes
/// the first frame). One decode, at import — nothing downstream speaks webp.
fn webp_to_png(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let mut decoder = makepad_webp::WebPDecoder::new(std::io::BufReader::new(
        std::io::Cursor::new(&bytes),
    ))
    .map_err(|e| format!("{e}"))?;
    let (w, h) = decoder.dimensions();
    let size = decoder.output_buffer_size().ok_or("size overflow")?;
    let mut decoded = vec![0u8; size];
    decoder.read_image(&mut decoded).map_err(|e| format!("{e}"))?;
    let pixels = (w as usize).checked_mul(h as usize).ok_or("dimension overflow")?;
    if pixels == 0 {
        return Err("zero-sized image".to_string());
    }
    let rgba: Vec<u8> = match decoded.len() / pixels {
        4 => decoded,
        3 => {
            let mut out = Vec::with_capacity(pixels * 4);
            for px in decoded.chunks_exact(3) {
                out.extend_from_slice(px);
                out.push(0xff);
            }
            out
        }
        n => return Err(format!("unsupported channel count {n}")),
    };
    makepad_asset_importer::classic_import::encode_png_rgba(&rgba, w, h)
        .map_err(|e| format!("png encode: {e}"))
}

/// Extensions a user will reasonably expect to work and that we cannot
/// publish yet. Naming them is the difference between a bug report and an
/// informed choice about transcoding.
const KNOWN_UNSUPPORTED: &[&str] = &[
    "mkv", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "m2ts", "ts", "gif", "bmp",
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
///
/// `path` is separate from `file` because a converted clip is pictured from
/// the CONVERTED file: the thumbnail must show what will actually play.
fn thumbnail_of(file: &MediaFile, path: &Path) -> Result<(PublishThumbnail, u32), String> {
    match file.class {
        MediaClass::Video => {
            let probe = probe_video(path)?;
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
            let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
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
            let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
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
    /// In the library. `converted` distinguishes the owned flow-warp blob
    /// from the usual no-copy reference, and the note, when there is one, is
    /// something the operator must be told about what landed (a conversion
    /// that had to scale the clip down, or one that failed and fell back).
    Published { converted: bool, note: Option<String> },
    /// The alias already points at something: already in the library.
    AlreadyPresent,
    Failed(String),
    /// The operator stopped the run in the middle of this file.
    Cancelled,
}

/// Where one file's slow phase has got to, for the panel's bar and label.
#[derive(Clone, Copy, Debug)]
pub struct FileProgress {
    /// What is happening — "converting" is the only phase slow enough to
    /// need naming so far.
    pub phase: &'static str,
    pub fraction: f64,
}

/// Everything one file's import needs beyond the file itself.
pub struct ImportCtx<'a> {
    pub rights: &'a PublishRights,
    /// Set to a scratch directory to CONVERT videos for flow-warp playback
    /// and store them as owned blobs; `None` keeps the no-copy reference
    /// import for everything.
    pub convert_dir: Option<&'a Path>,
    pub convert_options: ConvertOptions,
    /// Called during the slow phases so the bar keeps moving inside one file.
    pub progress: &'a mut dyn FnMut(FileProgress),
    /// Checked per frame during a conversion.
    pub cancel: &'a dyn Fn() -> bool,
}

/// A converted clip's temp file, removed whatever happens next.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Import ONE file: by reference, or — for a video when `ctx.convert_dir` is
/// set — converted for flow-warp playback and stored as bytes. Returns
/// quickly for a file already present.
pub fn import_file(
    client: &mut AssetClient,
    root: &Path,
    file: &MediaFile,
    ctx: &mut ImportCtx,
) -> FileOutcome {
    let Some(alias) = alias_for(&file.path) else {
        return FileOutcome::Failed("cannot derive an alias for this path".to_string());
    };
    // Cheap presence probe BEFORE anything expensive. One control-plane
    // round trip beats decoding a frame of every clip in a folder we
    // already imported last week — and beats re-converting one.
    match client.resolve_alias(&alias) {
        Ok(_) => return FileOutcome::AlreadyPresent,
        Err(ClientError::NotFound { .. }) => {}
        Err(error) => return FileOutcome::Failed(format!("alias probe: {error}")),
    }

    let abs = match std::path::absolute(&file.path) {
        Ok(p) => p,
        Err(error) => return FileOutcome::Failed(format!("absolute path: {error}")),
    };

    // ---- webp: decode once, publish owned png bytes ---------------------
    // The catalog has no webp media type and nothing downstream decodes
    // webp at draw time, so the conversion is the import.
    let webp_png: Option<Vec<u8>> = if file
        .path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("webp"))
    {
        match webp_to_png(&abs) {
            Ok(png) => Some(png),
            Err(why) => return FileOutcome::Failed(format!("webp decode: {why}")),
        }
    } else {
        None
    };

    // ---- the conversion route (videos only, and only when asked) --------
    let mut note: Option<String> = None;
    let mut converted: Option<(Scratch, String)> = None;
    if let (MediaClass::Video, Some(dir)) = (file.class, ctx.convert_dir) {
        match convert_for_flow(&abs, dir, ctx) {
            Ok(ConvertOutcome::Converted { scratch, provenance, note: n }) => {
                note = n;
                converted = Some((scratch, provenance));
            }
            Ok(ConvertOutcome::Cancelled) => return FileOutcome::Cancelled,
            // A clip we cannot convert is still a clip the operator asked to
            // import: it lands by reference, and the reason it is not
            // scratchable is REPORTED rather than swallowed.
            Err(why) => note = Some(format!("{}: {why}; imported by reference", file.stem)),
        }
    }

    let payload_path = converted
        .as_ref()
        .map(|(scratch, _)| scratch.0.clone())
        .unwrap_or_else(|| abs.clone());
    let (thumbnail, media_millis) = match &webp_png {
        // The converted picture IS the payload: thumbnail from those bytes,
        // never a second decode of the original.
        Some(png) => match makepad_asset_importer::import::usable_image_thumb(png) {
            Some((thumb, media, w, h)) => (PublishThumbnail::plain(thumb, media, w, h), 0),
            None => match thumbs::encode_jpeg_bgra(
                &thumbs::placeholder_bgra_512(),
                thumbs::THUMB_DIM,
                thumbs::THUMB_DIM,
            ) {
                Ok(jpeg) => (
                    PublishThumbnail::plain(
                        jpeg,
                        ThumbnailMedia::Jpeg,
                        thumbs::THUMB_DIM as u32,
                        thumbs::THUMB_DIM as u32,
                    ),
                    0,
                ),
                Err(error) => return FileOutcome::Failed(error),
            },
        },
        None => match thumbnail_of(file, &payload_path) {
            Ok(v) => v,
            Err(error) => return FileOutcome::Failed(error),
        },
    };
    // Images declare their pixel dims in the manifest; other media must not.
    let dims = if matches!(file.class, MediaClass::Image) {
        let header = match &webp_png {
            Some(png) => thumbs::png_dims(png),
            None => std::fs::read(&file.path)
                .ok()
                .and_then(|b| thumbs::png_dims(&b).or_else(|| thumbs::jpeg_dims(&b))),
        };
        match header {
            Some(d) => Some(d),
            None => return FileOutcome::Failed("unreadable image header".to_string()),
        }
    } else {
        None
    };

    let (files, provenance) = if let Some(png) = webp_png {
        // Owned converted bytes, like a converted clip: the png exists
        // nowhere on disk, so a reference would dangle.
        (
            vec![PublishBundleFile::bytes(role_of(file.class), MediaType::Png, png, dims)],
            format!("decoded from webp at {}", abs.display()),
        )
    } else {
        match &converted {
        // The converted clip is the store's OWN blob: these bytes exist
        // nowhere else, so referencing a temp file that is about to be
        // deleted would be a dangling library entry.
        Some((scratch, provenance)) => {
            let bytes = match std::fs::read(&scratch.0) {
                Ok(b) => b,
                Err(error) => {
                    return FileOutcome::Failed(format!("read converted clip: {error}"))
                }
            };
            (
                vec![PublishBundleFile::bytes(FileRole::Video, MediaType::Mp4, bytes, None)],
                provenance.clone(),
            )
        }
        // THE DEFAULT: a reference, not bytes. The store hashes this path in
        // place; nothing is uploaded and nothing is duplicated.
        None => (
            vec![PublishBundleFile::reference(
                role_of(file.class),
                file.media,
                abs.clone(),
                dims,
            )],
            format!("referenced in place from {}", abs.display()),
        ),
        }
    };

    let mut bundle = PublishBundle::new(
        MEDIA_NAMESPACE,
        kind_of(file.class),
        file.stem.clone(),
        files,
        thumbnail,
        ctx.rights.clone(),
    );
    bundle.alias = Some(alias);
    bundle.media_millis = media_millis;
    bundle.categories = vec!["imported".to_string(), file.class.label().to_string()];
    bundle.tags = import_tags(file, converted.is_some());
    bundle.generator = "makepad-vj import".to_string();
    bundle.provenance = provenance;
    bundle.description = format!("Imported from {}", root.display());

    match client.publish_bundle(&bundle) {
        Ok(_) => FileOutcome::Published { converted: converted.is_some(), note },
        Err(error) => FileOutcome::Failed(format!("{error}")),
    }
}

enum ConvertOutcome {
    Converted {
        scratch: Scratch,
        provenance: String,
        note: Option<String>,
    },
    Cancelled,
}

/// Convert one video into a flow-carrying all-intra clip in `dir`.
fn convert_for_flow(
    source: &Path,
    dir: &Path,
    ctx: &mut ImportCtx,
) -> Result<ConvertOutcome, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("conversion scratch dir: {e}"))?;
    // Named from the source path so two clips converting in one run cannot
    // collide, and a leftover from a killed run is overwritten rather than
    // accumulated.
    let scratch = Scratch(dir.join(format!(
        "convert-{}-{}.mp4",
        std::process::id(),
        hex8(source.to_string_lossy().as_bytes())
    )));
    let progress = &mut *ctx.progress;
    let report = match convert_video(
        source,
        &scratch.0,
        &ctx.convert_options,
        &mut |p| progress(FileProgress { phase: "converting", fraction: p.fraction }),
        ctx.cancel,
    ) {
        Ok(report) => report,
        Err(ConvertError::Cancelled) => return Ok(ConvertOutcome::Cancelled),
        Err(error) => return Err(format!("flow conversion failed ({error})")),
    };
    let provenance = format!(
        "flow-converted from {} — {}x{} all-intra, {} frames, {} motion pairs",
        source.display(),
        report.width,
        report.height,
        report.frames,
        report.pairs
    );
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut note = None;
    if report.scale > 1 {
        note = Some(format!(
            "{name}: converted at {}x{} (1/{} size) so the clip fits the flow-warp cache",
            report.width, report.height, report.scale
        ));
    }
    if !report.warps {
        note = Some(format!("{name}: {}", report.warp_note));
    }
    Ok(ConvertOutcome::Converted { scratch, provenance, note })
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

    /// The deck explorer narrows to the music tag, so the import that FEEDS
    /// it has to write that tag — audio only: a video import tagged music
    /// would put clips on the DJ surface.
    #[test]
    fn imported_audio_carries_the_music_tag_and_video_does_not() {
        let file = |class, media| MediaFile {
            path: PathBuf::from("x"),
            class,
            media,
            stem: "x".into(),
            dirs: vec!["Dua Lipa".into()],
            size: 1,
        };
        let audio = import_tags(&file(MediaClass::Audio, MediaType::Mp3), false);
        assert!(audio.contains(&crate::catalog::MUSIC_TAG.to_string()), "{audio:?}");
        assert!(audio.contains(&"local".to_string()));
        assert!(audio.contains(&"dua-lipa".to_string()), "dirs stay tags: {audio:?}");
        let video = import_tags(&file(MediaClass::Video, MediaType::Mp4), true);
        assert!(!video.contains(&crate::catalog::MUSIC_TAG.to_string()), "{video:?}");
        assert!(video.contains(&"flow".to_string()), "converted keeps flow: {video:?}");
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
