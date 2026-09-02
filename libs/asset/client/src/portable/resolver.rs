//! Portable manifest selection vocabulary. Materialization remains unavailable.

use crate::error::{ClientError, ClientResult};
use makepad_asset_data::{
    limits, AssetFile, AssetManifest, BlobId, DeviceTier, FileRole, MediaType, ThumbnailMedia,
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierPreference {
    Exact(DeviceTier),
    PreferWithAnyFallback(DeviceTier),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub media: MediaType,
    pub blob: BlobId,
    pub byte_len: u64,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedThumbnail {
    pub blob: BlobId,
    pub media: ThumbnailMedia,
    pub width: u32,
    pub height: u32,
    pub byte_len: u64,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug)]
pub struct ClosureBudget {
    pub max_assets: usize,
    pub max_depth: u32,
}

impl Default for ClosureBudget {
    fn default() -> Self {
        Self { max_assets: 512, max_depth: limits::MAX_DEPENDENCY_DEPTH }
    }
}

pub fn select_file(
    manifest: &AssetManifest,
    role: FileRole,
    tier: TierPreference,
    max_lod: u8,
) -> ClientResult<&AssetFile> {
    let (wanted, allow_any) = match tier {
        TierPreference::Exact(tier) => (tier, false),
        TierPreference::PreferWithAnyFallback(tier) => (tier, true),
    };
    manifest
        .files
        .iter()
        .filter(|file| {
            file.role == role
                && file.lod <= max_lod
                && (file.tier == wanted || (allow_any && file.tier == DeviceTier::Any))
        })
        .min_by_key(|file| (file.lod, file.tier != wanted))
        .ok_or(ClientError::NotFound { what: "matching asset file" })
}
