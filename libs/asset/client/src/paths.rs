//! Shared device-local library location for every asset producer and client.
//!
//! Generated source files and their index live at `local/asset-library`;
//! the catalog, content-addressed blobs and server identity live in `store`.
//! App caches remain separate. Explicit deployment overrides are preserved;
//! absent directories never select a different library behind the user's back.

use std::path::{Path, PathBuf};

fn override_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn checkout_root() -> PathBuf {
    override_path("MAKEPAD_ROOT")
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors().nth(3).expect("asset client lives under libs/asset/client").to_path_buf())
}

pub fn library_root() -> PathBuf {
    override_path("MAKEPAD_ASSET_LIBRARY")
        .unwrap_or_else(|| library_at(&checkout_root()))
}

/// `AI_CONTENT_ASSET_ROOT` remains an explicit server-root override.
pub fn store_root() -> PathBuf {
    override_path("AI_CONTENT_ASSET_ROOT")
        .unwrap_or_else(|| library_root().join("store"))
}

fn library_at(checkout: &Path) -> PathBuf {
    checkout.join("local/asset-library")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_and_store_share_one_root_even_before_it_exists() {
        let library = library_at(Path::new("/checkout"));
        assert_eq!(library, PathBuf::from("/checkout/local/asset-library"));
        assert_eq!(library.join("store"), PathBuf::from("/checkout/local/asset-library/store"));
    }
}
