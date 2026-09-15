//! From typed manifests to verified local content — with zero guessing.
//!
//! Selection is by explicit `(role, tier, lod)` policy against the manifest's
//! typed file table, never by filename, extension, or directory listing. The
//! returned paths come from the verified cache: their bytes re-hashed to the
//! manifest's declared digest on this resolve. Dependency closures walk exact
//! `AssetRevisionRef` pairs (digests all the way down), bounded by the
//! content contract's dependency budgets.

#[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
use crate::client::AssetClient;
use crate::cache_store::BlobContent;
use crate::error::{ClientError, ClientResult};
use makepad_asset_data::{
    limits, AssetFile, AssetManifest, BlobId, DeviceTier, FileRole, MediaType, ThumbnailMedia,
};
#[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
use makepad_asset_data::AssetRevisionRef;

/// How the device tier column is matched. Always explicit and deterministic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierPreference {
    /// Only files declaring exactly this tier.
    Exact(DeviceTier),
    /// Files of this tier, else files declaring `DeviceTier::Any`; an exact
    /// tier match always outranks an `Any` fallback.
    PreferWithAnyFallback(DeviceTier),
}

/// One verified, locally materialized asset file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub media: MediaType,
    pub blob: BlobId,
    pub byte_len: u64,
    /// Verified local content. Native dynamic-server clients normally return
    /// a cache path; static/portable clients return immutable bytes.
    pub content: BlobContent,
}

/// The manifest's mandatory thumbnail, verified and materialized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedThumbnail {
    pub blob: BlobId,
    pub media: ThumbnailMedia,
    pub width: u32,
    pub height: u32,
    pub byte_len: u64,
    pub content: BlobContent,
}

/// Bounds for a dependency-closure walk. Defaults mirror the content
/// contract's admission budgets.
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

/// Deterministic file selection. Eligible entries match `role`, satisfy the
/// tier policy, and have `lod <= max_lod`; among them the most detailed
/// (lowest lod) wins, with an exact tier match outranking an `Any` fallback
/// at equal lod. Returns a refusal — never a substitute — when nothing
/// matches.
pub fn select_file<'m>(
    manifest: &'m AssetManifest,
    role: FileRole,
    tier: TierPreference,
    max_lod: u8,
) -> ClientResult<&'m AssetFile> {
    let (want, allow_any) = match tier {
        TierPreference::Exact(t) => (t, false),
        TierPreference::PreferWithAnyFallback(t) => (t, true),
    };
    let mut best: Option<(&AssetFile, bool)> = None; // (file, exact_tier)
    for f in &manifest.files {
        if f.role != role || f.lod > max_lod {
            continue;
        }
        let exact = f.tier == want;
        if !exact && !(allow_any && f.tier == DeviceTier::Any) {
            continue;
        }
        best = match best {
            None => Some((f, exact)),
            Some((cur, cur_exact)) => {
                let better = (f.lod, !exact) < (cur.lod, !cur_exact);
                if better {
                    Some((f, exact))
                } else {
                    Some((cur, cur_exact))
                }
            }
        };
    }
    best.map(|(f, _)| f).ok_or(ClientError::NotFound { what: "matching asset file" })
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "web")))]
impl AssetClient {
    /// Select per the policy and materialize the file: cache-first fetch,
    /// digest verification, verified local path.
    pub fn resolve_file(
        &mut self,
        manifest: &AssetManifest,
        role: FileRole,
        tier: TierPreference,
        max_lod: u8,
        progress: Option<&mut dyn FnMut(u64, u64)>,
    ) -> ClientResult<ResolvedFile> {
        let file = select_file(manifest, role, tier, max_lod)?.clone();
        let path = self.fetch_blob(&file.blob, Some(file.byte_len), progress)?;
        Ok(ResolvedFile {
            role: file.role,
            tier: file.tier,
            lod: file.lod,
            media: file.media,
            blob: file.blob,
            byte_len: file.byte_len,
            content: BlobContent::VerifiedPath(path),
        })
    }

    /// As [`resolve_file`](Self::resolve_file) but returning verified bytes.
    pub fn resolve_file_bytes(
        &mut self,
        manifest: &AssetManifest,
        role: FileRole,
        tier: TierPreference,
        max_lod: u8,
    ) -> ClientResult<Vec<u8>> {
        let file = select_file(manifest, role, tier, max_lod)?.clone();
        let bytes = self.fetch_blob_bytes(&file.blob, Some(file.byte_len))?;
        if bytes.len() as u64 != file.byte_len {
            return Err(ClientError::SizeMismatch {
                what: "resolved file bytes",
                expected: file.byte_len,
                found: bytes.len() as u64,
            });
        }
        Ok(bytes)
    }

    /// Materialize the manifest's typed thumbnail. `Ok(None)` when the
    /// manifest legitimately has none (non-mesh kinds); the UI renders its
    /// honest placeholder, never an invented image.
    pub fn resolve_thumbnail(
        &mut self,
        manifest: &AssetManifest,
    ) -> ClientResult<Option<ResolvedThumbnail>> {
        let Some(t) = &manifest.thumbnail else {
            return Ok(None);
        };
        let t = t.clone();
        let path = self.fetch_blob(&t.blob, Some(t.byte_len), None)?;
        Ok(Some(ResolvedThumbnail {
            blob: t.blob,
            media: t.media,
            width: t.width,
            height: t.height,
            byte_len: t.byte_len,
            content: BlobContent::VerifiedPath(path),
        }))
    }

    /// Fetch the manifest for an exact `{asset_id, revision}` pair and prove
    /// the pair: the (digest-verified) manifest must declare the same
    /// asset_id, so an alias/listing answer cannot splice one asset's
    /// revision onto another's identity.
    pub fn resolve_ref(&mut self, r: &AssetRevisionRef) -> ClientResult<AssetManifest> {
        let manifest = self.fetch_asset_manifest(&r.revision)?;
        if manifest.asset_id != r.asset_id {
            return Err(ClientError::Protocol { what: "revision belongs to a different asset" });
        }
        Ok(manifest)
    }

    /// Breadth-first dependency closure from `root`, deduplicated by
    /// revision, bounded by `budget`. Returns `(ref, manifest)` pairs in
    /// deterministic BFS order, root first. Cycles are cryptographically
    /// impossible (a manifest's digest covers its dependency digests), so the
    /// budgets bound work, not correctness.
    pub fn resolve_closure(
        &mut self,
        root: &AssetRevisionRef,
        budget: ClosureBudget,
    ) -> ClientResult<Vec<(AssetRevisionRef, AssetManifest)>> {
        if budget.max_assets == 0 {
            return Err(ClientError::InvalidInput { what: "closure max_assets" });
        }
        let mut out: Vec<(AssetRevisionRef, AssetManifest)> = Vec::new();
        let mut seen: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
        let mut frontier: Vec<AssetRevisionRef> = vec![*root];
        seen.insert(*root.revision.as_bytes());
        let mut depth = 0u32;
        while !frontier.is_empty() {
            if depth > budget.max_depth {
                return Err(ClientError::OverBudget {
                    what: "closure depth",
                    limit: budget.max_depth as u64,
                    found: depth as u64,
                });
            }
            let mut next: Vec<AssetRevisionRef> = Vec::new();
            for r in frontier {
                let manifest = self.resolve_ref(&r)?;
                for dep in &manifest.dependencies {
                    if seen.insert(*dep.revision.as_bytes()) {
                        next.push(*dep);
                    }
                }
                out.push((r, manifest));
                if out.len() > budget.max_assets {
                    return Err(ClientError::OverBudget {
                        what: "closure assets",
                        limit: budget.max_assets as u64,
                        found: out.len() as u64,
                    });
                }
            }
            frontier = next;
            depth += 1;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_data::*;

    fn file(role: FileRole, tier: DeviceTier, lod: u8, seed: u8) -> AssetFile {
        AssetFile {
            role,
            tier,
            lod,
            media: MediaType::Glb,
            blob: BlobId::from_bytes([seed; 32]),
            byte_len: 100 + seed as u64,
            dims: None,
        }
    }

    fn manifest_with(files: Vec<AssetFile>) -> AssetManifest {
        AssetManifest {
            asset_id: AssetId::from_bytes([1; 16]),
            kind: AssetKind::Prop,
            files,
            dependencies: vec![],
            thumbnail: None,
            metrics: Metrics {
                total_bytes: 0,
                triangles: 0,
                vertices: 0,
                joints: 0,
                clips: 0,
                max_texture_dim: 0,
                media_millis: 0,
            },
            coordinate_system: CoordinateSystem {
                units_per_meter: 1.0,
                up: Axis::YPos,
                forward: Axis::ZNeg,
                pivot: Pivot::Origin,
            },
            bounds: Bounds {
                min: Vec3::new(-1.0, -1.0, -1.0),
                max: Vec3::new(1.0, 1.0, 1.0),
            },
            anchors: vec![],
            capabilities: Capabilities {
                rigged: false,
                animated: false,
                collidable: false,
                loopable: false,
                spawnable: false,
            },
            spawn_recipe: None,
            provenance: None,
            rights: Rights {
                license: "CC0-1.0".into(),
                license_revision: String::new(),
                terms_digest: None,
                terms_url: String::new(),
                credits: "t".into(),
                source: String::new(),
                source_archive: None,
                redistribution: makepad_asset_data::Redistribution::Allowed,
                derivatives: makepad_asset_data::DerivativePolicy::Allowed,
            },
        }
    }

    #[test]
    fn selection_is_explicit_and_deterministic() {
        let m = manifest_with(vec![
            file(FileRole::RenderGlb, DeviceTier::Any, 0, 1),
            file(FileRole::RenderGlb, DeviceTier::High, 0, 2),
            file(FileRole::RenderGlb, DeviceTier::Any, 2, 3),
            file(FileRole::Collider, DeviceTier::Any, 0, 4),
        ]);
        // Exact tier only.
        let f = select_file(&m, FileRole::RenderGlb, TierPreference::Exact(DeviceTier::High), 7)
            .unwrap();
        assert_eq!(f.blob, BlobId::from_bytes([2; 32]));
        assert!(select_file(&m, FileRole::RenderGlb, TierPreference::Exact(DeviceTier::Low), 7)
            .is_err());
        // Preference: exact outranks Any at equal lod.
        let f = select_file(
            &m,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::High),
            7,
        )
        .unwrap();
        assert_eq!(f.blob, BlobId::from_bytes([2; 32]));
        // Low tier falls back to Any lod 0.
        let f = select_file(
            &m,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::Low),
            7,
        )
        .unwrap();
        assert_eq!(f.blob, BlobId::from_bytes([1; 32]));
        // lod cap: only the lod-2 Any entry sits above cap 1? lod 0 exists, so
        // cap 1 still picks lod 0; cap forcing exclusion refuses.
        let only_lod2 = manifest_with(vec![file(FileRole::RenderGlb, DeviceTier::Any, 2, 9)]);
        assert!(select_file(
            &only_lod2,
            FileRole::RenderGlb,
            TierPreference::PreferWithAnyFallback(DeviceTier::High),
            1,
        )
        .is_err());
        // Role is never substituted.
        assert!(select_file(&m, FileRole::Audio, TierPreference::Exact(DeviceTier::Any), 7)
            .is_err());
    }
}
