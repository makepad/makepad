//! Packing a game directory into a `.arcade` archive, and — the part that
//! matters — unpacking one that arrived from a stranger.
//!
//! Extraction is a security boundary (game.md: "Untrusted games are untrusted
//! code"). Everything here is written on the assumption that the archive was
//! built to hurt us: names are validated before they are ever joined to a path,
//! sizes are checked against caps taken from the *declared* headers before a
//! single byte is decompressed, and the destination is re-checked after the
//! join so a name that slipped the first test still cannot escape.

use crate::manifest::{Manifest, ManifestError, MAX_MANIFEST_BYTES};
use makepad_zip_file::{zip_read_central_directory, ZipMethod, ZipWriter};
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

pub const GAME_FILE: &str = "game.splash";
pub const MANIFEST_FILE: &str = "manifest.toml";
pub const ASSETS_DIR: &str = "assets";
pub const PACKAGE_EXT: &str = "arcade";

/// Caps. Deliberately modest: a kid's game is kilobytes of script plus a few
/// assets, and anything claiming more is either broken or hostile.
pub const MAX_ENTRIES: usize = 2048;
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_NAME_LEN: usize = 512;
pub const MAX_GAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum PkgError {
    Io(String),
    Zip(String),
    Manifest(ManifestError),
    /// The archive is structurally fine but not a game.
    MissingMember(&'static str),
    /// Refused by a hardening rule; the string names which.
    Rejected(String),
}

impl std::fmt::Display for PkgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkgError::Io(e) => write!(f, "io error: {e}"),
            PkgError::Zip(e) => write!(f, "bad archive: {e}"),
            PkgError::Manifest(e) => write!(f, "{e}"),
            PkgError::MissingMember(m) => write!(f, "archive has no {m}"),
            PkgError::Rejected(why) => write!(f, "refused: {why}"),
        }
    }
}

fn io<E: std::fmt::Display>(e: E) -> PkgError {
    PkgError::Io(e.to_string())
}

fn rejected(why: impl Into<String>) -> PkgError {
    PkgError::Rejected(why.into())
}

/// What a package contains, without touching the filesystem.
#[derive(Debug)]
pub struct Package {
    pub manifest: Manifest,
    pub game: String,
    /// Relative path -> bytes, for everything under `assets/`.
    pub assets: Vec<(String, Vec<u8>)>,
}

/// The single gate every archive member name passes before it becomes a path.
///
/// Rejects: absolute paths (unix and windows), drive letters, UNC, backslashes
/// (a windows separator that unix `Path` would treat as an ordinary character),
/// `..` and `.` components, empty components, NULs, control characters, and
/// anything overlong. Directory entries are handled by the caller.
fn safe_relative_path(name: &str) -> Result<PathBuf, PkgError> {
    if name.is_empty() {
        return Err(rejected("empty member name"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(rejected(format!("member name too long: {} bytes", name.len())));
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err(rejected(format!("absolute member name: {name:?}")));
    }
    if name.contains('\\') {
        return Err(rejected(format!("backslash in member name: {name:?}")));
    }
    if name.contains('\0') {
        return Err(rejected("NUL in member name"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(rejected("control character in member name"));
    }
    // "C:foo" and "C:/foo" both reach the filesystem as absolute on windows.
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return Err(rejected(format!("drive-qualified member name: {name:?}")));
    }

    let mut out = PathBuf::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." {
            return Err(rejected(format!("empty or '.' path component in {name:?}")));
        }
        if part == ".." {
            return Err(rejected(format!("path traversal in {name:?}")));
        }
        out.push(part);
    }
    // Belt and braces: whatever we built must still be purely Normal parts.
    if out
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(rejected(format!("non-normal path component in {name:?}")));
    }
    Ok(out)
}

/// Read a package from archive bytes. Does no IO — hostile archives are
/// rejected before anything touches the disk.
pub fn read_package(bytes: &[u8]) -> Result<Package, PkgError> {
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(rejected(format!("archive too large: {} bytes", bytes.len())));
    }
    if bytes.len() < 22 {
        return Err(PkgError::Zip("shorter than an empty archive".into()));
    }

    let mut cursor = Cursor::new(bytes);
    let dir = zip_read_central_directory(&mut cursor)
        .map_err(|e| PkgError::Zip(format!("{e:?}")))?;

    if dir.file_headers.len() > MAX_ENTRIES {
        return Err(rejected(format!(
            "too many entries: {}",
            dir.file_headers.len()
        )));
    }

    // Cap on DECLARED sizes, before decompressing anything: this is what stops a
    // zip bomb, since the bomb's whole trick is a small archive declaring (and
    // producing) an enormous expansion.
    let mut declared_total: u64 = 0;
    for h in &dir.file_headers {
        let size = h.uncompressed_size as u64;
        if size > MAX_ENTRY_BYTES {
            return Err(rejected(format!(
                "member {:?} declares {size} bytes",
                h.file_name
            )));
        }
        declared_total = declared_total.saturating_add(size);
        if declared_total > MAX_TOTAL_BYTES {
            return Err(rejected("archive declares more than the total size cap"));
        }
    }

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut game: Option<String> = None;
    let mut assets: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut actual_total: u64 = 0;

    for h in &dir.file_headers {
        let name = h.file_name.as_str();
        // Directory entries carry no payload; skip them rather than creating
        // anything, so an archive cannot pre-create a path we did not choose.
        if name.ends_with('/') {
            continue;
        }
        let rel = safe_relative_path(name)?;

        if seen.iter().any(|s| s == name) {
            return Err(rejected(format!("duplicate member {name:?}")));
        }
        seen.push(name.to_string());

        // A symlink is stored with S_IFLNK in the high half of the external
        // attributes; its "contents" are the target path. We never create
        // symlinks, so treat any as hostile rather than silently writing the
        // target string into a regular file.
        let unix_mode = (h.external_file_attributes >> 16) as u16;
        if unix_mode & 0xF000 == 0xA000 {
            return Err(rejected(format!("symlink member {name:?}")));
        }

        let data = h
            .extract(&mut cursor)
            .map_err(|e| PkgError::Zip(format!("{name:?}: {e:?}")))?;

        // The header is a claim; this is the check. A member that decompresses
        // to more than it declared is malformed, and unbounded if trusted.
        if data.len() as u64 > h.uncompressed_size as u64 {
            return Err(rejected(format!(
                "member {name:?} expanded past its declared size"
            )));
        }
        actual_total = actual_total.saturating_add(data.len() as u64);
        if actual_total > MAX_TOTAL_BYTES {
            return Err(rejected("archive expanded past the total size cap"));
        }

        if rel == Path::new(MANIFEST_FILE) {
            if data.len() > MAX_MANIFEST_BYTES {
                return Err(PkgError::Manifest(ManifestError::TooLarge));
            }
            manifest_bytes = Some(data);
        } else if rel == Path::new(GAME_FILE) {
            if data.len() > MAX_GAME_BYTES {
                return Err(rejected("game.splash too large"));
            }
            game = Some(String::from_utf8(data).map_err(|_| rejected("game.splash is not utf-8"))?);
        } else if rel.starts_with(ASSETS_DIR) {
            assets.push((rel.to_string_lossy().replace('\\', "/"), data));
        }
        // Anything else is ignored: an archive may carry a README we neither
        // need nor want to write out.
    }

    let manifest = Manifest::parse(&manifest_bytes.ok_or(PkgError::MissingMember(MANIFEST_FILE))?)
        .map_err(PkgError::Manifest)?;
    let game = game.ok_or(PkgError::MissingMember(GAME_FILE))?;

    Ok(Package {
        manifest,
        game,
        assets,
    })
}

/// Write a validated package to a directory. `dest` is created if absent; the
/// caller owns the choice of location, and nothing lands outside it.
pub fn write_package(pkg: &Package, dest: &Path) -> Result<(), PkgError> {
    std::fs::create_dir_all(dest).map_err(io)?;
    let root = dest.canonicalize().map_err(io)?;

    let write_one = |rel: &str, data: &[u8]| -> Result<(), PkgError> {
        let rel_path = safe_relative_path(rel)?;
        let target = root.join(&rel_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
            // Re-check after the join: if a parent resolves outside the root
            // (a pre-existing symlink in the destination, say), refuse. The
            // name test alone cannot see that.
            let parent_real = parent.canonicalize().map_err(io)?;
            if !parent_real.starts_with(&root) {
                return Err(rejected(format!("{rel:?} resolves outside the destination")));
            }
        }
        std::fs::write(&target, data).map_err(io)
    };

    write_one(MANIFEST_FILE, pkg.manifest.to_toml().as_bytes())?;
    write_one(GAME_FILE, pkg.game.as_bytes())?;
    for (rel, data) in &pkg.assets {
        write_one(rel, data)?;
    }
    Ok(())
}

/// Read + write in one step.
pub fn unpack(bytes: &[u8], dest: &Path) -> Result<Manifest, PkgError> {
    let pkg = read_package(bytes)?;
    write_package(&pkg, dest)?;
    Ok(pkg.manifest)
}

/// Pack a game directory into archive bytes. Deterministic: same inputs give
/// byte-identical output, so a package can be addressed by its own sha256.
pub fn pack_dir(dir: &Path) -> Result<Vec<u8>, PkgError> {
    let manifest_bytes = std::fs::read(dir.join(MANIFEST_FILE))
        .map_err(|_| PkgError::MissingMember(MANIFEST_FILE))?;
    let manifest = Manifest::parse(&manifest_bytes).map_err(PkgError::Manifest)?;
    let game = std::fs::read(dir.join(GAME_FILE)).map_err(|_| PkgError::MissingMember(GAME_FILE))?;
    if game.len() > MAX_GAME_BYTES {
        return Err(rejected("game.splash too large"));
    }

    let mut assets = Vec::new();
    collect_assets(&dir.join(ASSETS_DIR), ASSETS_DIR, &mut assets)?;
    // Sorted so the archive does not depend on directory iteration order.
    assets.sort_by(|a, b| a.0.cmp(&b.0));

    let mut w = ZipWriter::new();
    w.add(MANIFEST_FILE, &manifest_bytes, ZipMethod::Deflate)
        .map_err(|e| PkgError::Zip(format!("{e:?}")))?;
    w.add(GAME_FILE, &game, ZipMethod::Deflate)
        .map_err(|e| PkgError::Zip(format!("{e:?}")))?;
    for (name, data) in &assets {
        w.add(name, data, ZipMethod::Deflate)
            .map_err(|e| PkgError::Zip(format!("{e:?}")))?;
    }
    let _ = manifest;
    w.finish().map_err(|e| PkgError::Zip(format!("{e:?}")))
}

fn collect_assets(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), PkgError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(()); // no assets/ is fine
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let rel = format!("{prefix}/{name}");
        // symlink_metadata, not metadata: we must not follow a link out of the
        // game directory while packing.
        let meta = std::fs::symlink_metadata(&path).map_err(io)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_assets(&path, &rel, out)?;
        } else if meta.is_file() {
            if meta.len() > MAX_ENTRY_BYTES {
                return Err(rejected(format!("asset {rel} is too large")));
            }
            out.push((rel, std::fs::read(&path).map_err(io)?));
        }
        if out.len() > MAX_ENTRIES {
            return Err(rejected("too many assets"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_every_shape_of_hostile_name() {
        for bad in [
            "/etc/passwd",
            "../../etc/passwd",
            "a/../../b",
            "..",
            "./x",
            "a//b",
            "C:/windows/system32",
            "C:x",
            "back\\slash",
            "\\\\server\\share",
            "nul\0byte",
            "bell\x07",
        ] {
            assert!(
                safe_relative_path(bad).is_err(),
                "should reject {bad:?}"
            );
        }
        assert!(safe_relative_path(&"x".repeat(MAX_NAME_LEN + 1)).is_err());
        // And the ones that must pass:
        for ok in ["game.splash", "assets/a.png", "assets/deep/nested/b.bin"] {
            assert!(safe_relative_path(ok).is_ok(), "should accept {ok:?}");
        }
    }
}
