//! Attaching side-channel files to an already published asset revision.
//!
//! A side-channel is derived data the audio (or any) asset carries beside
//! its primary payload — separated stems, aligned lyrics — published as a
//! NEW revision of the same asset that reuses every existing blob and adds
//! the new roles. The audio bytes never move again: only the new files
//! upload, the manifest re-stages, and the head advances.
//!
//! Idempotent and race-tolerant by construction: if the head revision
//! already carries every requested role, nothing happens
//! ([`SideChannelOutcome::AlreadyPresent`]) — which is also what a client
//! sees when another machine finished the same bake first. Content
//! addressing makes the double-publish race benign: two writers of the same
//! bytes stage the same revision.

use crate::client::AssetClient;
use crate::error::{ClientError, ClientResult};
use makepad_asset_data::{AssetFile, AssetId, AssetRevisionId, FileRole, MediaType};

/// One derived file to attach.
#[derive(Clone, Debug)]
pub struct SideChannelFile {
    pub role: FileRole,
    pub media: MediaType,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideChannelOutcome {
    /// A new revision with the attached files is now the head.
    Published { revision: AssetRevisionId },
    /// The head already carried every requested role; nothing was written.
    AlreadyPresent { revision: AssetRevisionId },
}

impl AssetClient {
    /// Attach `files` to the asset's latest published revision, as a new
    /// revision reusing every existing blob. Files whose `(role, tier 0,
    /// lod 0)` slot already exists are replaced (a re-bake supersedes).
    ///
    /// Contract rules still bind: the manifest revalidates locally before
    /// staging, so an incomplete stem set or a side-channel on a non-audio
    /// asset refuses here, not at the server.
    pub fn publish_side_channel_files(
        &mut self,
        asset: &AssetId,
        files: Vec<SideChannelFile>,
    ) -> ClientResult<SideChannelOutcome> {
        if files.is_empty() {
            return Err(ClientError::InvalidInput { what: "no side-channel files" });
        }
        let detail = self.asset_detail(asset)?;
        let head = detail
            .latest_published()
            .ok_or(ClientError::NotFound { what: "published revision" })?;
        let head_revision = head.revision;
        let mut manifest = self.fetch_asset_manifest(&head_revision)?;

        // Idempotence: every requested role already present means done —
        // whoever wrote it first (this machine or another) won.
        if files.iter().all(|f| manifest.files.iter().any(|m| m.role == f.role)) {
            return Ok(SideChannelOutcome::AlreadyPresent { revision: head_revision });
        }

        // Replace same-slot entries, append the rest, keep canonical order.
        for file in &files {
            manifest.files.retain(|m| !(m.role == file.role && m.tier == makepad_asset_data::DeviceTier::Any && m.lod == 0));
        }
        for file in &files {
            manifest.files.push(AssetFile {
                role: file.role,
                tier: makepad_asset_data::DeviceTier::Any,
                lod: 0,
                media: file.media,
                blob: makepad_asset_data::BlobId::hash_of(&file.bytes),
                byte_len: file.bytes.len() as u64,
                dims: None,
            });
        }
        manifest.canonicalize();
        manifest.metrics.total_bytes = manifest
            .files
            .iter()
            .map(|f| f.byte_len)
            .chain(manifest.thumbnail.iter().map(|t| t.byte_len))
            .sum();
        let canonical = manifest
            .to_canonical_bytes()
            .map_err(|_| ClientError::InvalidInput { what: "side-channel manifest" })?;

        // Bytes before catalog rows; a blob the server already holds at the
        // exact size is not moved again.
        for file in &files {
            let blob = makepad_asset_data::BlobId::hash_of(&file.bytes);
            let present = match self.blob_head(&blob) {
                Ok(head) => head.size == file.bytes.len() as u64 && head.etag_matches,
                Err(ClientError::NotFound { .. }) => false,
                Err(e) => return Err(e),
            };
            if !present {
                self.api().upload_blob(&detail.namespace, &file.bytes)?;
            }
        }

        // Late race check: if another writer advanced the head to a revision
        // that already has the roles, stand down instead of re-staging over
        // their work.
        let latest = self.asset_detail(asset)?;
        if let Some(now_head) = latest.latest_published() {
            if now_head.revision != head_revision {
                let now = self.fetch_asset_manifest(&now_head.revision)?;
                if files.iter().all(|f| now.files.iter().any(|m| m.role == f.role)) {
                    return Ok(SideChannelOutcome::AlreadyPresent { revision: now_head.revision });
                }
            }
        }

        let revision = self.api().stage_asset_revision(asset, &canonical)?;
        self.api().publish_asset_revision(asset, &revision)?;
        Ok(SideChannelOutcome::Published { revision })
    }
}
