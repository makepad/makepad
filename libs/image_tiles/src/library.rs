//! Where a baked tile library lives on disk.
//!
//! One directory is one library:
//! - `library.sqlite` — the index: items and shard bookkeeping ([`crate::db`]).
//! - `tapes/<shard>/L<level>.mov` — one HEVC intra frame per file: the atlas
//!   page of a shard at one level. Level 0 is a 4096 px frame, level 4 a
//!   256 px one; one hardware decode fills a whole atlas level.
//! - `full/<item>.mov` — the picture at the highest resolution it was
//!   fetched at (capped to [`crate::tape::FULL_MAX_PX`]), one intra frame.
//! - `pyr/<item>-<px>.mov` — the picture's pre-cut zoom levels, named by
//!   long side, so a zoom is one small hardware decode of the level that
//!   fits, never the archival frame plus a rescale.
//! - `tmp/` — scratch for the baker.

use crate::tape::{read_frame, Planes, PYRAMID_LEVELS};
use std::path::{Path, PathBuf};

pub type ItemId = i64;

#[derive(Clone, Debug)]
pub struct Library {
    pub root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Library {
        Library { root: root.into() }
    }

    /// Resolve a library root the way apps launched from a source tree
    /// expect: `IMAGE_TILES_HOME` wins, else the nearest `local/` walking up
    /// from the working directory gets a `local/image-tiles`, else
    /// `./local/image-tiles`.
    pub fn resolve() -> Library {
        if let Ok(p) = std::env::var("IMAGE_TILES_HOME") {
            if !p.trim().is_empty() {
                return Library { root: PathBuf::from(p.trim()) };
            }
        }
        let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for _ in 0..5 {
            let candidate = dir.join("local");
            if candidate.is_dir() {
                return Library { root: candidate.join("image-tiles") };
            }
            if !dir.pop() {
                break;
            }
        }
        Library { root: PathBuf::from("local/image-tiles") }
    }

    pub fn ensure_dirs(&self) -> Result<(), String> {
        for sub in ["tapes", "full", "pyr", "tmp"] {
            std::fs::create_dir_all(self.root.join(sub)).map_err(|e| format!("create {sub}: {e}"))?;
        }
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("library.sqlite")
    }

    pub fn tape_path(&self, shard: i64, level: usize) -> PathBuf {
        self.root.join("tapes").join(format!("{shard:05}")).join(format!("L{level}.mov"))
    }

    pub fn full_path(&self, item: ItemId) -> PathBuf {
        self.root.join("full").join(format!("{item}.mov"))
    }

    /// One level of a picture's on-disk pyramid, named by its long side.
    pub fn pyramid_path(&self, item: ItemId, px: u32) -> PathBuf {
        self.root.join("pyr").join(format!("{item}-{px}.mov"))
    }

    pub fn exists(&self) -> bool {
        self.db_path().is_file()
    }
}

/// Mark a picture as needing no pyramid levels at all (already smaller than
/// every level), so a later pass skips it: an empty file at level "0".
pub fn mark_no_pyramid(library: &Library, id: ItemId) {
    let path = library.pyramid_path(id, 0);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, []);
}

/// The frame a viewer should draw for a picture wanted at `want_px` across:
/// the smallest pyramid level that still covers the ask, falling back to the
/// archival frame only when nothing smaller will do. The second value says
/// whether this is as fine as it gets — a viewer that cannot tell will never
/// ask for sharper.
pub fn display_frame(library: &Library, item: ItemId, want_px: u32) -> Result<(Planes, bool), String> {
    let mut pick: Option<u32> = None;
    for px in PYRAMID_LEVELS {
        if px >= want_px && library.pyramid_path(item, px).exists() {
            pick = Some(px);
        }
    }
    match pick {
        Some(px) => {
            let finer = PYRAMID_LEVELS.iter().any(|p| *p > px && library.pyramid_path(item, *p).exists())
                || library.full_path(item).exists();
            read_frame(&library.pyramid_path(item, px)).map(|p| (p, !finer))
        }
        None => read_frame(&library.full_path(item)).map(|p| (p, true)),
    }
}

/// Best-effort file check used by tools; the viewer trusts the database.
pub fn tape_exists(library: &Library, shard: i64) -> bool {
    (0..crate::tape::LEVELS).all(|l| library.tape_path(shard, l).is_file())
}

pub fn tmp_path(library: &Library, item: ItemId, ext: &str) -> PathBuf {
    library.root.join("tmp").join(format!("{item}-{}.{ext}", std::process::id()))
}

impl Library {
    pub fn as_path(&self) -> &Path {
        &self.root
    }
}
