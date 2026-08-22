//! The stem/lyrics side-channel contract: frozen wire tags for the appended
//! roles and the Json media type, and the all-four-or-none stem rule.
//!
//! Follows `thumbnail_views.rs`: enum tags are append-only — these tests pin
//! the numbers so a renumber (which would silently re-role every stored
//! manifest) fails loudly.

mod common;

use common::*;
use makepad_asset_data::*;

fn audio_file(role: FileRole, media: MediaType, n: u8, len: u64) -> AssetFile {
    AssetFile { role, tier: DeviceTier::Any, lod: 0, media, blob: blob(n), byte_len: len, dims: None }
}

/// A published audio track with the full side-channel set: the original mp3,
/// four ogg stems and the lyrics json.
fn audio_manifest_with_side_channels() -> AssetManifest {
    let files = vec![
        audio_file(FileRole::Audio, MediaType::Mp3, 1, 9000),
        audio_file(FileRole::StemDrums, MediaType::Ogg, 2, 4000),
        audio_file(FileRole::StemBass, MediaType::Ogg, 3, 3000),
        audio_file(FileRole::StemVocals, MediaType::Ogg, 4, 3500),
        audio_file(FileRole::StemOther, MediaType::Ogg, 5, 4500),
        audio_file(FileRole::Lyrics, MediaType::Json, 6, 700),
    ];
    let total: u64 = files.iter().map(|f| f.byte_len).sum();
    AssetManifest {
        asset_id: asset_id(9),
        kind: AssetKind::Audio,
        files,
        dependencies: vec![],
        thumbnail: None,
        metrics: Metrics {
            total_bytes: total,
            triangles: 0,
            vertices: 0,
            joints: 0,
            clips: 0,
            max_texture_dim: 0,
            media_millis: 187_000,
        },
        coordinate_system: CoordinateSystem {
            units_per_meter: 1.0,
            up: Axis::YPos,
            forward: Axis::ZNeg,
            pivot: Pivot::Origin,
        },
        bounds: Bounds { min: Vec3::ZERO, max: Vec3::ZERO },
        anchors: vec![],
        capabilities: Capabilities {
            rigged: false,
            animated: false,
            collidable: false,
            loopable: true,
            spawnable: false,
        },
        spawn_recipe: None,
        provenance: None,
        rights: cc0_rights("test track", ""),
    }
}

/// Locate the file record whose blob is `[n; 32]` and return its leading
/// role tag byte. The tags are not public API, so they are pinned through
/// the only thing that is: the bytes (an `AssetFile` encodes role, tier,
/// lod, media, then the 32-byte blob id).
fn role_tag_of(bytes: &[u8], n: u8) -> (u8, u8) {
    let blob = [n; 32];
    // `rposition`, not `position`: a media tag byte equal to `n` directly
    // before the blob would otherwise shift the window one byte early.
    let at = bytes
        .windows(32)
        .rposition(|w| w == blob)
        .unwrap_or_else(|| panic!("blob {n} not found"));
    (bytes[at - 4], bytes[at - 1])
}

#[test]
fn side_channel_tags_are_frozen_and_append_only() {
    // The wire numbers, pinned via the encoded bytes. Changing any of these
    // re-roles every stored manifest: that is never a refactor.
    let m = audio_manifest_with_side_channels();
    let bytes = m.to_canonical_bytes().unwrap();
    let (audio_role, audio_media) = role_tag_of(&bytes, 1);
    assert_eq!((audio_role, audio_media), (13, 9), "audio role / mp3 media");
    assert_eq!(role_tag_of(&bytes, 2), (19, 4), "stem_drums / ogg");
    assert_eq!(role_tag_of(&bytes, 3), (20, 4), "stem_bass / ogg");
    assert_eq!(role_tag_of(&bytes, 4), (21, 4), "stem_vocals / ogg");
    assert_eq!(role_tag_of(&bytes, 5), (22, 4), "stem_other / ogg");
    assert_eq!(role_tag_of(&bytes, 6), (23, 10), "lyrics / json");
}

#[test]
fn stems_are_ogg_only_and_lyrics_json_only() {
    for stem in FileRole::STEMS {
        assert!(stem.allows(MediaType::Ogg));
        assert!(!stem.allows(MediaType::Mp3));
        assert!(!stem.allows(MediaType::Wav));
        assert!(!stem.allows(MediaType::Json));
        assert!(stem.is_stem());
    }
    assert!(FileRole::Lyrics.allows(MediaType::Json));
    assert!(!FileRole::Lyrics.allows(MediaType::Text));
    assert!(!FileRole::Lyrics.allows(MediaType::Ogg));
    assert!(!FileRole::Lyrics.is_stem());
    assert!(!FileRole::Audio.is_stem());
}

#[test]
fn a_full_side_channel_manifest_round_trips() {
    let m = audio_manifest_with_side_channels();
    let bytes = m.to_canonical_bytes().expect("encode");
    let back = AssetManifest::from_canonical_bytes(&bytes).expect("decode");
    assert_eq!(back, m);
    assert_eq!(back.files.iter().filter(|f| f.role.is_stem()).count(), 4);
}

#[test]
fn stems_are_all_four_or_none() {
    // Dropping any single stem must refuse.
    for missing in FileRole::STEMS {
        let mut m = audio_manifest_with_side_channels();
        m.files.retain(|f| f.role != missing);
        m.metrics.total_bytes = m.files.iter().map(|f| f.byte_len).sum();
        let err = m.to_canonical_bytes().unwrap_err();
        assert!(
            matches!(err, AssetDataError::Missing { .. }),
            "dropping {missing:?} gave {err:?}"
        );
    }
    // No stems at all is fine (lyrics may ride alone).
    let mut m = audio_manifest_with_side_channels();
    m.files.retain(|f| !f.role.is_stem());
    m.metrics.total_bytes = m.files.iter().map(|f| f.byte_len).sum();
    m.to_canonical_bytes().expect("audio + lyrics only");
    // And no side-channels at all is the plain audio asset.
    let mut m = audio_manifest_with_side_channels();
    m.files.retain(|f| !f.role.is_stem() && f.role != FileRole::Lyrics);
    m.metrics.total_bytes = m.files.iter().map(|f| f.byte_len).sum();
    m.to_canonical_bytes().expect("plain audio");
}

#[test]
fn side_channels_refuse_on_non_audio_kinds() {
    let mut m = weapon_manifest();
    m.files.push(audio_file(FileRole::Lyrics, MediaType::Json, 9, 100));
    m.metrics.total_bytes += 100;
    assert!(m.to_canonical_bytes().is_err(), "lyrics on a weapon must refuse");

    let mut m = weapon_manifest();
    for (i, stem) in FileRole::STEMS.into_iter().enumerate() {
        m.files.push(audio_file(stem, MediaType::Ogg, 10 + i as u8, 100));
    }
    m.metrics.total_bytes += 400;
    assert!(m.to_canonical_bytes().is_err(), "stems on a weapon must refuse");
}

#[test]
fn a_stem_with_the_wrong_media_refuses() {
    let mut m = audio_manifest_with_side_channels();
    for f in m.files.iter_mut() {
        if f.role == FileRole::StemBass {
            f.media = MediaType::Mp3;
        }
    }
    assert!(matches!(
        m.to_canonical_bytes().unwrap_err(),
        AssetDataError::Mismatch { .. }
    ));
}

#[test]
fn an_unknown_role_tag_refuses() {
    // Encode a good manifest, then bump a stem role byte to the next free
    // tag: an old reader must refuse, not misread.
    let m = audio_manifest_with_side_channels();
    let bytes = m.to_canonical_bytes().unwrap();
    // Find the StemOther tag byte (22) that sits in a role position by
    // corrupting every 22 byte and checking at least one flips the decode
    // into BadTag; the decode must never panic either way.
    let mut refused = false;
    for i in 0..bytes.len() {
        if bytes[i] == 22 {
            let mut corrupt = bytes.clone();
            corrupt[i] = 63;
            match AssetManifest::from_canonical_bytes(&corrupt) {
                Err(AssetDataError::BadTag { .. }) => refused = true,
                _ => {}
            }
        }
    }
    assert!(refused, "no role byte position produced a BadTag refusal");
}
