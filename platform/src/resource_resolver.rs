//! Confined resource resolution and decoder budgets for untrusted documents.
//!
//! Documents (AI-written, from the store or a LAN host, shared by another
//! user) name their resources: `file("textures/wood.png")`, `asset(id, rev)`.
//! Every host boundary that turns such a name into bytes goes through a
//! [`ResourceResolver`]: a relative path, resolved beneath the document's
//! root, that can never reach anything else on the machine. An asset id is
//! checked against [`validate_asset_id`] before it becomes part of any path.
//!
//! Decoders then take a [`DecodeBudget`]: the image size a header claims is
//! multiplied out with checked arithmetic and charged *before* anything is
//! allocated, so a 60-byte file claiming 100000 x 100000 pixels fails with a
//! message instead of aborting the process on an out-of-memory allocation.

use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Why a resource was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceError {
    /// The name is empty, absolute, escapes the root (`..`), or holds a
    /// character no portable relative path has.
    InvalidPath(String),
    /// A path component is a symbolic link; links are never followed.
    Symlink(String),
    /// The resolved path lies outside the root (a race or a link above the
    /// component checks; defence in depth).
    Escape(String),
    /// The asset id does not match the id grammar.
    InvalidAssetId(String),
    /// A size or allocation went over the budget.
    Budget(String),
    /// The file system refused (missing file, permissions).
    Io(String),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidPath(m) => write!(f, "invalid resource path: {m}"),
            Self::Symlink(m) => write!(f, "symbolic links are not followed: {m}"),
            Self::Escape(m) => write!(f, "resource path escapes the document root: {m}"),
            Self::InvalidAssetId(m) => write!(f, "invalid asset id: {m}"),
            Self::Budget(m) => write!(f, "over budget: {m}"),
            Self::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ResourceError {}

/// Longest accepted relative resource path, in bytes.
pub const MAX_RESOURCE_PATH_BYTES: usize = 1024;
/// Longest accepted asset id, in bytes.
pub const MAX_ASSET_ID_BYTES: usize = 128;

/// Check an asset id (or revision) against the id grammar: 1 to 128 of
/// `A-Z a-z 0-9 _ - .`, not starting with `.`, and no `..`. An id that
/// passes can be used as a file stem (`<id>.glb`) without escaping its
/// directory or naming a hidden file.
pub fn validate_asset_id(id: &str) -> Result<(), ResourceError> {
    let bad = |why: &str| Err(ResourceError::InvalidAssetId(format!("{:?}: {why}", truncate(id))));
    if id.is_empty() {
        return bad("empty");
    }
    if id.len() > MAX_ASSET_ID_BYTES {
        return bad("too long");
    }
    if !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) {
        return bad("only A-Z a-z 0-9 _ - . are allowed");
    }
    if id.starts_with('.') || id.contains("..") {
        return bad("no leading dot and no `..`");
    }
    Ok(())
}

fn truncate(s: &str) -> String {
    s.chars().take(64).collect()
}

/// Resolves a document's relative resource names beneath one root
/// directory. Absolute paths, `..`, backslashes, drive prefixes, NUL and
/// symbolic links (at any component) are refused; the final path is
/// canonicalised and must still lie under the (canonical) root.
#[derive(Clone, Debug)]
pub struct ResourceResolver {
    root: PathBuf,
}

impl ResourceResolver {
    /// A resolver rooted at `root`, which must exist (it is canonicalised
    /// once here; a root that is itself reached through a link is fine, the
    /// host chose it).
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ResourceError> {
        let root = root.as_ref();
        let root = root
            .canonicalize()
            .map_err(|e| ResourceError::Io(format!("resource root {}: {e}", root.display())))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Check a relative name lexically: the normalised components it is made
    /// of (`.` dropped), or why it is refused.
    pub fn check_relative(name: &str) -> Result<Vec<&str>, ResourceError> {
        let bad = |why: &str| Err(ResourceError::InvalidPath(format!("{:?}: {why}", truncate(name))));
        if name.is_empty() {
            return bad("empty");
        }
        if name.len() > MAX_RESOURCE_PATH_BYTES {
            return bad("too long");
        }
        if name.contains('\0') {
            return bad("contains NUL");
        }
        // One separator on every platform: a backslash is a separator on
        // Windows and an ordinary file character elsewhere, so a document
        // that uses it would resolve differently per machine.
        if name.contains('\\') {
            return bad("use `/` as the separator");
        }
        if name.starts_with('/') {
            return bad("absolute paths are not allowed");
        }
        let mut parts = Vec::new();
        for part in name.split('/') {
            match part {
                "" | "." => {}
                ".." => return bad("`..` is not allowed"),
                // `C:` (a drive) or `name:stream` (NTFS alternate streams).
                p if p.contains(':') => return bad("`:` is not allowed"),
                p => parts.push(p),
            }
        }
        if parts.is_empty() {
            return bad("names no file");
        }
        // Belt and braces: the platform's own parser must agree that every
        // component is a plain name.
        let joined: PathBuf = parts.iter().collect();
        if !joined.components().all(|c| matches!(c, Component::Normal(_))) {
            return bad("not a plain relative path");
        }
        Ok(parts)
    }

    /// Resolve `name` to an existing file or directory beneath the root.
    pub fn resolve(&self, name: &str) -> Result<PathBuf, ResourceError> {
        let parts = Self::check_relative(name)?;
        let mut path = self.root.clone();
        for part in parts {
            path.push(part);
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| ResourceError::Io(format!("{name}: {e}")))?;
            if meta.file_type().is_symlink() {
                return Err(ResourceError::Symlink(name.to_string()));
            }
        }
        let canonical = path
            .canonicalize()
            .map_err(|e| ResourceError::Io(format!("{name}: {e}")))?;
        if !canonical.starts_with(&self.root) {
            return Err(ResourceError::Escape(name.to_string()));
        }
        Ok(canonical)
    }

    /// Read the file `name` names, refusing one larger than `max_bytes`
    /// (checked on the metadata and again while reading, so a file that
    /// grows underneath cannot exceed it).
    pub fn read(&self, name: &str, max_bytes: u64) -> Result<Vec<u8>, ResourceError> {
        let path = self.resolve(name)?;
        let file = std::fs::File::open(&path).map_err(|e| ResourceError::Io(format!("{name}: {e}")))?;
        let meta = file.metadata().map_err(|e| ResourceError::Io(format!("{name}: {e}")))?;
        if !meta.is_file() {
            return Err(ResourceError::InvalidPath(format!("{name}: not a file")));
        }
        if meta.len() > max_bytes {
            return Err(ResourceError::Budget(format!("{name} is {} bytes, over the {max_bytes}-byte limit", meta.len())));
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| ResourceError::Io(format!("{name}: {e}")))?;
        if bytes.len() as u64 > max_bytes {
            return Err(ResourceError::Budget(format!("{name} grew over the {max_bytes}-byte limit while reading")));
        }
        Ok(bytes)
    }
}

/// A byte budget decoders charge before they allocate: one per document (or
/// per load), shared by every decoder it runs.
#[derive(Clone, Debug)]
pub struct DecodeBudget {
    limit: u64,
    used: u64,
}

impl Default for DecodeBudget {
    /// 1 GiB of decoded data per document.
    fn default() -> Self {
        Self::new(1 << 30)
    }
}

impl DecodeBudget {
    pub fn new(limit: u64) -> Self {
        Self { limit, used: 0 }
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn used(&self) -> u64 {
        self.used
    }

    pub fn remaining(&self) -> u64 {
        self.limit - self.used
    }

    /// Bytes of a `width` x `height` image with `bytes_per_pixel`, with
    /// checked multiplication: an overflow is an error, never a wrap.
    pub fn image_bytes(width: u64, height: u64, bytes_per_pixel: u64) -> Result<u64, ResourceError> {
        width
            .checked_mul(height)
            .and_then(|px| px.checked_mul(bytes_per_pixel))
            .ok_or_else(|| ResourceError::Budget(format!("{width}x{height}x{bytes_per_pixel} bytes overflows")))
    }

    /// Take `bytes` for `what`, or refuse (and take nothing) when they do
    /// not fit.
    pub fn charge(&mut self, bytes: u64, what: &str) -> Result<(), ResourceError> {
        if bytes > self.remaining() {
            return Err(ResourceError::Budget(format!(
                "{what} needs {bytes} bytes, {} of the {}-byte budget are left",
                self.remaining(),
                self.limit
            )));
        }
        self.used += bytes;
        Ok(())
    }

    /// Give back bytes charged earlier (a temporary freed after decoding).
    pub fn release(&mut self, bytes: u64) {
        self.used = self.used.saturating_sub(bytes);
    }

    /// Charge a `width` x `height` image of `bytes_per_pixel`: the checked
    /// product, then the budget. Returns the byte count.
    pub fn charge_image(&mut self, width: u64, height: u64, bytes_per_pixel: u64, what: &str) -> Result<u64, ResourceError> {
        let bytes = Self::image_bytes(width, height, bytes_per_pixel)?;
        self.charge(bytes, what)?;
        Ok(bytes)
    }
}
