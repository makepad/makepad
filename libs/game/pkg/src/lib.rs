//! Makepad Arcade packaging and sharing (game.md §"Game format + sharing").
//!
//! A game is a directory: `game.splash` + `manifest.toml` + `assets/`. A package
//! is that directory as a deterministic zip, so it can be addressed by its own
//! sha256 and verified after any transport.
//!
//! Everything downstream of `read_package` treats its input as hostile — the
//! archives and index bytes here arrive from other people's machines.

pub mod library;
pub mod manifest;
pub mod pack;
pub mod registry;
pub mod sha256;

pub use library::{Library, LibraryEntry};
pub use manifest::{Knob, Manifest, ManifestError};
pub use pack::{
    pack_dir, read_package, unpack, write_package, Package, PkgError, ASSETS_DIR, GAME_FILE,
    MANIFEST_FILE, PACKAGE_EXT,
};
pub use registry::{fetch_lan_package, verify_digest, HttpError, IndexEntry, Registry};
pub use sha256::{sha256, sha256_hex};
