//! Shared classic-import types, PNG, slugs, and binary helpers.

use crate::ao_bake::BakeStats;
use crate::pack_import::{PackCompileReport, PackImportError, PackSourceSpec};
use makepad_asset_data::{sha256, AssetKind};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Rights / identity (explicit — never CC0, never Kenney, never retail id art)
// ---------------------------------------------------------------------------

pub const FREEDOOM_SOURCE_ID: &str = "freedoom";
pub const FREEDOOM_SOURCE_TITLE: &str = "Freedoom";
pub const FREEDOOM_LICENSE: &str = "BSD-3-Clause";
pub const FREEDOOM_TERMS_URL: &str = "https://github.com/freedoom/freedoom/blob/master/COPYING.adoc";
pub const FREEDOOM_CREDITS: &str = "Freedoom contributors (freedoom.github.io)";
pub const FREEDOOM_HOME: &str = "https://freedoom.github.io/";
pub const FREEDOOM_GITHUB: &str = "https://github.com/freedoom/freedoom";
/// Pinned test terms text for deterministic terms_digest (not a downloaded PDF).
pub const FREEDOOM_TERMS_TEXT: &[u8] =
    b"Freedoom BSD-3-Clause legal text for classic_import tests";

pub const LIBREQUAKE_SOURCE_ID: &str = "librequake";
pub const LIBREQUAKE_SOURCE_TITLE: &str = "LibreQuake";
pub const LIBREQUAKE_LICENSE: &str = "BSD-3-Clause-Modified";
pub const LIBREQUAKE_TERMS_URL: &str =
    "https://github.com/MissLav/LibreQuake/blob/master/LICENSE";
pub const LIBREQUAKE_CREDITS: &str = "LibreQuake contributors";
pub const LIBREQUAKE_HOME: &str = "https://github.com/MissLav/LibreQuake";
pub const LIBREQUAKE_GITHUB: &str = "https://github.com/MissLav/LibreQuake";
pub const LIBREQUAKE_TERMS_TEXT: &[u8] =
    b"LibreQuake Modified BSD legal text for classic_import tests";

pub const DUKE3D_SOURCE_ID: &str = "duke3d";
pub const DUKE3D_SOURCE_TITLE: &str = "Duke Nukem 3D (shareware)";
pub const DUKE3D_LICENSE: &str = "3D-Realms-shareware";
pub const DUKE3D_TERMS_URL: &str = "https://wiki.eduke32.com/wiki/Frequently_Asked_Questions";
pub const DUKE3D_CREDITS: &str = "3D Realms shareware + Duke4 HRP (hrp.duke4.net)";
pub const DUKE3D_HOME: &str = "https://www.eduke32.com/";
pub const DUKE3D_GITHUB: &str = "https://voidpoint.io/terminx/eduke32";
pub const DUKE3D_TERMS_TEXT: &[u8] =
    b"Duke3D shareware GRP is 3D Realms shareware; HRP art uses the HRP art license";

pub const QUAKE2_SOURCE_ID: &str = "quake2";
pub const QUAKE2_SOURCE_TITLE: &str = "Quake II (shareware)";
pub const QUAKE2_LICENSE: &str = "id-Software-shareware";
pub const QUAKE2_TERMS_URL: &str = "https://www.idsoftware.com/";
pub const QUAKE2_CREDITS: &str = "id Software Quake II demo / shareware";
pub const QUAKE2_HOME: &str = "https://www.idsoftware.com/";
pub const QUAKE2_GITHUB: &str = "https://github.com/id-Software/Quake-2";
pub const QUAKE2_TERMS_TEXT: &[u8] =
    b"Quake II demo PAK is id Software shareware; not a libre replacement set";

pub const QUAKE3_SOURCE_ID: &str = "quake3";
pub const QUAKE3_SOURCE_TITLE: &str = "Quake III Arena (demo)";
pub const QUAKE3_LICENSE: &str = "id-Software-demo";
pub const QUAKE3_TERMS_URL: &str = "https://ioquake3.org/help/players-guide/";
pub const QUAKE3_CREDITS: &str = "id Software Quake III Arena demo";
pub const QUAKE3_HOME: &str = "https://ioquake3.org/";
pub const QUAKE3_GITHUB: &str = "https://github.com/ioquake/ioq3";
pub const QUAKE3_TERMS_TEXT: &[u8] =
    b"Quake III Arena demo PK3 is id Software demo data; not OpenArena";

pub const DARKMOD_SOURCE_ID: &str = "darkmod";
pub const DARKMOD_SOURCE_TITLE: &str = "The Dark Mod";
pub const DARKMOD_LICENSE: &str = "CC-BY-NC-SA-3.0";
pub const DARKMOD_TERMS_URL: &str = "https://creativecommons.org/licenses/by-nc-sa/3.0/";
pub const DARKMOD_CREDITS: &str = "The Dark Mod team (thedarkmod.com)";
pub const DARKMOD_HOME: &str = "https://www.thedarkmod.com/";
pub const DARKMOD_GITHUB: &str = "https://www.thedarkmod.com/";
pub const DARKMOD_TERMS_TEXT: &[u8] =
    b"The Dark Mod assets are CC BY-NC-SA 3.0 (non-commercial, share-alike). Not CC0, not BSD. Fan missions remain property of their authors and are not imported as TDM core.";

pub const DOOM_SOURCE_ID: &str = "doom";
pub const DOOM_SOURCE_TITLE: &str = "Doom (shareware)";
pub const DOOM_LICENSE: &str = "id-Software-shareware";
pub const DOOM_TERMS_URL: &str = "https://doomwiki.org/wiki/DOOM1.WAD";
pub const DOOM_CREDITS: &str = "id Software Doom shareware (Knee-Deep in the Dead)";
pub const DOOM_HOME: &str = "https://www.idsoftware.com/";
pub const DOOM_GITHUB: &str = "https://github.com/id-Software/DOOM";
pub const DOOM_TERMS_TEXT: &[u8] =
    b"DOOM1.WAD is id Software shareware (episode 1). Not Freedoom. Not retail Doom/Doom II.";

pub const QUAKE_SOURCE_ID: &str = "quake";
pub const QUAKE_SOURCE_TITLE: &str = "Quake (shareware)";
pub const QUAKE_LICENSE: &str = "id-Software-shareware";
pub const QUAKE_TERMS_URL: &str = "https://quakewiki.org/wiki/Getting_Started";
pub const QUAKE_CREDITS: &str = "id Software Quake shareware (id1/pak0.pak)";
pub const QUAKE_HOME: &str = "https://www.idsoftware.com/";
pub const QUAKE_GITHUB: &str = "https://github.com/id-Software/Quake";
pub const QUAKE_TERMS_TEXT: &[u8] =
    b"Quake shareware pak0.pak is id Software shareware. Not LibreQuake. Not retail pak1.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassicSource {
    Freedoom,
    LibreQuake,
    Doom,
    Quake,
    Duke3d,
    Quake2,
    Quake3,
    DarkMod,
}

impl ClassicSource {
    pub fn id(self) -> &'static str {
        match self {
            Self::Freedoom => FREEDOOM_SOURCE_ID,
            Self::LibreQuake => LIBREQUAKE_SOURCE_ID,
            Self::Doom => DOOM_SOURCE_ID,
            Self::Quake => QUAKE_SOURCE_ID,
            Self::Duke3d => DUKE3D_SOURCE_ID,
            Self::Quake2 => QUAKE2_SOURCE_ID,
            Self::Quake3 => QUAKE3_SOURCE_ID,
            Self::DarkMod => DARKMOD_SOURCE_ID,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Freedoom => FREEDOOM_SOURCE_TITLE,
            Self::LibreQuake => LIBREQUAKE_SOURCE_TITLE,
            Self::Doom => DOOM_SOURCE_TITLE,
            Self::Quake => QUAKE_SOURCE_TITLE,
            Self::Duke3d => DUKE3D_SOURCE_TITLE,
            Self::Quake2 => QUAKE2_SOURCE_TITLE,
            Self::Quake3 => QUAKE3_SOURCE_TITLE,
            Self::DarkMod => DARKMOD_SOURCE_TITLE,
        }
    }

    pub fn license(self) -> &'static str {
        match self {
            Self::Freedoom => FREEDOOM_LICENSE,
            Self::LibreQuake => LIBREQUAKE_LICENSE,
            Self::Doom => DOOM_LICENSE,
            Self::Quake => QUAKE_LICENSE,
            Self::Duke3d => DUKE3D_LICENSE,
            Self::Quake2 => QUAKE2_LICENSE,
            Self::Quake3 => QUAKE3_LICENSE,
            Self::DarkMod => DARKMOD_LICENSE,
        }
    }

    pub fn terms_url(self) -> &'static str {
        match self {
            Self::Freedoom => FREEDOOM_TERMS_URL,
            Self::LibreQuake => LIBREQUAKE_TERMS_URL,
            Self::Doom => DOOM_TERMS_URL,
            Self::Quake => QUAKE_TERMS_URL,
            Self::Duke3d => DUKE3D_TERMS_URL,
            Self::Quake2 => QUAKE2_TERMS_URL,
            Self::Quake3 => QUAKE3_TERMS_URL,
            Self::DarkMod => DARKMOD_TERMS_URL,
        }
    }

    pub fn credits(self) -> &'static str {
        match self {
            Self::Freedoom => FREEDOOM_CREDITS,
            Self::LibreQuake => LIBREQUAKE_CREDITS,
            Self::Doom => DOOM_CREDITS,
            Self::Quake => QUAKE_CREDITS,
            Self::Duke3d => DUKE3D_CREDITS,
            Self::Quake2 => QUAKE2_CREDITS,
            Self::Quake3 => QUAKE3_CREDITS,
            Self::DarkMod => DARKMOD_CREDITS,
        }
    }

    pub fn home(self) -> &'static str {
        match self {
            Self::Freedoom => FREEDOOM_HOME,
            Self::LibreQuake => LIBREQUAKE_HOME,
            Self::Doom => DOOM_HOME,
            Self::Quake => QUAKE_HOME,
            Self::Duke3d => DUKE3D_HOME,
            Self::Quake2 => QUAKE2_HOME,
            Self::Quake3 => QUAKE3_HOME,
            Self::DarkMod => DARKMOD_HOME,
        }
    }

    pub fn github(self) -> &'static str {
        match self {
            Self::Freedoom => FREEDOOM_GITHUB,
            Self::LibreQuake => LIBREQUAKE_GITHUB,
            Self::Doom => DOOM_GITHUB,
            Self::Quake => QUAKE_GITHUB,
            Self::Duke3d => DUKE3D_GITHUB,
            Self::Quake2 => QUAKE2_GITHUB,
            Self::Quake3 => QUAKE3_GITHUB,
            Self::DarkMod => DARKMOD_GITHUB,
        }
    }

    pub fn terms_text(self) -> &'static [u8] {
        match self {
            Self::Freedoom => FREEDOOM_TERMS_TEXT,
            Self::LibreQuake => LIBREQUAKE_TERMS_TEXT,
            Self::Doom => DOOM_TERMS_TEXT,
            Self::Quake => QUAKE_TERMS_TEXT,
            Self::Duke3d => DUKE3D_TERMS_TEXT,
            Self::Quake2 => QUAKE2_TERMS_TEXT,
            Self::Quake3 => QUAKE3_TERMS_TEXT,
            Self::DarkMod => DARKMOD_TERMS_TEXT,
        }
    }

    pub fn terms_digest_hex(self) -> String {
        hex32(&sha256(self.terms_text()))
    }

    /// Local-folder override (`DUKE3D_PACK_ROOT`, …). Candidate dirs still
    /// include `local/packs/{id}` when the env var is unset.
    pub fn pack_root_env(self) -> &'static str {
        match self {
            Self::Freedoom => "FREEDOOM_PACK_ROOT",
            Self::LibreQuake => "LIBREQUAKE_PACK_ROOT",
            Self::Doom => "DOOM_PACK_ROOT",
            Self::Quake => "QUAKE_PACK_ROOT",
            Self::Duke3d => "DUKE3D_PACK_ROOT",
            Self::Quake2 => "QUAKE2_PACK_ROOT",
            Self::Quake3 => "QUAKE3_PACK_ROOT",
            Self::DarkMod => "DARKMOD_PACK_ROOT",
        }
    }

    /// Source/rights for [`pack_import::compile_pack`]. Attribution-required.
    pub fn pack_spec(self, pack_name: &str) -> PackSourceSpec {
        let name = if pack_name.trim().is_empty() {
            self.id().to_string()
        } else {
            sanitize_slug(pack_name)
        };
        PackSourceSpec {
            source_id: Some(self.id().into()),
            source_title: Some(self.title().into()),
            pack_name: Some(name),
            pack_version: Some("1.0".into()),
            license: Some(self.license().into()),
            license_revision: None,
            terms_digest: Some(self.terms_digest_hex()),
            terms_url: Some(self.terms_url().into()),
            credits: Some(self.credits().into()),
            source: Some(self.home().into()),
            source_archive: None,
            redistribution: Some(match self {
                Self::Freedoom | Self::LibreQuake => "attribution-required",
                Self::Doom | Self::Quake | Self::Duke3d | Self::Quake2 | Self::Quake3 => {
                    "user-owned-local"
                }
                Self::DarkMod => "non-commercial-sharealike",
            }.into()),
            derivatives: Some(match self {
                Self::Freedoom | Self::LibreQuake => "allowed",
                Self::Doom | Self::Quake | Self::Duke3d | Self::Quake2 | Self::Quake3 => {
                    "local-preview-only"
                }
                Self::DarkMod => "share-alike",
            }.into()),
        }
    }
}

pub(crate) fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub(crate) fn sanitize_slug(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "pack".into()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Public convert / compile API
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassicAsset {
    pub key: String,
    pub kind: AssetKind,
    /// Relative path under the staged library root.
    pub rel_path: String,
    pub tags: Vec<String>,
    /// Optional 1024-wide 128²-tile animation / still icon (PNG), relative
    /// to the staged root. Used as the library thumbnail.
    pub icon_rel: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClassicConvertReport {
    pub source: ClassicSource,
    pub assets: Vec<ClassicAsset>,
    pub staged_dir: PathBuf,
    pub bake: BakeStats,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ClassicCompileReport {
    pub convert: ClassicConvertReport,
    pub pack: Option<PackCompileReport>,
}

#[derive(Debug)]
pub struct ClassicImportError {
    message: String,
}

impl ClassicImportError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ClassicImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ClassicImportError {}

impl From<PackImportError> for ClassicImportError {
    fn from(e: PackImportError) -> Self {
        Self::new(e.to_string())
    }
}

/// Fail-closed when `pack_dir` is missing or not a directory.
pub fn require_pack_dir(pack_dir: &Path) -> Result<(), ClassicImportError> {
    let meta = std::fs::symlink_metadata(pack_dir).map_err(|_| {
        ClassicImportError::new(format!(
            "pack is not on disk ({}) — import is local-folder only",
            pack_dir.display()
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(ClassicImportError::new(format!(
            "pack path is a symlink ({}) — refuse",
            pack_dir.display()
        )));
    }
    if !meta.is_dir() {
        return Err(ClassicImportError::new(format!(
            "pack is not a directory ({})",
            pack_dir.display()
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvertStage {
    /// Walking the pack and unzipping PK4/PK3/PAK/GRP. Large id Tech 4
    /// trees spend most of their wall time here; the UI must tick.
    Expand,
    Convert,
    Ao,
}

#[derive(Clone, Debug)]
pub struct ConvertTick {
    pub stage: ConvertStage,
    pub done: usize,
    pub total: usize,
    pub current: String,
    /// Spawn-view or sprite still for the queue strip. None on count-only ticks.
    pub preview_png: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Kind / path helpers
// ---------------------------------------------------------------------------

/// Intended catalog kind for a converted relative path (also used by
/// pack_import path overrides for classic staging layout).
pub fn kind_for_staged_path(rel: &str) -> AssetKind {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    if lower.contains("/worlds/") || lower.starts_with("worlds/") || lower.contains("/world/") {
        return AssetKind::World;
    }
    if lower.contains("/characters/") || lower.starts_with("characters/") {
        return AssetKind::Character;
    }
    if lower.contains("/billboards/")
        || lower.starts_with("billboards/")
        || lower.contains("/sprites/")
        || lower.contains("billboard")
    {
        return AssetKind::Billboard;
    }
    if lower.contains("/weapons/") || lower.starts_with("weapons/") || lower.contains("weapon") {
        return AssetKind::Weapon;
    }
    if lower.contains("/props/") || lower.starts_with("props/") {
        return AssetKind::Prop;
    }
    if lower.ends_with(".wav") {
        return AssetKind::Audio;
    }
    if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        return AssetKind::Texture;
    }
    AssetKind::Mesh
}

pub(crate) fn tags_for(kind: AssetKind, extra: &[&str]) -> Vec<String> {
    let mut tags = vec![kind_tag(kind).to_string()];
    for e in extra {
        let s = sanitize_slug(e);
        if !s.is_empty() && !tags.iter().any(|t| t == &s) {
            tags.push(s);
        }
    }
    tags
}

pub(crate) fn kind_tag(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Mesh => "mesh",
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Vehicle => "vehicle",
        AssetKind::Prop => "prop",
        AssetKind::Texture => "texture",
        AssetKind::Material => "material",
        AssetKind::Audio => "audio",
        AssetKind::Video => "video",
        AssetKind::Skybox => "skybox",
        AssetKind::World => "world",
        AssetKind::Prefab => "prefab",
        AssetKind::Billboard => "billboard",
    }
}

pub(crate) fn stem_slug(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    sanitize_slug(stem)
}

// ---------------------------------------------------------------------------
// Color key → alpha
// ---------------------------------------------------------------------------

/// Doom patch/sprite: palette index 0 is transparent.
pub fn indexed_to_rgba(indices: &[u8], palette: &[[u8; 3]; 256], transparent_index: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(indices.len() * 4);
    for &i in indices {
        if i == transparent_index {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let c = palette[i as usize];
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    out
}

/// Magenta #FF00FF and cyan #00FFFF → A=0 (Quake / common sprite keys).
pub fn colorkey_rgb_to_rgba(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    assert_eq!(rgb.len(), w * h * 3);
    let mut out = Vec::with_capacity(w * h * 4);
    for px in rgb.chunks_exact(3) {
        let (r, g, b) = (px[0], px[1], px[2]);
        let a = if (r == 255 && g == 0 && b == 255) || (r == 0 && g == 255 && b == 255) {
            0
        } else {
            255
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

// ---------------------------------------------------------------------------
// PNG encode (RGBA / RGB stored-deflate)
// ---------------------------------------------------------------------------

pub fn encode_png_rgba(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    encode_png(rgba, w, h, 6, 4)
}

/// Decode a PNG produced by [`encode_png_rgba`] (stored zlib, 8-bit RGBA/RGB).
pub fn decode_png_stored(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if bytes.len() < 33 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("not a png".into());
    }
    let mut off = 8usize;
    let mut w = 0u32;
    let mut h = 0u32;
    let mut color = 0u8;
    let mut zlib = Vec::new();
    while off + 12 <= bytes.len() {
        let n = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let typ = &bytes[off + 4..off + 8];
        if off + 12 + n > bytes.len() {
            break;
        }
        let data = &bytes[off + 8..off + 8 + n];
        if typ == b"IHDR" && data.len() >= 13 {
            w = u32::from_be_bytes(data[0..4].try_into().unwrap());
            h = u32::from_be_bytes(data[4..8].try_into().unwrap());
            color = data[9];
        } else if typ == b"IDAT" {
            zlib.extend_from_slice(data);
        } else if typ == b"IEND" {
            break;
        }
        off += 12 + n;
    }
    if w == 0 || h == 0 || zlib.len() < 8 {
        return Err("png missing ihdr/idat".into());
    }
    if w > 16384 || h > 16384 {
        return Err("png dimensions out of range".into());
    }
    let bpp = match color {
        2 => 3,
        6 => 4,
        _ => return Err("png color type".into()),
    };
    // stored zlib: 78 01 + blocks
    let mut raw = Vec::new();
    let mut z = 2usize;
    loop {
        if z + 5 > zlib.len() {
            return Err("png zlib truncated".into());
        }
        let last = zlib[z] & 1 != 0;
        let n = u16::from_le_bytes([zlib[z + 1], zlib[z + 2]]) as usize;
        z += 5;
        if z + n > zlib.len() {
            return Err("png zlib block".into());
        }
        raw.extend_from_slice(&zlib[z..z + n]);
        z += n;
        if last {
            break;
        }
    }
    let stride = w as usize * bpp;
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let row = 1 + y * (stride + 1);
        if row + stride > raw.len() {
            return Err("png row truncated".into());
        }
        for x in 0..w as usize {
            let p = row + x * bpp;
            if bpp == 4 {
                rgba.extend_from_slice(&raw[p..p + 4]);
            } else {
                rgba.extend_from_slice(&[raw[p], raw[p + 1], raw[p + 2], 255]);
            }
        }
    }
    Ok((rgba, w, h))
}

pub(crate) fn encode_png_rgb(rgb: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    encode_png(rgb, w, h, 2, 3)
}

pub(crate) fn encode_png(pixels: &[u8], w: u32, h: u32, color_type: u8, bpp: usize) -> Result<Vec<u8>, String> {
    let expected = w as usize * h as usize * bpp;
    if pixels.len() != expected {
        return Err(format!(
            "png pixel length {} != {expected}",
            pixels.len()
        ));
    }
    let stride = w as usize * bpp;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        raw.extend_from_slice(&pixels[y * stride..y * stride + stride]);
    }
    let mut zlib = vec![0x78, 0x01];
    let mut off = 0usize;
    while off < raw.len() {
        let take = (raw.len() - off).min(65535);
        let last = off + take == raw.len();
        zlib.push(if last { 0x01 } else { 0x00 });
        let n = take as u16;
        zlib.extend_from_slice(&n.to_le_bytes());
        zlib.extend_from_slice(&(!n).to_le_bytes());
        zlib.extend_from_slice(&raw[off..off + take]);
        off += take;
    }
    let mut s1 = 1u32;
    let mut s2 = 0u32;
    for &b in &raw {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    push_png_chunk(&mut out, b"IHDR", &ihdr);
    push_png_chunk(&mut out, b"IDAT", &zlib);
    push_png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

pub(crate) fn push_png_chunk(out: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(data);
    let mut crc_src = Vec::with_capacity(4 + data.len());
    crc_src.extend_from_slice(typ);
    crc_src.extend_from_slice(data);
    out.extend_from_slice(&png_crc(&crc_src).to_be_bytes());
}

pub(crate) fn png_crc(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[allow(dead_code)]
pub(crate) fn placeholder_png_512() -> Vec<u8> {
    let w = 512u32;
    let h = 512u32;
    let rgb = vec![0x20u8; (w * h * 3) as usize];
    encode_png_rgb(&rgb, w, h).expect("placeholder png")
}

#[allow(dead_code)]
pub(crate) fn write_glb_placeholder_thumbs(root: &Path) -> Result<(), ClassicImportError> {
    let mut glbs = Vec::new();
    let mut images = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                if let Some(stem) = path.file_stem() {
                    images.insert(stem.to_os_string());
                }
            }
            if ext == "glb" {
                glbs.push(path);
            }
        }
    }
    let placeholder = placeholder_png_512();
    for glb in glbs {
        let stem = glb.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
        if images.contains(&stem) {
            continue;
        }
        let thumb = glb.with_extension("png");
        std::fs::write(&thumb, &placeholder).map_err(|e| {
            ClassicImportError::new(format!("write thumb {}: {e}", thumb.display()))
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Binary helpers
// ---------------------------------------------------------------------------

pub(crate) fn u16_le(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        return 0;
    }
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn i16_le(b: &[u8], o: usize) -> i16 {
    u16_le(b, o) as i16
}

pub(crate) fn u32_le(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub(crate) fn i32_le(b: &[u8], o: usize) -> i32 {
    u32_le(b, o) as i32
}

pub(crate) fn f32_le(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_le(b, o))
}

pub(crate) fn lump_name(raw: &[u8]) -> String {
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end])
        .trim()
        .to_ascii_uppercase()
}

/// Doom sky flats are never drawn as geometry — the engine shows the sky
/// through that opening. `F_SKY1` is the IWAD name; `starts_with("SKY")`
/// alone misses it and used to emit a default-textured ceiling (the blue
/// slivers in MAP27's open wood ceiling).
pub(crate) fn is_sky_flat(name: &str) -> bool {
    let n = name.trim();
    n.starts_with("F_SKY") || n.starts_with("SKY")
}

/// Sidedef `-` means "do not draw this strip". Substituting `_default`
/// painted opaque/translucent slabs over sky holes and steps.
pub(crate) fn is_blank_tex(name: &str) -> bool {
    name.is_empty() || name == "-"
}

pub(crate) fn opaque_gray_tile() -> RgbaImage {
    let mut rgba = vec![0x80u8; 64 * 64 * 4];
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    RgbaImage {
        w: 64,
        h: 64,
        rgba,
    }
}

pub(crate) fn default_vga_palette() -> [[u8; 3]; 256] {
    let mut pal = [[0u8; 3]; 256];
    for i in 0..256 {
        let v = i as u8;
        pal[i] = [v, v, v];
    }
    // index 0 reserved transparent for Doom
    pal[0] = [0, 0, 0];
    pal
}


/// Engine-space first-person spawn for a World thumbnail (metres, radians).
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorldSpawn {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

pub(crate) fn write_spawn_sidecar(glb: &Path, spawn: WorldSpawn) {
    let text = format!(
        "world-spawn 1\n{:.4} {:.4} {:.4}\n{:.5} {:.5}\n",
        spawn.pos[0], spawn.pos[1], spawn.pos[2], spawn.yaw, spawn.pitch
    );
    let _ = std::fs::write(glb.with_extension("spawn"), text);
}

#[derive(Clone)]
pub(crate) struct RgbaImage {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}
