//! Word-aligned lyrics, shared: the karaoke alignment core (whisper
//! cross-attention DTW, teacher-forced re-alignment, onset snapping) and the
//! one JSON schema those timings travel in.
//!
//! Two consumers, one implementation:
//! * the VJ decks, which bake lyrics live when a track has none, and
//! * the asset-ui analysis bake, which publishes the same JSON as the
//!   `Lyrics` side-channel on the audio asset.
//!
//! The schema is simultaneously the VJ's on-disk lyrics cache (v4) and the
//! server side-channel payload: a fetched `Lyrics` file can be dropped into
//! the local cache verbatim, because both are keyed by the same content
//! digest of the decoded PCM ([`track_digest`]).

pub mod align;
pub mod bake;
pub mod schema;

pub use schema::{LyricLine, OnsetStats, TrackLyrics, LYRICS_FORMAT, LYRICS_VERSION};

use makepad_asset_data::Sha256;

/// Content digest of decoded track audio: rate, frame count, then every
/// stereo i16 frame. The one key for the stem cache, the lyrics cache and
/// the analysed-side-channel dedupe — computed over DECODED samples so the
/// same track fetched from the store or read from disk shares one identity.
pub fn track_digest(sample_rate: u32, frames: &[[i16; 2]]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(&sample_rate.to_le_bytes());
    hasher.update(&(frames.len() as u64).to_le_bytes());
    let mut block: Vec<u8> = Vec::with_capacity(64 * 1024);
    for frame in frames {
        block.extend_from_slice(&frame[0].to_le_bytes());
        block.extend_from_slice(&frame[1].to_le_bytes());
        if block.len() >= 64 * 1024 {
            hasher.update(&block);
            block.clear();
        }
    }
    hasher.update(&block);
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}
