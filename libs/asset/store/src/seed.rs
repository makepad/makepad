//! Deterministic stock-seed interfaces.
//!
//! Stock content ships with the server binary and must exist identically on
//! every deployment: same bytes, same blob IDs, same asset IDs, same
//! revisions. The source supplies aliases, manifests and raw blob bytes; the
//! server derives every identity from content, so re-seeding is a no-op and
//! two servers seeded from the same source agree bit-for-bit.
//!
//! This module holds only types and derivations (no IO); the application of a
//! seed lives on `AssetServerCore::apply_stock_seed`.

use makepad_asset_data::{sha256, AssetAlias, AssetId, AssetManifest};

/// One stock asset: its authoring alias, its manifest, and the raw bytes of
/// every blob the manifest references (files and thumbnail).
pub struct SeedAsset {
    pub alias: AssetAlias,
    pub manifest: AssetManifest,
    pub blobs: Vec<Vec<u8>>,
}

/// A provider of stock content. Implementations must be deterministic: the
/// same source version always returns the same assets in any order (the
/// server sorts by alias before applying).
pub trait StockSeedSource {
    /// Stable name for reporting.
    fn name(&self) -> &'static str;
    fn assets(&self) -> Vec<SeedAsset>;
}

/// Deterministic identity for a stock asset: the first 16 bytes of
/// SHA-256("makepad-stock-asset-v1\0" + alias). Every deployment derives the
/// same AssetId for the same alias without coordination.
pub fn stock_asset_id(alias: &AssetAlias) -> AssetId {
    let mut input = Vec::with_capacity(24 + alias.as_str().len());
    input.extend_from_slice(b"makepad-stock-asset-v1\0");
    input.extend_from_slice(alias.as_str().as_bytes());
    let digest = sha256(&input);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    AssetId::from_bytes(id)
}

/// What one apply pass did. Re-running a seed yields zero `*_new` counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeedReport {
    pub assets_seen: u32,
    pub assets_published_new: u32,
    pub assets_already_published: u32,
    pub blobs_written: u32,
    pub blobs_deduped: u32,
}
