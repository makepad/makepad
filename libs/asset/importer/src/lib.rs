#![allow(dead_code)]
//! Library surface for the Asset Worker: the licensed pack compiler and the
//! media-inspection helpers the binary modes share.
//!
//! The networked coordinators stay in the binary. This crate exists so the
//! Asset UI Import page can compile a local Kenney (or later OSS) pack
//! through the same fail-closed path as `--import-pack`.

pub mod anim_icon;
pub mod ao_bake;
pub mod billboard_sheet;
pub mod vertex_skin;
pub mod classic_fetch;
pub mod classic_import;
pub mod iso9660;
pub mod tdm_zipsync;
pub mod doom3_import;
pub mod duke_import;
pub mod quake2_import;
pub mod quake3_import;
pub mod stateful_billboard;
pub mod world_preview;
pub mod world_place;
pub mod world_nav;
pub mod glb;
pub mod glb_nodes;
// Idempotent ai-content-library publication (one-shot + continuous). Public
// because the Asset UI runs the same watcher in-process against its own
// embedded Asset Server.
pub mod import;
pub mod watch;
pub mod coordinator;
// The wired generation kinds (job kind <-> fleet domain <-> catalog shape)
// and the profile advertisement built from a live fleet snapshot.
pub mod gen_kinds;
pub mod gen_profiles;
// The per-box claim fan-out + profile announcer both hosts run.
pub mod gen_service;
// Splash game folders -> `AssetKind::Game` assets (the sandbox's Games list).
pub mod games_import;
// A music directory tree -> `AssetKind::Audio` assets, tagged by folder name
// (the DJ's audio lane reads them straight back out of the catalog).
pub mod music_import;
pub mod pack_import;
pub mod spectrogram;
pub mod thumbs;
pub mod videothumb;

/// The head manifest of a revision, or `None` when this build cannot read it.
///
/// A schema bump makes every stored manifest unreadable ON PURPOSE: the
/// content contract's rule is that old decoders refuse rather than misread.
/// But "I cannot read what is already there" is not a reason to refuse to
/// WRITE. A re-import is exactly how a catalog crosses a schema bump — it is
/// the mechanism, not an accident — and a probe that returns an error where
/// it could return "nothing readable" brings the whole crossing to a halt
/// and leaves the catalog stranded on a version nothing can read.
///
/// So an unreadable head is treated as no head: the importer publishes a
/// fresh revision under the SAME asset id and the alias moves to something
/// this build can read. The asset keeps its identity and its history; only
/// the head becomes legible again.
///
/// Every other failure — the network, a refusal, a digest mismatch — is
/// still an error, because those are reasons to stop.
pub fn readable_head(
    client: &mut makepad_asset_client::AssetClient,
    revision: &makepad_asset_data::AssetRevisionId,
) -> Result<Option<makepad_asset_data::AssetManifest>, makepad_asset_client::ClientError> {
    match client.fetch_asset_manifest(revision) {
        Ok(manifest) => Ok(Some(manifest)),
        Err(error) if stale_schema(&error).is_some() => {
            let found = stale_schema(&error).unwrap_or(0);
            // Loud, once per asset: a catalog crossing a schema bump should
            // say so, not silently look like a first-time import. `0` is the
            // server-side refusal, where the old version is not reported.
            let was = match found {
                0 => "an older schema".to_string(),
                v => format!("schema v{v}"),
            };
            eprintln!(
                "[import] head manifest is {was}, this build reads v{} \
                 — publishing a fresh revision",
                makepad_asset_data::CONTENT_SCHEMA_VERSION
            );
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The schema version of a head this build is too new to read, if that is
/// what went wrong.
///
/// Two shapes, because a stored document can be refused at either end. The
/// client refuses it locally with `UnsupportedSchema`; the SERVER refuses to
/// serve it at all with a 422, because it validates what it holds against
/// the same contract. Both mean the same thing on this path: the head that
/// is there cannot be read under today's contract.
///
/// ONLY on the head-manifest FETCH path. A 422 while PUBLISHING means the
/// document being written is bad, which is a reason to stop, not a reason to
/// write it again.
///
/// Everything else — a timeout, a denial, a digest mismatch, a malformed
/// document — stays an error.
pub fn stale_schema(error: &makepad_asset_client::ClientError) -> Option<u16> {
    use makepad_asset_client::ClientError;
    if !error.is_unreadable_stored_document() {
        return None;
    }
    match error {
        ClientError::Content(makepad_asset_data::AssetDataError::UnsupportedSchema {
            found,
        }) => Some(*found),
        // The server holds it and will not serve it; it does not report
        // which version it was.
        _ => Some(0),
    }
}

#[cfg(test)]
mod tests {
    use makepad_asset_client::ClientError;
    use makepad_asset_data::AssetDataError;

    /// Crossing a schema bump: a head this build cannot read is treated as
    /// no head, so a re-import can WRITE one it can read. Every other
    /// failure still stops the import, because every other failure is a
    /// reason to stop.
    #[test]
    fn only_an_old_schema_counts_as_an_unreadable_head() {
        assert_eq!(
            super::stale_schema(&ClientError::Content(AssetDataError::UnsupportedSchema {
                found: 3
            })),
            Some(3)
        );
        assert_eq!(
            super::stale_schema(&ClientError::Server { status: 422, detail: None }),
            Some(0),
            "the server refusing to serve what it holds is the same situation"
        );
        // Not "unreadable": these are real failures.
        for error in [
            ClientError::Server { status: 500, detail: None },
            ClientError::Server { status: 409, detail: None },
            ClientError::Content(AssetDataError::BadMagic),
            ClientError::Content(AssetDataError::TrailingBytes),
            ClientError::Content(AssetDataError::Malformed { what: "thumbnail" }),
            ClientError::NotFound { what: "manifest" },
            ClientError::Denied,
            ClientError::Timeout { op: "fetch" },
            ClientError::DigestMismatch {
                what: "manifest",
                expected: [0; 32],
                found: [1; 32],
            },
        ] {
            assert_eq!(super::stale_schema(&error), None, "{error:?}");
        }
    }
}
