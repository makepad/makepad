//! Music-directory import: a folder tree of audio files → `AssetKind::Audio`
//! assets, one row per track.
//!
//! The shape of a music library is `<Artist>/<Album>/<track>.mp3`, and that
//! shape IS the metadata the user cares about: every relative directory name
//! under the picked root becomes a catalog tag (slugified), beside the
//! constant `music` tag. Container metadata (ID3v2/ID3v1 text frames, Vorbis
//! comments) supplies the title/artist/album when the file carries them; the
//! path supplies them when it does not. Nothing is invented — an unreadable
//! tag degrades to "the name on disk", never to a guess.
//!
//! The blob published is the ORIGINAL file, untouched: the mp3 IS the
//! product and clients decode it. Duration is measured from the container
//! (MPEG frame walk with Xing/VBRI, Ogg final granule, RIFF frame count) —
//! `AssetKind::Audio` manifests refuse a zero `media_millis`, so a file whose
//! length cannot be measured is reported as skipped rather than published
//! with a fabricated one.
//!
//! Idempotent by content: the alias `<namespace>/music/<artist>/<title>` is
//! the publication marker, and a track whose head revision already holds the
//! same audio blob is skipped. Changed bytes publish a NEW revision of the
//! SAME asset id, so a playlist that pinned the identity keeps it.

use crate::import::alias_slug;
use crate::thumbs::{encode_jpeg_bgra, parse_wav, waveform_bgra_512, THUMB_DIM};
use makepad_asset_client::util::{sanitize_text, to_hex};
use makepad_asset_client::{
    AssetClient, ClientError, PublishFile, PublishRequest, PublishRights, PublishThumbnail,
};
use makepad_asset_data::limits::MAX_ALIAS_BYTES;
use makepad_asset_data::{
    AssetAlias, AssetKind, BlobId, FileRole, MediaType, ThumbnailMedia, ThumbnailView,
    ThumbnailViewKind,
};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Catalog category every imported track carries.
pub const MUSIC_CATEGORY: &str = "music";
/// Catalog tag every imported track carries, beside its directory tags.
pub const MUSIC_TAG: &str = "music";
/// Middle alias segment: `<namespace>/music/<artist>/<title>`.
pub const MUSIC_ALIAS_CLASS: &str = "music";
/// Catalog label charset budget (`check_label` in the store).
const MAX_TAG_BYTES: usize = 48;
/// Most labels one annotation may carry (`MAX_LABELS` in the store).
const MAX_TAGS: usize = 24;
/// Longest human alias segment before the collision suffix is appended.
const MAX_ALIAS_SEGMENT: usize = 48;
/// Hex digits of the relative-path digest that disambiguates a collision.
const ALIAS_DIGEST_HEX: usize = 8;
/// Deepest directory nesting walked under the picked root.
const MAX_DEPTH: usize = 12;
/// Most files enumerated in one import (a hostile tree cannot make the walk
/// unbounded; a real library is a few thousand).
const MAX_FILES: usize = 100_000;
/// Longest catalog title kept (the wire cap is 512; a title is a line).
const MAX_TITLE: usize = 200;
/// Longest description kept.
const MAX_DESCRIPTION: usize = 400;

/// Which container one file is in. Only these three are published; anything
/// else audio-shaped is listed as unsupported so the user sees it was seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Mp3,
    Ogg,
    Wav,
}

impl Container {
    pub fn media(self) -> MediaType {
        match self {
            Container::Mp3 => MediaType::Mp3,
            Container::Ogg => MediaType::Ogg,
            Container::Wav => MediaType::Wav,
        }
    }

    fn from_ext(ext: &str) -> Option<Container> {
        match ext {
            "mp3" => Some(Container::Mp3),
            "ogg" | "oga" => Some(Container::Ogg),
            "wav" | "wave" => Some(Container::Wav),
            _ => None,
        }
    }
}

/// Audio-shaped extensions this importer recognises but cannot publish yet.
/// Listing them is the point: a library is half FLAC and the user must see
/// that those files were found and deliberately left alone.
const UNSUPPORTED_EXTS: &[&str] = &["flac", "m4a", "aac", "alac", "aiff", "aif", "wma", "opus"];

/// One audio file found under the picked root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MusicFile {
    pub path: PathBuf,
    /// Path relative to the picked root, `/`-joined.
    pub rel: String,
    /// Relative DIRECTORY names under the root, outermost first. These are
    /// the user's tag vocabulary.
    pub dirs: Vec<String>,
    /// File name without its extension, as written on disk.
    pub stem: String,
    pub container: Container,
}

/// A file the walk saw and did not publish, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedFile {
    pub rel: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MusicScan {
    pub files: Vec<MusicFile>,
    pub skipped: Vec<SkippedFile>,
}

/// Walk `root` for audio files. Directories are descended in sorted order so
/// two runs enumerate identically; symlinks are not followed (a music
/// library full of symlinked collections must not be walked twice or
/// escape the root). A missing root is an empty scan, not an error — the
/// caller reports "no audio files" once.
pub fn scan_music(root: &Path) -> MusicScan {
    let mut scan = MusicScan::default();
    walk_dir(root, &mut Vec::new(), &mut scan);
    scan.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    scan.skipped.sort_by(|a, b| a.rel.cmp(&b.rel));
    scan
}

fn walk_dir(dir: &Path, stack: &mut Vec<String>, scan: &mut MusicScan) {
    if stack.len() > MAX_DEPTH || scan.files.len() + scan.skipped.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Hidden files are resource forks and `.DS_Store`, never tracks.
        if name.starts_with('.') {
            continue;
        }
        names.push((name, entry.path()));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path) in names {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            stack.push(name);
            walk_dir(&path, stack, scan);
            stack.pop();
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let rel = rel_join(stack, &name);
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        let stem = name
            .rsplit_once('.')
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| name.clone());
        if let Some(container) = Container::from_ext(&ext) {
            if scan.files.len() + scan.skipped.len() >= MAX_FILES {
                return;
            }
            scan.files.push(MusicFile {
                path,
                rel,
                dirs: stack.clone(),
                stem,
                container,
            });
        } else if UNSUPPORTED_EXTS.contains(&ext.as_str()) {
            scan.skipped.push(SkippedFile {
                rel,
                reason: format!("unsupported container .{ext} — no decoder yet"),
            });
        }
    }
}

fn rel_join(dirs: &[String], name: &str) -> String {
    if dirs.is_empty() {
        return name.to_string();
    }
    format!("{}/{name}", dirs.join("/"))
}

// ---------------------------------------------------------------------------
// container metadata
// ---------------------------------------------------------------------------

/// Text metadata read out of a container. Every field is optional because
/// every field is genuinely absent in real files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<String>,
    pub year: Option<String>,
}

impl TrackTags {
    fn set(slot: &mut Option<String>, value: String) {
        // First writer wins: the first TIT2 in a file is the real one, and
        // appended junk (a second tag, a padded duplicate) is not.
        if slot.is_none() {
            let value = sanitize_text(value.trim(), MAX_TITLE);
            if !value.is_empty() {
                *slot = Some(value);
            }
        }
    }

    fn push(&mut self, key: &str, value: String) {
        match key {
            "TIT2" | "TT2" | "TITLE" => Self::set(&mut self.title, value),
            "TPE1" | "TP1" | "ARTIST" => Self::set(&mut self.artist, value),
            "TPE2" | "TP2" | "ALBUMARTIST" => Self::set(&mut self.artist, value),
            "TALB" | "TAL" | "ALBUM" => Self::set(&mut self.album, value),
            "TRCK" | "TRK" | "TRACKNUMBER" => Self::set(&mut self.track, value),
            "TYER" | "TDRC" | "TYE" | "DATE" => Self::set(&mut self.year, value),
            _ => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.artist.is_none() && self.album.is_none()
    }
}

/// ID3 text metadata: ID3v2.2/2.3/2.4 text frames, falling back to the
/// 128-byte ID3v1 trailer. Total on any input: a malformed tag yields
/// whatever was readable before the damage, never a panic and never an
/// allocation sized from an unchecked header field.
pub fn read_id3(bytes: &[u8]) -> TrackTags {
    let mut tags = read_id3v2(bytes);
    read_id3v1_into(bytes, &mut tags);
    tags
}

fn read_id3v2(bytes: &[u8]) -> TrackTags {
    let mut tags = TrackTags::default();
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" {
        return tags;
    }
    let major = bytes[3];
    if major < 2 || major > 4 {
        return tags;
    }
    let flags = bytes[5];
    let Some(size) = syncsafe_u32(&bytes[6..10]) else {
        return tags;
    };
    let end = (10usize).saturating_add(size as usize).min(bytes.len());
    if end <= 10 {
        return tags;
    }
    let body = &bytes[10..end];
    // Unsynchronisation (v2.2/2.3 whole-tag flag): 0xFF 0x00 pairs stand for
    // a literal 0xFF. Undo it before framing, or frame sizes lie.
    let owned;
    let body: &[u8] = if flags & 0x80 != 0 {
        owned = de_unsynchronise(body);
        &owned
    } else {
        body
    };

    let mut at = 0usize;
    if flags & 0x40 != 0 {
        // Extended header. v2.3 states a size that EXCLUDES its own 4 bytes;
        // v2.4 states a syncsafe size that INCLUDES them.
        if body.len() < 4 {
            return tags;
        }
        at = if major >= 4 {
            syncsafe_u32(&body[0..4]).unwrap_or(0) as usize
        } else {
            4 + u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize
        };
        if at >= body.len() {
            return tags;
        }
    }

    let (id_len, size_len) = if major == 2 { (3usize, 3usize) } else { (4usize, 4usize) };
    let flag_len = if major == 2 { 0usize } else { 2usize };
    let header_len = id_len + size_len + flag_len;
    while at + header_len <= body.len() {
        let id_bytes = &body[at..at + id_len];
        // Padding: the tag is zero-filled to its declared size.
        if id_bytes[0] == 0 {
            break;
        }
        let Ok(id) = std::str::from_utf8(id_bytes) else {
            break;
        };
        let id = id.to_ascii_uppercase();
        let size_at = at + id_len;
        let size = if major == 2 {
            u32::from_be_bytes([0, body[size_at], body[size_at + 1], body[size_at + 2]]) as usize
        } else if major >= 4 {
            match syncsafe_u32(&body[size_at..size_at + 4]) {
                Some(s) => s as usize,
                None => break,
            }
        } else {
            u32::from_be_bytes([
                body[size_at],
                body[size_at + 1],
                body[size_at + 2],
                body[size_at + 3],
            ]) as usize
        };
        let data_at = at + header_len;
        let data_end = match data_at.checked_add(size) {
            Some(end) if end <= body.len() => end,
            // A frame size past the tag end is corruption: keep what we read.
            _ => break,
        };
        if id.starts_with('T') && !id.starts_with("TXX") && size > 0 {
            if let Some(text) = decode_text_frame(&body[data_at..data_end]) {
                tags.push(&id, text);
            }
        }
        at = data_end;
        if size == 0 && header_len == 0 {
            break;
        }
    }
    tags
}

/// The ID3v1 trailer: fixed 128 bytes of space-padded latin-1 at EOF. Only
/// fills fields ID3v2 did not.
fn read_id3v1_into(bytes: &[u8], tags: &mut TrackTags) {
    if bytes.len() < 128 {
        return;
    }
    let tail = &bytes[bytes.len() - 128..];
    if &tail[0..3] != b"TAG" {
        return;
    }
    let field = |from: usize, to: usize| -> String {
        latin1(&tail[from..to])
            .trim_end_matches(|c: char| c == '\u{0}' || c == ' ')
            .to_string()
    };
    tags.push("TIT2", field(3, 33));
    tags.push("TPE1", field(33, 63));
    tags.push("TALB", field(63, 93));
    tags.push("TYER", field(93, 97));
}

/// A syncsafe 32-bit integer: 7 bits per byte, high bit always clear. A set
/// high bit means this is not a syncsafe field, so refuse rather than
/// silently decode a wrong (usually enormous) size.
fn syncsafe_u32(b: &[u8]) -> Option<u32> {
    if b.len() < 4 || b[..4].iter().any(|x| x & 0x80 != 0) {
        return None;
    }
    Some(
        ((b[0] as u32) << 21) | ((b[1] as u32) << 14) | ((b[2] as u32) << 7) | (b[3] as u32),
    )
}

fn de_unsynchronise(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        out.push(body[i]);
        if body[i] == 0xFF && i + 1 < body.len() && body[i + 1] == 0x00 {
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// One ID3 text frame body: an encoding byte then the string.
fn decode_text_frame(data: &[u8]) -> Option<String> {
    let (encoding, rest) = data.split_first()?;
    let text = match encoding {
        0 => latin1(cut_at_nul(rest)),
        1 => utf16_with_bom(rest),
        2 => utf16_be(rest),
        3 => String::from_utf8_lossy(cut_at_nul(rest)).into_owned(),
        // Unknown encoding byte: latin-1 is the only safe reading.
        _ => latin1(cut_at_nul(rest)),
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn cut_at_nul(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(n) => &b[..n],
        None => b,
    }
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn utf16_with_bom(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFF && b[1] == 0xFE {
        return utf16(&b[2..], true);
    }
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        return utf16(&b[2..], false);
    }
    // No BOM on a "UTF-16 with BOM" frame: little-endian is what encoders
    // that get this wrong actually write.
    utf16(b, true)
}

fn utf16_be(b: &[u8]) -> String {
    utf16(b, false)
}

fn utf16(b: &[u8], little_endian: bool) -> String {
    let mut units = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        let unit = if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    char::decode_utf16(units)
        .map(|c| c.unwrap_or('\u{fffd}'))
        .collect()
}

/// Vorbis comments out of an Ogg stream: the `\x03vorbis` comment header's
/// `KEY=value` pairs. Bounded and total.
pub fn read_vorbis_comments(bytes: &[u8]) -> TrackTags {
    let mut tags = TrackTags::default();
    // The comment header sits in the first pages; do not scan a whole album
    // side looking for it.
    let scan_end = bytes.len().min(64 * 1024);
    let Some(start) = find(&bytes[..scan_end], b"\x03vorbis") else {
        return tags;
    };
    let mut at = start + 7;
    let Some(vendor_len) = read_u32_le(bytes, at) else {
        return tags;
    };
    at = match at.checked_add(4 + vendor_len as usize) {
        Some(next) if next <= bytes.len() => next,
        _ => return tags,
    };
    let Some(count) = read_u32_le(bytes, at) else {
        return tags;
    };
    at += 4;
    for _ in 0..count.min(512) {
        let Some(len) = read_u32_le(bytes, at) else {
            return tags;
        };
        at += 4;
        let end = match at.checked_add(len as usize) {
            Some(end) if end <= bytes.len() => end,
            _ => return tags,
        };
        let pair = String::from_utf8_lossy(&bytes[at..end]).into_owned();
        at = end;
        if let Some((key, value)) = pair.split_once('=') {
            tags.push(&key.trim().to_ascii_uppercase(), value.to_string());
        }
    }
    tags
}

fn read_u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    if end > bytes.len() {
        return None;
    }
    Some(u32::from_le_bytes(bytes[at..end].try_into().ok()?))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------------------------------------------------------------------------
// measurement: duration + a real loudness envelope
// ---------------------------------------------------------------------------

/// What the container told us about the audio itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measured {
    pub millis: u32,
    /// Per-granule loudness in [0,1], measured — empty when the container
    /// cannot be read cheaply (then no waveform is drawn from it).
    pub envelope: Vec<f32>,
}

/// Longest MPEG frame walk. 500k frames is ~3.6 hours of MPEG-1 audio; past
/// that a "file" is not a track.
const MAX_MPEG_FRAMES: usize = 500_000;

/// MPEG-1 Layer III bitrates (kbit/s), index 0 = free, 15 = invalid.
const BITRATE_V1_L3: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
/// MPEG-2 / 2.5 Layer III bitrates (kbit/s).
const BITRATE_V2_L3: [u32; 16] =
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
const RATE_V1: [u32; 4] = [44100, 48000, 32000, 0];
const RATE_V2: [u32; 4] = [22050, 24000, 16000, 0];
const RATE_V25: [u32; 4] = [11025, 12000, 8000, 0];

/// One MPEG audio frame header, already validated.
#[derive(Clone, Copy, Debug)]
struct FrameHeader {
    /// True for MPEG-1 (2 granules per frame), false for MPEG-2 / 2.5.
    mpeg1: bool,
    mono: bool,
    crc: bool,
    sample_rate: u32,
    samples: u32,
    length: usize,
}

fn parse_frame_header(b: &[u8]) -> Option<FrameHeader> {
    if b.len() < 4 || b[0] != 0xFF || (b[1] & 0xE0) != 0xE0 {
        return None;
    }
    let version_id = (b[1] >> 3) & 0x03;
    let layer = (b[1] >> 1) & 0x03;
    // Layer III only (bits `01`); reserved version `01` is not audio.
    if layer != 0x01 || version_id == 0x01 {
        return None;
    }
    let mpeg1 = version_id == 0x03;
    let bitrate_index = ((b[2] >> 4) & 0x0F) as usize;
    let rate_index = ((b[2] >> 2) & 0x03) as usize;
    let padding = ((b[2] >> 1) & 0x01) as usize;
    let bitrate = if mpeg1 {
        BITRATE_V1_L3[bitrate_index]
    } else {
        BITRATE_V2_L3[bitrate_index]
    };
    let sample_rate = match version_id {
        0x03 => RATE_V1[rate_index],
        0x02 => RATE_V2[rate_index],
        _ => RATE_V25[rate_index],
    };
    if bitrate == 0 || sample_rate == 0 {
        return None;
    }
    let samples = if mpeg1 { 1152 } else { 576 };
    let length = (samples as usize / 8) * (bitrate as usize * 1000) / sample_rate as usize + padding;
    if length < 4 {
        return None;
    }
    Some(FrameHeader {
        mpeg1,
        mono: ((b[3] >> 6) & 0x03) == 0x03,
        crc: (b[1] & 0x01) == 0,
        sample_rate,
        samples,
        length,
    })
}

/// Where the MPEG stream starts: past any ID3v2 tag, then at the first real
/// frame sync (real-world files carry junk in between).
fn mpeg_audio_start(bytes: &[u8]) -> usize {
    let mut at = 0usize;
    if bytes.len() >= 10 && &bytes[0..3] == b"ID3" {
        if let Some(size) = syncsafe_u32(&bytes[6..10]) {
            let footer = if bytes[5] & 0x10 != 0 { 10 } else { 0 };
            at = (10 + size as usize + footer).min(bytes.len());
        }
    }
    // Two consecutive parseable frames is the sync test; a lone 0xFFE pair
    // inside cover art is not a stream.
    let limit = (at + 128 * 1024).min(bytes.len());
    let mut probe = at;
    while probe + 4 <= limit {
        if let Some(head) = parse_frame_header(&bytes[probe..]) {
            let next = probe + head.length;
            if next + 4 > bytes.len() || parse_frame_header(&bytes[next..]).is_some() {
                return probe;
            }
        }
        probe += 1;
    }
    at
}

/// Measure an MPEG Layer III stream: exact duration from the frame walk (or
/// the Xing/VBRI frame count when the walk would be wasteful), plus a real
/// loudness envelope read out of each granule's `global_gain` field.
///
/// `global_gain` is the decoder's per-granule output scale — it is the
/// audio's own loudness, not a fabricated shape, which is why this is a
/// legitimate waveform source without a full decoder.
pub fn measure_mp3(bytes: &[u8]) -> Option<Measured> {
    let start = mpeg_audio_start(bytes);
    let first = parse_frame_header(bytes.get(start..)?)?;
    let mut at = start;
    let mut frames = 0usize;
    let mut samples: u64 = 0;
    let mut envelope: Vec<f32> = Vec::new();
    let mut peak_gain = 0u8;
    while at + 4 <= bytes.len() && frames < MAX_MPEG_FRAMES {
        let Some(head) = parse_frame_header(&bytes[at..]) else {
            // Lost sync: a tail tag (ID3v1/APE) or garbage. Stop here — the
            // frames counted so far are real.
            break;
        };
        if at + head.length > bytes.len() {
            break;
        }
        for gain in granule_gains(&bytes[at..at + head.length], &head) {
            peak_gain = peak_gain.max(gain);
            envelope.push(gain as f32);
        }
        samples += head.samples as u64;
        frames += 1;
        at += head.length;
    }
    if frames == 0 {
        return None;
    }
    // A Xing/Info/VBRI header states the true frame count of a VBR stream
    // including any frames the walk lost sync on; prefer it when it agrees
    // in magnitude with what we walked.
    let millis = match xing_frames(&bytes[start..], &first) {
        Some(stated) if stated as usize >= frames => {
            // The header frame itself carries no audio.
            ((stated as u64).saturating_sub(1) * first.samples as u64 * 1000
                / first.sample_rate as u64) as u32
        }
        _ => (samples * 1000 / first.sample_rate as u64) as u32,
    };
    if millis == 0 {
        return None;
    }
    Some(Measured {
        millis,
        envelope: normalise_gains(envelope, peak_gain),
    })
}

/// `global_gain` for every granule/channel of one frame, in stream order.
/// Empty when the side info is not where the header says it is.
fn granule_gains(frame: &[u8], head: &FrameHeader) -> Vec<u8> {
    let side_at = 4 + if head.crc { 2 } else { 0 };
    let channels = if head.mono { 1 } else { 2 };
    let (side_len, base_bits, block_bits, granules) = if head.mpeg1 {
        let side_len = if head.mono { 17 } else { 32 };
        let base = if head.mono { 9 + 5 + 4 } else { 9 + 3 + 8 };
        (side_len, base, 59usize, 2usize)
    } else {
        let side_len = if head.mono { 9 } else { 17 };
        let base = if head.mono { 8 + 1 } else { 8 + 2 };
        (side_len, base, 63usize, 1usize)
    };
    if frame.len() < side_at + side_len {
        return Vec::new();
    }
    let side = &frame[side_at..side_at + side_len];
    let mut out = Vec::with_capacity(granules * channels);
    for granule in 0..granules {
        for channel in 0..channels {
            // Inside each granule/channel block: part2_3_length (12 bits) +
            // big_values (9 bits) come first, then global_gain (8 bits).
            let bit = base_bits + (granule * channels + channel) * block_bits + 12 + 9;
            out.push(read_bits8(side, bit));
        }
    }
    out
}

/// Eight bits at `bit` (MSB-first) out of `data`; zero past the end.
fn read_bits8(data: &[u8], bit: usize) -> u8 {
    let mut out = 0u8;
    for i in 0..8 {
        let index = bit + i;
        let byte = index / 8;
        let value = data
            .get(byte)
            .map(|b| (b >> (7 - (index % 8))) & 1)
            .unwrap_or(0);
        out = (out << 1) | value;
    }
    out
}

/// `global_gain` is a log scale: amplitude ∝ 2^((gain-210)/4). Convert, then
/// normalise against the loudest granule so quiet tracks still draw.
fn normalise_gains(gains: Vec<f32>, peak: u8) -> Vec<f32> {
    if gains.is_empty() || peak == 0 {
        return Vec::new();
    }
    let peak_amp = 2f32.powf((peak as f32 - 210.0) / 4.0);
    if !(peak_amp > 0.0) {
        return Vec::new();
    }
    gains
        .into_iter()
        .map(|g| (2f32.powf((g - 210.0) / 4.0) / peak_amp).clamp(0.0, 1.0))
        .collect()
}

/// The stated frame count from a Xing/Info or VBRI header in the first frame.
fn xing_frames(stream: &[u8], head: &FrameHeader) -> Option<u32> {
    let side_len = if head.mpeg1 {
        if head.mono {
            17
        } else {
            32
        }
    } else if head.mono {
        9
    } else {
        17
    };
    let xing_at = 4 + side_len;
    if stream.len() >= xing_at + 12 {
        let tag = &stream[xing_at..xing_at + 4];
        if tag == b"Xing" || tag == b"Info" {
            let flags = u32::from_be_bytes(stream[xing_at + 4..xing_at + 8].try_into().ok()?);
            if flags & 0x01 != 0 {
                return Some(u32::from_be_bytes(
                    stream[xing_at + 8..xing_at + 12].try_into().ok()?,
                ));
            }
        }
    }
    // Fraunhofer VBRI sits at a fixed offset instead of after the side info.
    if stream.len() >= 36 + 18 && &stream[36..40] == b"VBRI" {
        return Some(u32::from_be_bytes(stream[50..54].try_into().ok()?));
    }
    None
}

/// Ogg duration: the last page's granule position (a sample count) over the
/// identification header's sample rate. No Vorbis decode needed.
pub fn measure_ogg(bytes: &[u8]) -> Option<Measured> {
    let head_end = bytes.len().min(64 * 1024);
    let ident = find(&bytes[..head_end], b"\x01vorbis")?;
    let rate = read_u32_le(bytes, ident + 12)?;
    if rate == 0 {
        return None;
    }
    // Walk back from EOF to the last capture pattern. A page header is 27
    // bytes plus its segment table, so the last one is close to the end.
    let mut at = bytes.len().saturating_sub(4);
    let floor = bytes.len().saturating_sub(128 * 1024);
    while at >= floor {
        if bytes[at..].starts_with(b"OggS") && at + 14 <= bytes.len() {
            let granule = u64::from_le_bytes(bytes[at + 6..at + 14].try_into().ok()?);
            // -1 marks "no packet finishes on this page".
            if granule != u64::MAX {
                let millis = (granule.saturating_mul(1000) / rate as u64).min(u32::MAX as u64);
                return (millis > 0).then_some(Measured {
                    millis: millis as u32,
                    envelope: Vec::new(),
                });
            }
        }
        if at == 0 {
            break;
        }
        at -= 1;
    }
    None
}

// ---------------------------------------------------------------------------
// thumbnails
// ---------------------------------------------------------------------------

/// The 512² preview for one track: a waveform whenever the container gives
/// real amplitude (RIFF PCM decoded outright, MPEG granule gains read from
/// the side info), else a deterministic per-track card so the DJ grid still
/// reads as a grid of distinct tracks.
pub fn track_thumbnail(bytes: &[u8], container: Container, measured: &Measured, key: &str) -> Vec<u32> {
    track_picture(bytes, container, measured, key).pixels
}

/// The picture for one track, with its size: the high-definition
/// spectrogram whenever the container decodes to real samples — MP3 and
/// Ogg included, this app carries its own decoders — else the 512² envelope
/// strip, else a deterministic per-track card so the DJ grid still reads as
/// a grid of distinct tracks.
pub fn track_picture(
    bytes: &[u8],
    container: Container,
    measured: &Measured,
    key: &str,
) -> TrackPicture {
    let media = match container {
        Container::Wav => Some(makepad_asset_data::MediaType::Wav),
        Container::Mp3 => Some(makepad_asset_data::MediaType::Mp3),
        Container::Ogg => Some(makepad_asset_data::MediaType::Ogg),
        _ => None,
    };
    if let Some(media) = media {
        if let Ok(pcm) = crate::thumbs::decode_audio(bytes, media) {
            if let Some((pixels, w, h, regions)) = crate::thumbs::audio_picture_hd(&pcm) {
                return TrackPicture {
                    pixels,
                    width: w,
                    height: h,
                    views: crate::thumbs::audio_views(regions),
                };
            }
        }
    }
    if !measured.envelope.is_empty() {
        // The envelope strip is a wave picture, and says so: a preview that
        // wants the wave has the whole picture, and one that hoped for a
        // spectrogram learns there is none rather than reading this as one.
        return TrackPicture {
            pixels: envelope_bgra_512(&measured.envelope),
            width: THUMB_DIM,
            height: THUMB_DIM,
            views: vec![ThumbnailView::rect(
                ThumbnailViewKind::Wave,
                0,
                0,
                THUMB_DIM as u32,
                THUMB_DIM as u32,
            )],
        };
    }
    // A colour card is not a picture OF the audio, so it declares nothing.
    TrackPicture {
        pixels: card_bgra_512(key),
        width: THUMB_DIM,
        height: THUMB_DIM,
        views: Vec::new(),
    }
}

/// A track's baked picture and what its regions are.
pub struct TrackPicture {
    pub pixels: Vec<u32>,
    pub width: usize,
    pub height: usize,
    pub views: Vec<ThumbnailView>,
}

const WAVE_BG: u32 = 0xff14_181c;
const WAVE_FG: u32 = 0xff58_c4a0;
const WAVE_MID: u32 = 0xff2a_3238;

/// A symmetric waveform strip from a normalised [0,1] loudness envelope.
pub fn envelope_bgra_512(envelope: &[f32]) -> Vec<u32> {
    let (width, height) = (THUMB_DIM, THUMB_DIM);
    let mut out = vec![WAVE_BG; width * height];
    let mid_y = height / 2;
    for x in 0..width {
        out[mid_y * width + x] = WAVE_MID;
    }
    if envelope.is_empty() {
        return out;
    }
    let per_col = (envelope.len() as f64 / width as f64).max(1.0);
    let half = (height / 2) as f32;
    for x in 0..width {
        let start = ((x as f64 * per_col) as usize).min(envelope.len() - 1);
        let end = (((x + 1) as f64 * per_col) as usize).clamp(start + 1, envelope.len());
        let peak = envelope[start..end].iter().fold(0.0f32, |a, b| a.max(*b));
        let reach = (peak.clamp(0.0, 1.0) * (half - 1.0)) as usize;
        let y0 = mid_y.saturating_sub(reach);
        let y1 = (mid_y + reach).min(height - 1);
        for y in y0..=y1 {
            out[y * width + x] = WAVE_FG;
        }
    }
    out
}

/// A deterministic per-track card: the alias hue with a darker border. Same
/// shape (and same reason) as the games importer's placeholder — the catalog
/// requires SOME thumbnail, and a stable colour beats a black tile.
pub fn card_bgra_512(key: &str) -> Vec<u32> {
    let (r, g, b) = key_rgb(key);
    let border = 0xff00_0000
        | ((r as u32 / 3) << 16)
        | ((g as u32 / 3) << 8)
        | (b as u32 / 3);
    let mut out = vec![WAVE_BG; THUMB_DIM * THUMB_DIM];
    for y in 0..THUMB_DIM {
        for x in 0..THUMB_DIM {
            let edge = x < 12 || y < 12 || x + 12 >= THUMB_DIM || y + 12 >= THUMB_DIM;
            let shade = 200u32.saturating_sub((y / 4) as u32);
            let pixel = if edge {
                border
            } else {
                0xff00_0000
                    | (((r as u32 * shade) / 255) << 16)
                    | (((g as u32 * shade) / 255) << 8)
                    | ((b as u32 * shade) / 255)
            };
            out[y * THUMB_DIM + x] = pixel;
        }
    }
    out
}

fn key_rgb(key: &str) -> (u8, u8, u8) {
    let mut h = 2166136261u32;
    for b in key.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(16777619);
    }
    hsv((h % 360) as i32, 0.5, 0.8)
}

fn hsv(h: i32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - (((h as f32 / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

// ---------------------------------------------------------------------------
// the plan: names, tags, aliases
// ---------------------------------------------------------------------------

/// One track ready to publish: everything derived from the file and its path,
/// with nothing left to guess at publish time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedTrack {
    pub rel: String,
    pub path: PathBuf,
    pub container: Container,
    /// Catalog title: the container's title, else the file stem.
    pub title: String,
    pub artist: String,
    pub album: String,
    /// `<namespace>/music/<artist>/<title>`, with a path-digest suffix when
    /// two tracks in this tree would otherwise claim the same name.
    pub alias: String,
    /// `music` + every relative directory name + the container's artist and
    /// album, slugified and deduplicated.
    pub tags: Vec<String>,
}

/// Every relative directory name under the root, plus the constant `music`
/// tag and (when the container knows them) the artist and album. Slugified to
/// the catalog label charset, deduplicated, order-stable, bounded.
pub fn music_tags(dirs: &[String], artist: &str, album: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![MUSIC_TAG.to_string()];
    let mut push = |raw: &str| {
        let slug = alias_slug(raw, MAX_TAG_BYTES);
        if !slug.is_empty() && !out.contains(&slug) && out.len() < MAX_TAGS {
            out.push(slug);
        }
    };
    for dir in dirs {
        push(dir);
    }
    push(artist);
    push(album);
    out
}

/// Bytes read from the head of a file to find its metadata. An ID3v2 tag with
/// embedded cover art is a few hundred KB at worst; a Vorbis comment header
/// sits in the first pages. Planning a 5000-track library must not read the
/// whole library — that is the difference between a card that starts moving
/// and one that looks dead for minutes.
const METADATA_HEAD_BYTES: u64 = 1 << 20;

/// The metadata window of one file: a bounded head plus the 128-byte ID3v1
/// trailer, which is at the very END of the file by definition.
fn read_metadata_window(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len == 0 {
        return Err("empty file".into());
    }
    if len <= METADATA_HEAD_BYTES {
        let mut all = Vec::with_capacity(len as usize);
        file.read_to_end(&mut all).map_err(|e| e.to_string())?;
        return Ok(all);
    }
    let mut head = vec![0u8; METADATA_HEAD_BYTES as usize];
    file.read_exact(&mut head).map_err(|e| e.to_string())?;
    let mut tail = [0u8; 128];
    file.seek(SeekFrom::End(-128)).map_err(|e| e.to_string())?;
    file.read_exact(&mut tail).map_err(|e| e.to_string())?;
    head.extend_from_slice(&tail);
    Ok(head)
}

/// Turn a scan into publishable tracks: read each file's metadata, name it,
/// and resolve alias collisions across the whole batch at once.
///
/// `progress(done, total, rel)` is called per file — planning a big library is
/// minutes of work and must not look like a hang.
pub fn plan_tracks(
    root: &Path,
    scan: &MusicScan,
    namespace: &str,
    progress: &mut dyn FnMut(usize, usize, &str),
    cancel: &dyn Fn() -> bool,
) -> (Vec<PlannedTrack>, Vec<SkippedFile>) {
    let mut skipped = scan.skipped.clone();
    let mut planned = Vec::new();
    let total = scan.files.len();
    for (index, file) in scan.files.iter().enumerate() {
        if cancel() {
            break;
        }
        progress(index, total, &file.rel);
        // Only the metadata window: the payload is read again at publish
        // time, once, for the file that is actually going up.
        let bytes = match read_metadata_window(&file.path) {
            Ok(bytes) => bytes,
            Err(error) => {
                skipped.push(SkippedFile {
                    rel: file.rel.clone(),
                    reason: error,
                });
                continue;
            }
        };
        let tags = match file.container {
            Container::Mp3 => read_id3(&bytes),
            Container::Ogg => read_vorbis_comments(&bytes),
            Container::Wav => TrackTags::default(),
        };
        // Directory names are the fallback identity, in the layout a music
        // library actually has: `<Artist>/<Album>/<track>`.
        let dir_artist = file.dirs.first().cloned().unwrap_or_default();
        let dir_album = if file.dirs.len() >= 2 {
            file.dirs[file.dirs.len() - 1].clone()
        } else {
            String::new()
        };
        let title = tags
            .title
            .clone()
            .unwrap_or_else(|| sanitize_text(file.stem.trim(), MAX_TITLE));
        let artist = tags.artist.clone().unwrap_or(dir_artist);
        let album = tags.album.clone().unwrap_or(dir_album);
        let _ = root;
        planned.push(PlannedTrack {
            rel: file.rel.clone(),
            path: file.path.clone(),
            container: file.container,
            title: if title.is_empty() {
                file.stem.clone()
            } else {
                title
            },
            artist: artist.clone(),
            album: album.clone(),
            alias: String::new(),
            tags: music_tags(&file.dirs, &artist, &album),
        });
    }
    progress(total, total, "");
    assign_aliases(&mut planned, namespace);
    (planned, skipped)
}

/// Give every track its alias, disambiguating only where it is needed.
///
/// The alias is deliberately content-INDEPENDENT: re-encoding a track must
/// produce a new REVISION under the same name, not a second asset. When two
/// distinct files in the tree would claim one name (the same song on two
/// albums), every claimant gains a suffix from its RELATIVE PATH digest, so
/// the outcome is the same on every run over the same tree.
fn assign_aliases(tracks: &mut [PlannedTrack], namespace: &str) {
    let base: Vec<String> = tracks
        .iter()
        .map(|track| music_alias(namespace, &track.artist, &track.title, None))
        .collect();
    let mut collided: Vec<bool> = vec![false; tracks.len()];
    for i in 0..tracks.len() {
        for j in (i + 1)..tracks.len() {
            if base[i] == base[j] {
                collided[i] = true;
                collided[j] = true;
            }
        }
    }
    for (index, track) in tracks.iter_mut().enumerate() {
        track.alias = if collided[index] {
            let digest = to_hex(BlobId::hash_of(track.rel.as_bytes()).as_bytes());
            music_alias(
                namespace,
                &track.artist,
                &track.title,
                Some(&digest[..ALIAS_DIGEST_HEX]),
            )
        } else {
            base[index].clone()
        };
    }
}

/// `<namespace>/music/<artist>/<title>[-<digest>]`, always inside the alias
/// contract: each segment `[a-z0-9][a-z0-9_-]*` and ≤48 bytes, the whole
/// thing ≤128 bytes. A namespace long enough to crowd the name shortens the
/// name, never the identity. A namespace that is ALREADY `music` does not
/// repeat itself: `music/adana-twins/strange`, not `music/music/…`.
pub fn music_alias(namespace: &str, artist: &str, title: &str, digest: Option<&str>) -> String {
    let class = (namespace != MUSIC_ALIAS_CLASS).then_some(MUSIC_ALIAS_CLASS);
    let suffix = digest.map(|d| d.len() + 1).unwrap_or(0);
    let head = namespace.len() + 1 + class.map(|c| c.len() + 1).unwrap_or(0);
    // Split what is left between the two human segments, artist first.
    let room = MAX_ALIAS_BYTES.saturating_sub(head + 2 + suffix);
    let artist_budget = MAX_ALIAS_SEGMENT.min(room / 2);
    let artist_slug = non_empty(alias_slug(artist, artist_budget), "unknown-artist");
    let title_room = room.saturating_sub(artist_slug.len());
    let title_budget = MAX_ALIAS_SEGMENT.saturating_sub(suffix).min(title_room);
    let mut title_slug = non_empty(alias_slug(title, title_budget), "untitled");
    if let Some(digest) = digest {
        title_slug = format!("{title_slug}-{digest}");
    }
    match class {
        Some(class) => format!("{namespace}/{class}/{artist_slug}/{title_slug}"),
        None => format!("{namespace}/{artist_slug}/{title_slug}"),
    }
}

fn non_empty(slug: String, fallback: &str) -> String {
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

// ---------------------------------------------------------------------------
// publication
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackOutcome {
    Published,
    Updated,
    Unchanged,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MusicReport {
    /// Aliases of newly created assets.
    pub published: Vec<String>,
    /// Aliases whose bytes changed: a new revision of the same asset.
    pub updated: Vec<String>,
    /// Aliases already holding exactly these bytes.
    pub unchanged: Vec<String>,
    pub failed: Vec<(String, String)>,
    /// Files seen and deliberately not published, with the reason.
    pub skipped: Vec<(String, String)>,
    /// True when the caller's cancel flag stopped the run early.
    pub cancelled: bool,
}

impl MusicReport {
    pub fn landed(&self) -> usize {
        self.published.len() + self.updated.len() + self.unchanged.len()
    }
}

/// Which half of the run a progress report belongs to. A library import is
/// two passes over the tree and the UI must not show one bar that fills
/// twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MusicStage {
    /// Walking the tree and reading each file's metadata window.
    Reading,
    /// Uploading and publishing one track at a time.
    Publishing,
}

/// One progress report. `total` is 0 only before the walk has finished.
#[derive(Clone, Copy, Debug)]
pub struct MusicProgress<'a> {
    pub stage: MusicStage,
    pub done: usize,
    pub total: usize,
    /// The file (reading) or the track title (publishing) in hand.
    pub current: &'a str,
}

/// Publish every audio file under `root` into `namespace`.
///
/// `progress` is called for every file of both passes so a UI can draw an
/// honest bar from the first second; `cancel()` is polled throughout and
/// stops the run with `cancelled: true` and whatever landed so far — never a
/// rollback, the catalog rows already published are real.
pub fn import_music(
    client: &mut AssetClient,
    root: &Path,
    namespace: &str,
    rights: &PublishRights,
    log: bool,
    progress: &mut dyn FnMut(MusicProgress),
    cancel: &dyn Fn() -> bool,
) -> Result<MusicReport, String> {
    progress(MusicProgress {
        stage: MusicStage::Reading,
        done: 0,
        total: 0,
        current: "",
    });
    let scan = scan_music(root);
    if scan.files.is_empty() && scan.skipped.is_empty() {
        return Err(format!("no audio files under {}", root.display()));
    }
    let (tracks, skipped) = {
        let mut on_file = |done: usize, total: usize, rel: &str| {
            progress(MusicProgress {
                stage: MusicStage::Reading,
                done,
                total,
                current: rel,
            })
        };
        plan_tracks(root, &scan, namespace, &mut on_file, cancel)
    };
    let mut report = MusicReport {
        skipped: skipped
            .into_iter()
            .map(|s| (s.rel, s.reason))
            .collect(),
        ..MusicReport::default()
    };
    if cancel() {
        report.cancelled = true;
        return Ok(report);
    }
    if tracks.is_empty() {
        return Ok(report);
    }
    let total = tracks.len();
    let started = std::time::Instant::now();
    // Decode + bake fan out; publishing stays on this thread because the
    // client is one connection and the catalog wants one writer. Workers
    // claim tracks from a shared cursor, so a six-minute track and a
    // twelve-second sting do not wait for each other — and the bake of the
    // long one already spreads its own columns across cores. The channel is
    // bounded, so at most a few tracks' audio is ever resident.
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 4).max(2))
        .unwrap_or(2)
        .min(tracks.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    // The cancel flag crosses into the bakers as a plain bool: the caller's
    // closure is not `Sync`, and a baker only needs to know "stop", which
    // the publishing thread already asks on every track.
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, Result<BakedTrack, String>)>(workers * 2);
    let mut audio_secs = 0.0f64;
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let (next, tracks, tx, stop) = (&next, &tracks, tx.clone(), &stop);
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(track) = tracks.get(index) else { break };
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    if tx.send((index, bake_track(track))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut done = 0usize;
        while let Ok((index, baked)) = rx.recv() {
            let track = &tracks[index];
            if cancel() {
                report.cancelled = true;
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
            progress(MusicProgress {
                stage: MusicStage::Publishing,
                done,
                total,
                current: &track.title,
            });
            done += 1;
            let outcome = match baked {
                Ok(baked) => {
                    audio_secs += baked.measured.millis as f64 / 1000.0;
                    publish_baked(client, track, baked, namespace, rights)
                }
                Err(error) => Err(error),
            };
            match outcome {
            Ok(TrackOutcome::Published) => {
                if log {
                    eprintln!("[music-import] published {}", track.alias);
                }
                report.published.push(track.alias.clone());
            }
            Ok(TrackOutcome::Updated) => {
                if log {
                    eprintln!("[music-import] new revision {}", track.alias);
                }
                report.updated.push(track.alias.clone());
            }
            Ok(TrackOutcome::Unchanged) => {
                if log {
                    eprintln!("[music-import] unchanged {}", track.alias);
                }
                report.unchanged.push(track.alias.clone());
            }
            Err(error) => {
                if log {
                    eprintln!("[music-import] FAILED {}: {error}", track.rel);
                }
                report.failed.push((track.rel.clone(), error));
            }
            }
        }
    });
    if log {
        eprintln!(
            "[music-import] {total} tracks, {:.0}s of audio, {:.1}s wall on {workers} bakers",
            audio_secs,
            started.elapsed().as_secs_f64()
        );
    }
    progress(MusicProgress {
        stage: MusicStage::Publishing,
        done: total,
        total,
        current: "",
    });
    Ok(report)
}

/// Publish one track: skip when the alias head already carries exactly these
/// bytes, re-publish as a new revision of the same asset when they changed.
/// One track read, measured and pictured — everything a publish needs that
/// costs CPU, done off the publishing thread.
pub struct BakedTrack {
    bytes: Vec<u8>,
    pub measured: Measured,
    thumbnail_bytes: Vec<u8>,
    dims: (usize, usize),
    /// What the baked picture's regions ARE — declared on the published
    /// thumbnail so a preview reads the layout instead of measuring it.
    views: Vec<ThumbnailView>,
}

/// Read, decode, measure and bake one track's picture. No network, no
/// client: this is the part that fans out across cores.
pub fn bake_track(track: &PlannedTrack) -> Result<BakedTrack, String> {
    let bytes = std::fs::read(&track.path).map_err(|e| format!("read: {e}"))?;
    if bytes.is_empty() {
        return Err("empty file".into());
    }
    let measured = measure(&bytes, track.container)
        .ok_or_else(|| "unmeasurable duration — not published without one".to_string())?;
    let picture = track_picture(&bytes, track.container, &measured, &track.alias);
    let thumbnail_bytes = encode_jpeg_bgra(&picture.pixels, picture.width, picture.height)?;
    Ok(BakedTrack {
        bytes,
        measured,
        thumbnail_bytes,
        dims: (picture.width, picture.height),
        views: picture.views,
    })
}

pub fn publish_track(
    client: &mut AssetClient,
    track: &PlannedTrack,
    namespace: &str,
    rights: &PublishRights,
) -> Result<TrackOutcome, String> {
    publish_baked(client, track, bake_track(track)?, namespace, rights)
}

/// Publish what a baker prepared: probe the alias, decide whether anything
/// actually changed, and write a revision when it did.
pub fn publish_baked(
    client: &mut AssetClient,
    track: &PlannedTrack,
    baked: BakedTrack,
    namespace: &str,
    rights: &PublishRights,
) -> Result<TrackOutcome, String> {
    let alias = AssetAlias::from_str(&track.alias)
        .map_err(|e| format!("{}: alias {}: {e}", track.rel, track.alias))?;
    let BakedTrack { bytes, measured, thumbnail_bytes, dims: (pic_w, pic_h), views } = baked;
    let audio_blob = BlobId::hash_of(&bytes);
    // The picture is baked BEFORE anything is decided: a re-import is how a
    // track gets today's imagery, so "unchanged" has to mean the audio AND
    // the picture are what this importer produces now. Comparing the baked
    // bytes is exact — no version marker to forget to bump, and no shape
    // heuristic that loops forever on a track whose picture legitimately is
    // not a spectrogram.
    let thumbnail_blob = BlobId::hash_of(&thumbnail_bytes);
    let existing = match client.resolve_alias(&alias) {
        Ok(head) => {
            let manifest = client
                .fetch_asset_manifest(&head.head_revision)
                .map_err(|e| format!("head manifest: {e}"))?;
            let same_audio = manifest
                .files
                .iter()
                .any(|f| f.role == FileRole::Audio && f.blob == audio_blob);
            let same_picture = manifest
                .thumbnail
                .is_some_and(|thumb| thumb.blob == thumbnail_blob);
            if same_audio && same_picture {
                return Ok(TrackOutcome::Unchanged);
            }
            // The audio blob is content-addressed, so a picture-only change
            // re-uploads nothing: only the thumbnail and the manifest.
            Some(head.asset_id)
        }
        Err(ClientError::NotFound { .. }) => None,
        Err(error) => return Err(format!("alias probe: {error}")),
    };
    let thumbnail = PublishThumbnail {
        bytes: thumbnail_bytes,
        media: ThumbnailMedia::Jpeg,
        width: pic_w as u32,
        height: pic_h as u32,
        views,
    };
    let mut request = PublishRequest::new(
        namespace,
        AssetKind::Audio,
        sanitize_text(&track.title, MAX_TITLE),
        PublishFile {
            bytes,
            media: track.container.media(),
            role: FileRole::Audio,
            media_millis: measured.millis,
            dims: None,
        },
        thumbnail,
    );
    request.description = sanitize_text(&describe(track), MAX_DESCRIPTION);
    request.alias = Some(alias);
    request.asset_id = existing;
    request.categories = vec![MUSIC_CATEGORY.into()];
    request.tags = track.tags.clone();
    request.creator = sanitize_text(&track.artist, MAX_TITLE);
    request.generator = "music_import".into();
    request.backend = "asset-importer".into();
    request.model = sanitize_text(&track.album, MAX_TITLE);
    request.provenance = sanitize_text(&format!("music-import {}", track.rel), MAX_DESCRIPTION);
    request.rights = rights.clone();
    client
        .publish_artifact(&request)
        .map_err(|e| format!("publish: {e}"))?;
    Ok(if existing.is_some() {
        TrackOutcome::Updated
    } else {
        TrackOutcome::Published
    })
}

/// Duration + envelope for one container.
pub fn measure(bytes: &[u8], container: Container) -> Option<Measured> {
    match container {
        Container::Mp3 => measure_mp3(bytes),
        Container::Ogg => measure_ogg(bytes),
        Container::Wav => {
            let pcm = parse_wav(bytes).ok()?;
            let millis = pcm.millis();
            (millis > 0).then_some(Measured {
                millis,
                envelope: Vec::new(),
            })
        }
    }
}

fn describe(track: &PlannedTrack) -> String {
    match (track.artist.is_empty(), track.album.is_empty()) {
        (false, false) => format!("{} · {}", track.artist, track.album),
        (false, true) => track.artist.clone(),
        (true, false) => track.album.clone(),
        (true, true) => track.rel.clone(),
    }
}

/// The default terms for a personal music library: content the operator holds
/// a copy of, servable on their own LAN, never redistributed off it, and
/// derivable only for local preview (the waveform thumbnail this importer
/// makes is exactly such a derivative). There is no blanket grant here and
/// none is invented — a caller who knows better terms passes them in.
pub fn personal_library_rights(root: &Path) -> PublishRights {
    PublishRights::declared(
        "All-Rights-Reserved",
        "",
        format!("local music library {}", root.display()),
        makepad_asset_data::Redistribution::LanLocal,
        makepad_asset_data::DerivativePolicy::LocalPreview,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "music-import-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&root);
        root
    }

    // ---- fixtures ---------------------------------------------------------

    /// A syncsafe 32-bit size field.
    fn syncsafe(mut n: u32) -> [u8; 4] {
        let mut out = [0u8; 4];
        for slot in out.iter_mut().rev() {
            *slot = (n & 0x7F) as u8;
            n >>= 7;
        }
        out
    }

    /// One ID3v2 text frame (v2.3/v2.4 framing).
    fn text_frame(major: u8, id: &str, encoding: u8, payload: &[u8]) -> Vec<u8> {
        let mut body = vec![encoding];
        body.extend_from_slice(payload);
        let mut out = id.as_bytes().to_vec();
        if major >= 4 {
            out.extend_from_slice(&syncsafe(body.len() as u32));
        } else {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        }
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&body);
        out
    }

    fn id3v2(major: u8, frames: Vec<u8>) -> Vec<u8> {
        let mut out = b"ID3".to_vec();
        out.extend_from_slice(&[major, 0, 0]);
        out.extend_from_slice(&syncsafe(frames.len() as u32));
        out.extend_from_slice(&frames);
        out
    }

    fn id3v1(title: &str, artist: &str, album: &str, year: &str) -> Vec<u8> {
        let mut out = b"TAG".to_vec();
        let mut field = |text: &str, len: usize| {
            let mut bytes = text.as_bytes().to_vec();
            bytes.resize(len, 0);
            out.extend_from_slice(&bytes);
        };
        field(title, 30);
        field(artist, 30);
        field(album, 30);
        field(year, 4);
        out.resize(128, 0);
        out
    }

    /// One MPEG-1 Layer III frame at 128 kbit/s, 44.1 kHz, joint stereo, with
    /// `gain` written into every granule's `global_gain` field.
    fn mp3_frame(gain: u8) -> Vec<u8> {
        // 0xFF 0xFB = sync + MPEG-1 + Layer III + no CRC.
        // 0x90 = bitrate index 9 (128k), rate index 0 (44100), no padding.
        // 0x04 = joint stereo.
        let mut frame = vec![0xFF, 0xFB, 0x90, 0x04];
        let mut side = [0u8; 32];
        // Stereo MPEG-1 side info: 9 + 3 + 8 header bits, then 59 bits per
        // (granule, channel); global_gain sits 21 bits into each block.
        for granule in 0..2 {
            for channel in 0..2 {
                let bit = 20 + (granule * 2 + channel) * 59 + 12 + 9;
                for i in 0..8 {
                    let value = (gain >> (7 - i)) & 1;
                    let index = bit + i;
                    if value != 0 {
                        side[index / 8] |= 1 << (7 - (index % 8));
                    }
                }
            }
        }
        frame.extend_from_slice(&side);
        // 144 * 128000 / 44100 = 417 bytes, no padding.
        frame.resize(417, 0);
        frame
    }

    fn mp3_file(frames: usize, gain: u8, tag: Option<Vec<u8>>) -> Vec<u8> {
        let mut out = tag.unwrap_or_default();
        for _ in 0..frames {
            out.extend_from_slice(&mp3_frame(gain));
        }
        out
    }

    fn wav_file(frames: usize, rate: u32) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..frames {
            let sample = ((i as f32 * 0.1).sin() * 16000.0) as i16;
            data.extend_from_slice(&sample.to_le_bytes());
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    // ---- ID3 --------------------------------------------------------------

    #[test]
    fn id3v23_latin1_text_frames() {
        let mut frames = text_frame(3, "TIT2", 0, b"Strange\0");
        frames.extend(text_frame(3, "TPE1", 0, b"Adana Twins"));
        frames.extend(text_frame(3, "TALB", 0, b"Strange EP"));
        frames.extend(text_frame(3, "TRCK", 0, b"2/5"));
        let bytes = mp3_file(1, 180, Some(id3v2(3, frames)));
        let tags = read_id3(&bytes);
        assert_eq!(tags.title.as_deref(), Some("Strange"));
        assert_eq!(tags.artist.as_deref(), Some("Adana Twins"));
        assert_eq!(tags.album.as_deref(), Some("Strange EP"));
        assert_eq!(tags.track.as_deref(), Some("2/5"));
    }

    #[test]
    fn id3v24_utf8_and_syncsafe_sizes() {
        let mut frames = text_frame(4, "TIT2", 3, "Röyksopp Forever".as_bytes());
        frames.extend(text_frame(4, "TPE1", 3, "Röyksopp".as_bytes()));
        let bytes = mp3_file(1, 180, Some(id3v2(4, frames)));
        let tags = read_id3(&bytes);
        assert_eq!(tags.title.as_deref(), Some("Röyksopp Forever"));
        assert_eq!(tags.artist.as_deref(), Some("Röyksopp"));
    }

    #[test]
    fn id3v23_utf16_with_and_without_bom() {
        let le: Vec<u8> = [0xFF, 0xFE]
            .iter()
            .copied()
            .chain("Neon".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let be: Vec<u8> = [0xFE, 0xFF]
            .iter()
            .copied()
            .chain("Drift".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let mut frames = text_frame(3, "TIT2", 1, &le);
        frames.extend(text_frame(3, "TALB", 1, &be));
        // Encoding 2 is UTF-16BE with no BOM at all.
        let raw_be: Vec<u8> = "Axes".encode_utf16().flat_map(u16::to_be_bytes).collect();
        frames.extend(text_frame(3, "TPE1", 2, &raw_be));
        let tags = read_id3(&id3v2(3, frames));
        assert_eq!(tags.title.as_deref(), Some("Neon"));
        assert_eq!(tags.album.as_deref(), Some("Drift"));
        assert_eq!(tags.artist.as_deref(), Some("Axes"));
    }

    #[test]
    fn id3v1_fills_only_what_v2_left_open() {
        let frames = text_frame(3, "TIT2", 0, b"From v2");
        let mut bytes = id3v2(3, frames);
        bytes.extend(mp3_file(1, 180, None));
        bytes.extend(id3v1("From v1", "V1 Artist", "V1 Album", "1999"));
        let tags = read_id3(&bytes);
        assert_eq!(tags.title.as_deref(), Some("From v2"), "v2 wins the title");
        assert_eq!(tags.artist.as_deref(), Some("V1 Artist"));
        assert_eq!(tags.album.as_deref(), Some("V1 Album"));
        assert_eq!(tags.year.as_deref(), Some("1999"));
    }

    #[test]
    fn id3_is_total_on_garbage() {
        // Every one of these has been seen in the wild.
        assert_eq!(read_id3(&[]), TrackTags::default());
        assert_eq!(read_id3(b"ID3"), TrackTags::default());
        // A declared size far past EOF.
        let mut lying = b"ID3".to_vec();
        lying.extend_from_slice(&[3, 0, 0]);
        lying.extend_from_slice(&syncsafe(0x0FFF_FFFF));
        lying.extend_from_slice(&text_frame(3, "TIT2", 0, b"Truncated"));
        assert!(read_id3(&lying).title.is_some());
        // A frame whose size runs past the tag: keep what came before it.
        let mut frames = text_frame(3, "TIT2", 0, b"Good");
        frames.extend_from_slice(b"TPE1");
        frames.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
        frames.extend_from_slice(&[0, 0, 0]);
        let tags = read_id3(&id3v2(3, frames));
        assert_eq!(tags.title.as_deref(), Some("Good"));
        assert!(tags.artist.is_none());
        // Random bytes never panic and never invent a tag.
        let junk: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let _ = read_id3(&junk);
    }

    #[test]
    fn vorbis_comments_read_case_insensitively() {
        let mut packet = b"\x03vorbis".to_vec();
        let vendor = b"makepad-test";
        packet.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        packet.extend_from_slice(vendor);
        let pairs = ["TITLE=Ogg Song", "artist=Ogg Artist", "ALBUM=Ogg Album"];
        packet.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
        for pair in pairs {
            packet.extend_from_slice(&(pair.len() as u32).to_le_bytes());
            packet.extend_from_slice(pair.as_bytes());
        }
        let tags = read_vorbis_comments(&packet);
        assert_eq!(tags.title.as_deref(), Some("Ogg Song"));
        assert_eq!(tags.artist.as_deref(), Some("Ogg Artist"));
        assert_eq!(tags.album.as_deref(), Some("Ogg Album"));
        // A truncated comment header yields what was readable, not a panic.
        let cut = &packet[..packet.len() - 5];
        let _ = read_vorbis_comments(cut);
    }

    // ---- measurement ------------------------------------------------------

    #[test]
    fn mp3_duration_and_envelope_come_from_the_frames() {
        // 38.28 frames per second at 44.1 kHz / 1152 samples.
        let bytes = mp3_file(100, 200, None);
        let measured = measure_mp3(&bytes).expect("measured");
        assert_eq!(measured.millis, 100 * 1152 * 1000 / 44100);
        // Two granules × two channels per frame.
        assert_eq!(measured.envelope.len(), 400);
        assert!(measured.envelope.iter().all(|v| (*v - 1.0).abs() < 1e-6));
        // An ID3v2 tag in front must not shift the walk.
        let tagged = mp3_file(100, 200, Some(id3v2(3, text_frame(3, "TIT2", 0, b"x"))));
        assert_eq!(measure_mp3(&tagged).expect("measured").millis, measured.millis);
        // Non-audio is unmeasurable, not zero.
        assert!(measure_mp3(b"not an mp3 at all").is_none());
    }

    #[test]
    fn mp3_envelope_tracks_relative_gain() {
        let quiet = measure_mp3(&mp3_file(4, 120, None)).expect("quiet");
        let mut mixed = mp3_file(4, 120, None);
        mixed.extend(mp3_file(4, 200, None));
        let mixed = measure_mp3(&mixed).expect("mixed");
        // Normalised against its own peak: an all-one-gain file is flat 1.0,
        // and the quiet half of a mixed file is far below the loud half.
        assert!(quiet.envelope.iter().all(|v| (*v - 1.0).abs() < 1e-6));
        assert!(mixed.envelope[0] < 0.01, "{}", mixed.envelope[0]);
        assert!((mixed.envelope[mixed.envelope.len() - 1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn wav_measures_through_the_shared_parser() {
        let bytes = wav_file(44100, 44100);
        let measured = measure(&bytes, Container::Wav).expect("measured");
        assert_eq!(measured.millis, 1000);
        assert!(measure(b"RIFFnope", Container::Wav).is_none());
    }

    #[test]
    fn thumbnails_are_real_jpegs_and_differ_per_track() {
        let bytes = mp3_file(20, 190, None);
        let measured = measure_mp3(&bytes).expect("measured");
        let strip = track_thumbnail(&bytes, Container::Mp3, &measured, "ns/music/a/b");
        assert_eq!(strip.len(), THUMB_DIM * THUMB_DIM);
        let jpeg = encode_jpeg_bgra(&strip, THUMB_DIM, THUMB_DIM).expect("jpeg");
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
        // No envelope (an ogg we cannot decode) still gets a stable, distinct card.
        let flat = Measured::default();
        let a = track_thumbnail(b"", Container::Ogg, &flat, "ns/music/a/one");
        let b = track_thumbnail(b"", Container::Ogg, &flat, "ns/music/a/two");
        assert_ne!(a, b);
        assert_eq!(a, track_thumbnail(b"", Container::Ogg, &flat, "ns/music/a/one"));
    }

    // ---- scan, tags, aliases ---------------------------------------------

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn scan_finds_tracks_and_lists_unsupported() {
        let root = temp_root("scan");
        write(&root, "Adana Twins/Strange/01 Strange.mp3", &mp3_file(2, 180, None));
        write(&root, "Adana Twins/Strange/cover.jpg", b"not audio");
        write(&root, "Echo, Red Axes/Single/track.ogg", b"OggS");
        write(&root, "Lossless/song.flac", b"fLaC");
        write(&root, ".hidden/secret.mp3", &mp3_file(1, 180, None));
        let scan = scan_music(&root);
        let rels: Vec<&str> = scan.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(
            rels,
            [
                "Adana Twins/Strange/01 Strange.mp3",
                "Echo, Red Axes/Single/track.ogg"
            ],
            "{rels:?}"
        );
        assert_eq!(scan.skipped.len(), 1);
        assert_eq!(scan.skipped[0].rel, "Lossless/song.flac");
        assert!(scan.skipped[0].reason.contains("unsupported"));
        let first = &scan.files[0];
        assert_eq!(first.dirs, ["Adana Twins", "Strange"]);
        assert_eq!(first.stem, "01 Strange");
        assert_eq!(first.container, Container::Mp3);
        // A missing root is an empty scan, not a panic.
        assert_eq!(scan_music(&root.join("nope")), MusicScan::default());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tags_come_from_the_directory_names() {
        let dirs = vec!["Echo, Red Axes".to_string(), "Nofar".to_string()];
        let tags = music_tags(&dirs, "Echo & Red Axes", "Nofar");
        assert_eq!(tags[0], MUSIC_TAG);
        assert!(tags.contains(&"echo-red-axes".to_string()), "{tags:?}");
        assert!(tags.contains(&"nofar".to_string()));
        // Artist folded to the same slug as the directory: one tag, not two.
        assert_eq!(tags.iter().filter(|t| *t == "echo-red-axes").count(), 1);
        assert_eq!(tags.len(), 3, "{tags:?}");
        // Every tag is inside the catalog label charset.
        for tag in &tags {
            assert!(!tag.is_empty() && tag.len() <= MAX_TAG_BYTES, "{tag}");
            let first = tag.as_bytes()[0];
            assert!(first.is_ascii_lowercase() || first.is_ascii_digit(), "{tag}");
            assert!(tag
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_'));
        }
        // Names with nothing ASCII-able contribute no tag rather than a bad one.
        let cjk = music_tags(&["音楽".to_string()], "", "");
        assert_eq!(cjk, [MUSIC_TAG]);
        // The label budget is honoured even by a pathological tree.
        let many: Vec<String> = (0..60).map(|i| format!("dir{i}")).collect();
        assert_eq!(music_tags(&many, "", "").len(), MAX_TAGS);
    }

    #[test]
    fn aliases_are_readable_and_inside_the_contract() {
        let alias = music_alias("rik2", "Adana Twins", "Strange", None);
        assert_eq!(alias, "rik2/music/adana-twins/strange");
        assert!(AssetAlias::from_str(&alias).is_ok());
        // The `music` namespace does not repeat itself as the class segment.
        let own = music_alias("music", "Adana Twins", "Strange", None);
        assert_eq!(own, "music/adana-twins/strange");
        assert!(AssetAlias::from_str(&own).is_ok());
        // Hostile inputs still form a legal alias.
        for (artist, title) in [
            ("", ""),
            ("音楽", "音楽"),
            ("-leading-dash", "--"),
            (&"x".repeat(300), &"y".repeat(300)),
        ] {
            let alias = music_alias("rik2", artist, title, None);
            AssetAlias::from_str(&alias)
                .unwrap_or_else(|e| panic!("{alias} rejected: {e}"));
        }
        // A long namespace shortens the name, never the identity (the
        // contract caps one segment at 48 bytes, so 40 is as long as a
        // namespace legally gets near).
        let long_ns = "n".repeat(40);
        let alias = music_alias(&long_ns, &"a".repeat(80), &"b".repeat(80), Some("deadbeef"));
        assert!(alias.starts_with(&format!("{long_ns}/music/")));
        assert!(alias.ends_with("-deadbeef"));
        AssetAlias::from_str(&alias).expect("long namespace alias");
    }

    #[test]
    fn colliding_titles_get_a_path_digest_and_nothing_else_does() {
        let root = temp_root("collide");
        // The same song, same artist, on two albums.
        write(&root, "Artist/Album A/Song.mp3", &mp3_file(2, 180, None));
        write(&root, "Artist/Album B/Song.mp3", &mp3_file(3, 180, None));
        write(&root, "Artist/Album A/Other.mp3", &mp3_file(2, 180, None));
        let scan = scan_music(&root);
        let (tracks, _) = plan_tracks(&root, &scan, "rik2", &mut |_, _, _| {}, &|| false);
        let by_rel = |rel: &str| {
            tracks
                .iter()
                .find(|t| t.rel == rel)
                .unwrap_or_else(|| panic!("{rel}"))
                .alias
                .clone()
        };
        let a = by_rel("Artist/Album A/Song.mp3");
        let b = by_rel("Artist/Album B/Song.mp3");
        assert_ne!(a, b);
        assert!(a.starts_with("rik2/music/artist/song-"), "{a}");
        assert!(b.starts_with("rik2/music/artist/song-"), "{b}");
        // The uncontested name stays clean.
        assert_eq!(by_rel("Artist/Album A/Other.mp3"), "rik2/music/artist/other");
        // Stable across runs over the same tree.
        let (again, _) = plan_tracks(&root, &scan_music(&root), "rik2", &mut |_, _, _| {}, &|| false);
        assert_eq!(
            again.iter().map(|t| t.alias.clone()).collect::<Vec<_>>(),
            tracks.iter().map(|t| t.alias.clone()).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plan_prefers_container_metadata_over_the_path() {
        let root = temp_root("plan");
        let mut frames = text_frame(3, "TIT2", 0, b"Real Title");
        frames.extend(text_frame(3, "TPE1", 0, b"Real Artist"));
        frames.extend(text_frame(3, "TALB", 0, b"Real Album"));
        write(
            &root,
            "Folder Artist/Folder Album/99 file name.mp3",
            &mp3_file(2, 180, Some(id3v2(3, frames))),
        );
        write(&root, "Bare Artist/Bare Album/untagged.mp3", &mp3_file(2, 180, None));
        let (tracks, _) = plan_tracks(&root, &scan_music(&root), "rik2", &mut |_, _, _| {}, &|| false);
        let tagged = tracks.iter().find(|t| t.rel.contains("file name")).unwrap();
        assert_eq!(tagged.title, "Real Title");
        assert_eq!(tagged.artist, "Real Artist");
        assert_eq!(tagged.alias, "rik2/music/real-artist/real-title");
        // The directory tags are there REGARDLESS of what ID3 said.
        assert!(tagged.tags.contains(&"folder-artist".to_string()), "{:?}", tagged.tags);
        assert!(tagged.tags.contains(&"folder-album".to_string()));
        assert!(tagged.tags.contains(&"real-artist".to_string()));
        let bare = tracks.iter().find(|t| t.rel.contains("untagged")).unwrap();
        assert_eq!(bare.title, "untagged");
        assert_eq!(bare.artist, "Bare Artist");
        assert_eq!(bare.album, "Bare Album");
        assert_eq!(bare.alias, "rik2/music/bare-artist/untagged");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- end to end -------------------------------------------------------

    /// Real server round trip: publish a small tree, skip it unchanged,
    /// publish a new revision when bytes change, and answer the DJ's
    /// `kind=audio` catalog query.
    #[test]
    fn music_publishes_and_lists_by_kind_and_tag() {
        use makepad_asset_client::{ApiEndpoints, CatalogQuery, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};

        let base = temp_root("e2e");
        let server_root = base.join("server");
        std::fs::create_dir_all(&server_root).unwrap();
        let mut config = ServerConfig::new(server_root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.discovery = None;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(server_root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(base.join("cache"));
        client_config.token = Some(token);
        let endpoints = ApiEndpoints {
            control: server.control_addr(),
            data: server.data_addr(),
        };
        let mut client = AssetClient::connect(client_config, endpoints, Some(server.server_id()))
            .expect("connect");

        let music = base.join("music");
        let mut frames = text_frame(3, "TIT2", 0, b"Strange");
        frames.extend(text_frame(3, "TPE1", 0, b"Adana Twins"));
        write(&music, "Adana Twins/Strange/01.mp3", &mp3_file(40, 190, Some(id3v2(3, frames))));
        write(&music, "Echo, Red Axes/Nofar/02.mp3", &mp3_file(30, 170, None));
        write(&music, "Lossless/keeper.flac", b"fLaC not decoded");
        let rights = personal_library_rights(&music);
        let mut noop = |_: MusicProgress| {};
        let never = || false;

        let first = import_music(&mut client, &music, "rik2", &rights, false, &mut noop, &never)
            .expect("import");
        assert!(first.failed.is_empty(), "{:?}", first.failed);
        assert_eq!(first.published.len(), 2, "{:?}", first.published);
        assert_eq!(first.skipped.len(), 1);
        assert!(first.skipped[0].0.ends_with("keeper.flac"));
        assert!(!first.cancelled);

        let second = import_music(&mut client, &music, "rik2", &rights, false, &mut noop, &never)
            .expect("import");
        assert_eq!(second.unchanged.len(), 2, "{:?}", second);
        assert!(second.published.is_empty() && second.updated.is_empty());

        // Changed bytes: a NEW REVISION of the same asset, never a second row.
        write(&music, "Echo, Red Axes/Nofar/02.mp3", &mp3_file(31, 170, None));
        let third = import_music(&mut client, &music, "rik2", &rights, false, &mut noop, &never)
            .expect("import");
        assert_eq!(third.updated.len(), 1, "{third:?}");
        assert_eq!(third.unchanged.len(), 1);

        let alias = AssetAlias::from_str("rik2/music/adana-twins/strange").expect("alias");
        let head = client.resolve_alias(&alias).expect("alias head");
        let manifest = client
            .fetch_asset_manifest(&head.head_revision)
            .expect("manifest");
        assert_eq!(manifest.kind, AssetKind::Audio);
        assert!(manifest.metrics.media_millis > 0, "duration is measured, never zero");
        let audio = manifest
            .files
            .iter()
            .find(|f| f.role == FileRole::Audio)
            .expect("audio role");
        assert_eq!(audio.media, MediaType::Mp3, "the original mp3 IS the product");

        // The DJ's lane: kind=audio, plus the directory tag the user asked for.
        let by_kind = |tag: Option<&str>| {
            client
                .catalog_search(
                    &CatalogQuery {
                        text: String::new(),
                        namespace: None,
                        kind: Some(AssetKind::Audio),
                        category: None,
                        tag: tag.map(str::to_string),
                        exclude_tag: None,
                        creator: None,
                        live_only: true,
                        page_size: 50,
                        facets: 0,
                    },
                    None,
                )
                .expect("search")
        };
        let all = by_kind(Some(MUSIC_TAG));
        let mut titles: Vec<&str> = all.hits.iter().map(|h| h.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, ["02", "Strange"], "{titles:?}");
        assert_eq!(
            all.hits.len(),
            2,
            "a changed track must not mint a second row"
        );
        let by_dir = by_kind(Some("echo-red-axes"));
        assert_eq!(by_dir.hits.len(), 1, "directory names are searchable tags");
        assert_eq!(by_dir.hits[0].title, "02");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_stops_between_tracks_and_keeps_what_landed() {
        use makepad_asset_client::{ApiEndpoints, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};
        use std::sync::atomic::{AtomicBool, Ordering};

        let base = temp_root("cancel");
        let server_root = base.join("server");
        std::fs::create_dir_all(&server_root).unwrap();
        let mut config = ServerConfig::new(server_root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.discovery = None;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(server_root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(base.join("cache"));
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

        let music = base.join("music");
        for i in 0..4 {
            write(&music, &format!("A/B/{i}.mp3"), &mp3_file(5 + i, 180, None));
        }
        // Cancel once two tracks have actually gone up — the reading pass
        // polls `cancel` too, so a plain call counter would stop the run
        // before anything was published at all.
        let stop = AtomicBool::new(false);
        let mut watch = |p: MusicProgress| {
            // `done` is reported BEFORE that track goes up, so arming at
            // 1 stops the run after exactly two publications.
            if p.stage == MusicStage::Publishing && p.done >= 1 {
                stop.store(true, Ordering::SeqCst);
            }
        };
        let stop_after_two = || stop.load(Ordering::SeqCst);
        let report = import_music(
            &mut client,
            &music,
            "rik2",
            &personal_library_rights(&music),
            false,
            &mut watch,
            &stop_after_two,
        )
        .expect("import");
        assert!(report.cancelled);
        assert_eq!(report.published.len(), 2, "{report:?}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
