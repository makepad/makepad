//! Stems and lyrics as precomputed, server-stored side-channels.
//!
//! The heavy AI analysis of a track — BS-RoFormer stem separation, whisper
//! word alignment — runs ONCE, wherever it happens first (the asset-ui's
//! deliberate bake, or a VJ deck computing locally as a fallback), and the
//! result is published back onto the audio asset as typed side-channel
//! files: four ~256 kbit/s Ogg Vorbis stems and one lyrics JSON. Every
//! other client then gets "full fancy" behaviour by FETCHING — a deck load
//! decodes four small oggs in well under a second instead of spending half
//! a minute of GPU separation.
//!
//! This crate is the one implementation both apps call: stem encoding
//! (parallel, deterministic — same PCM in, same bytes out, so the store
//! dedupes double bakes), the role mapping between the separation model's
//! stem order and the contract's, and the idempotent publish
//! ([`publish_side_channels`], on top of
//! `AssetClient::publish_side_channel_files`).

use makepad_ai_stems::{StemSet, SAMPLE_RATE as STEMS_RATE};
use makepad_asset_client::client::AssetClient;
use makepad_asset_client::error::ClientResult;
use makepad_asset_client::side_channels::{SideChannelFile, SideChannelOutcome};
use makepad_asset_data::{AssetId, FileRole, MediaType};
use makepad_audio_encode::{encode_vorbis, EncodeOptions};

/// The one stem encode setting: a flat ~256 kbit/s per stem, deliberately
/// NOT matched to the input's bitrate. The separation model's own artifact
/// floor (~9.6 dB SDR) dominates far above codec noise at this rate, so
/// 256k is comfortably "never the bottleneck" while staying cascade-safe on
/// lossy inputs; higher is provably inaudible improvement, lower risks
/// stacking artifacts. Quality 0.85 of our encoder lands there on stem-like
/// (sparse) material.
pub const STEM_QUALITY: f32 = 0.85;

/// Encoder options for one stem. `threads: 1` — stem-level parallelism (four
/// encodes at once) beats block-level here, and keeps each output byte-form
/// independent of the machine's core count.
pub fn stem_encode_options() -> EncodeOptions {
    EncodeOptions { quality: STEM_QUALITY, threads: 1, tags: Vec::new() }
}

/// The contract's stem role order, and where each lives in the separation
/// model's `StemSet` (`[drums, bass, other, vocals]`).
pub const STEM_ROLE_TO_SET: [(FileRole, usize); 4] = [
    (FileRole::StemDrums, 0),
    (FileRole::StemBass, 1),
    (FileRole::StemVocals, 3),
    (FileRole::StemOther, 2),
];

fn encode_one_stem(buf: &makepad_ai_stems::StereoBuf, opts: &EncodeOptions) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(buf.left.len() * 2);
    for (l, r) in buf.left.iter().zip(buf.right.iter()) {
        pcm.push(*l);
        pcm.push(*r);
    }
    encode_vorbis(STEMS_RATE, 2, &pcm, opts).expect("stem encode")
}

/// Encode a full separated `StemSet` (at the model's 44.1 kHz) to four Ogg
/// Vorbis streams, in [`FileRole::STEMS`] order. Native uses one thread per
/// stem; the web build has no `std::thread` workers, so it encodes in order.
pub fn encode_stem_oggs(stems: &StemSet) -> [Vec<u8>; 4] {
    let opts = stem_encode_options();
    let mut out: [Vec<u8>; 4] = Default::default();
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (_, set_index) in STEM_ROLE_TO_SET {
            let buf = &stems[set_index];
            let opts = opts.clone();
            handles.push(scope.spawn(move || encode_one_stem(buf, &opts)));
        }
        for (slot, handle) in out.iter_mut().zip(handles) {
            *slot = handle.join().expect("stem encode worker");
        }
    });
    #[cfg(target_arch = "wasm32")]
    {
        for (slot, (_, set_index)) in out.iter_mut().zip(STEM_ROLE_TO_SET) {
            *slot = encode_one_stem(&stems[set_index], &opts);
        }
    }
    out
}

/// The typed side-channel files for the original stem/lyrics bake contract.
pub fn side_channel_files(
    stem_oggs: Option<[Vec<u8>; 4]>,
    lyrics_json: Option<String>,
) -> Vec<SideChannelFile> {
    side_channel_files_with_analysis(stem_oggs, lyrics_json, None, None)
}

/// The complete DJ cache side-channel set: four stem oggs (in
/// [`FileRole::STEMS`] order), lyrics, the native whole-track analysis cache,
/// and the prebuilt loop-splat grid.
pub fn side_channel_files_with_analysis(
    stem_oggs: Option<[Vec<u8>; 4]>,
    lyrics_json: Option<String>,
    dj_analysis: Option<Vec<u8>>,
    dj_loop_splat: Option<Vec<u8>>,
) -> Vec<SideChannelFile> {
    let mut files = Vec::new();
    if let Some(oggs) = stem_oggs {
        for ((role, _), bytes) in STEM_ROLE_TO_SET.into_iter().zip(oggs) {
            files.push(SideChannelFile { role, media: MediaType::Ogg, bytes });
        }
    }
    if let Some(json) = lyrics_json {
        files.push(SideChannelFile {
            role: FileRole::Lyrics,
            media: MediaType::Json,
            bytes: json.into_bytes(),
        });
    }
    if let Some(bytes) = dj_analysis {
        files.push(SideChannelFile {
            role: FileRole::DjAnalysis,
            media: MediaType::Bin,
            bytes,
        });
    }
    if let Some(bytes) = dj_loop_splat {
        files.push(SideChannelFile {
            role: FileRole::DjLoopSplat,
            media: MediaType::Bin,
            bytes,
        });
    }
    files
}

/// Publish a bake's outputs onto `asset` as a revision update: same audio
/// blob, side-channels attached. Idempotent — a re-run or a concurrent
/// winner reports [`SideChannelOutcome::AlreadyPresent`].
pub fn publish_side_channels(
    client: &mut AssetClient,
    asset: &AssetId,
    stem_oggs: Option<[Vec<u8>; 4]>,
    lyrics_json: Option<String>,
) -> ClientResult<SideChannelOutcome> {
    client.publish_side_channel_files(asset, side_channel_files(stem_oggs, lyrics_json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_stems::StereoBuf;

    fn tone_set() -> StemSet {
        // Four distinguishable half-second tones.
        std::array::from_fn(|i| {
            let hz = [220.0, 110.0, 660.0, 440.0][i];
            let n = STEMS_RATE as usize / 2;
            let wave: Vec<f32> = (0..n)
                .map(|t| {
                    (2.0 * std::f32::consts::PI * hz * t as f32 / STEMS_RATE as f32).sin() * 0.4
                })
                .collect();
            StereoBuf { left: wave.clone(), right: wave }
        })
    }

    #[test]
    fn stems_encode_decode_and_map_roles() {
        let set = tone_set();
        let oggs = encode_stem_oggs(&set);
        // FileRole::STEMS order: drums(set 0), bass(1), vocals(3), other(2).
        let expect_hz = [220.0f32, 110.0, 440.0, 660.0];
        for (i, ogg) in oggs.iter().enumerate() {
            let decoded = makepad_audio_decode::decode_any(ogg).expect("stem decodes");
            assert_eq!(decoded.rate, STEMS_RATE);
            assert_eq!(decoded.channels, 2);
            // Dominant frequency via zero crossings of the mid channel.
            let mono: Vec<f32> = decoded
                .pcm_interleaved_f32
                .chunks(2)
                .map(|f| (f[0] + f[1]) * 0.5)
                .collect();
            let mid = &mono[mono.len() / 4..mono.len() * 3 / 4];
            let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
            let hz = crossings as f32 * STEMS_RATE as f32 / mid.len() as f32;
            let want = expect_hz[i];
            assert!(
                (hz - want).abs() < want * 0.05,
                "stem {i}: measured {hz} Hz, want {want}"
            );
        }
    }

    #[test]
    fn encoding_is_deterministic_across_runs() {
        let set = tone_set();
        let a = encode_stem_oggs(&set);
        let b = encode_stem_oggs(&set);
        assert_eq!(a, b);
    }

    #[test]
    fn files_carry_the_contract_roles() {
        let set = tone_set();
        let files = side_channel_files_with_analysis(
            Some(encode_stem_oggs(&set)),
            Some("{}".into()),
            Some(b"wave".to_vec()),
            Some(b"splat".to_vec()),
        );
        let roles: Vec<FileRole> = files.iter().map(|f| f.role).collect();
        assert_eq!(
            roles,
            vec![
                FileRole::StemDrums,
                FileRole::StemBass,
                FileRole::StemVocals,
                FileRole::StemOther,
                FileRole::Lyrics,
                FileRole::DjAnalysis,
                FileRole::DjLoopSplat,
            ]
        );
        assert!(files[..4].iter().all(|f| f.media == MediaType::Ogg));
        assert_eq!(files[4].media, MediaType::Json);
        assert!(files[5..].iter().all(|f| f.media == MediaType::Bin));
    }
}
