//! An `hvcC` record (HEVCDecoderConfigurationRecord, ISO/IEC 14496-15) built from the parameter
//! sets an encoder hands out as Annex-B, and an Annex-B access unit reframed as an mp4 sample
//! (4-byte big-endian NAL lengths). For encoders that give bitstreams rather than a format
//! description (Android's MediaCodec); the mp4 then reads anywhere [`crate::mp4_first_frame`]
//! and the platform demuxers do.
//!
//! Plain byte-slicing, no platform dependency, tested on every OS.

use crate::annex_b::{hevc_nal_unit_type, split_annex_b};

/// HEVC NAL unit types of the parameter sets.
const VPS: u8 = 32;
const SPS: u8 = 33;
const PPS: u8 = 34;

/// The RBSP of a NAL (emulation-prevention bytes removed), from its first payload byte on.
fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal.iter().skip(2) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// The `hvcC` record for Annex-B parameter sets (VPS, SPS, PPS, in any order), or `None`
/// without an SPS. The general profile/tier/level fields are the SPS's own
/// (profile_tier_level follows its first byte); 4:2:0, 8 bit, 4-byte NAL lengths.
pub fn hvcc_from_annex_b(parameter_sets: &[u8]) -> Option<Vec<u8>> {
    let nals = split_annex_b(parameter_sets);
    let sps = nals.iter().find(|n| hevc_nal_unit_type(n) == SPS)?;
    let ptl = rbsp(sps);
    // sps_video_parameter_set_id(4) max_sub_layers_minus1(3) temporal_id_nesting(1), then the
    // general profile_tier_level: 1 + 4 + 6 + 1 bytes.
    let general = ptl.get(1..13)?;
    let mut out = Vec::with_capacity(64 + parameter_sets.len());
    out.push(1); // configurationVersion
    out.extend_from_slice(&general[..11]); // profile space/tier/idc, compatibility flags, constraint flags
    out.push(general[11]); // general_level_idc
    out.extend_from_slice(&0xF000u16.to_be_bytes()); // min_spatial_segmentation_idc = 0
    out.push(0xFC); // parallelismType = 0
    out.push(0xFC | 1); // chroma_format_idc = 1 (4:2:0)
    out.push(0xF8); // bit_depth_luma_minus8 = 0
    out.push(0xF8); // bit_depth_chroma_minus8 = 0
    out.extend_from_slice(&0u16.to_be_bytes()); // avgFrameRate
    // constantFrameRate 0, numTemporalLayers 1, temporalIdNested 1, lengthSizeMinusOne 3.
    out.push((1 << 3) | (1 << 2) | 3);
    let arrays: Vec<(u8, Vec<&[u8]>)> = [VPS, SPS, PPS]
        .iter()
        .map(|&t| (t, nals.iter().copied().filter(|n| hevc_nal_unit_type(n) == t).collect::<Vec<_>>()))
        .filter(|(_, sets)| !sets.is_empty())
        .collect();
    out.push(arrays.len() as u8);
    for (t, sets) in arrays {
        out.push(0x80 | t); // array_completeness 1
        out.extend_from_slice(&(sets.len() as u16).to_be_bytes());
        for set in sets {
            out.extend_from_slice(&(set.len() as u16).to_be_bytes());
            out.extend_from_slice(set);
        }
    }
    Some(out)
}

/// An Annex-B access unit as an mp4 sample: every NAL but parameter sets, each behind its
/// 4-byte big-endian length.
pub fn sample_from_annex_b(access_unit: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(access_unit.len() + 16);
    for nal in split_annex_b(access_unit) {
        if nal.is_empty() || matches!(hevc_nal_unit_type(nal), VPS | SPS | PPS) {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real parameter sets of a 64x48 Main-profile intra stream (x265), Annex-B.
    const VPS_NAL: &[u8] = &[0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0x95, 0x98, 0x09];
    const SPS_NAL: &[u8] = &[0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xa0, 0x08, 0x08, 0x06, 0x01, 0xe5, 0x96, 0x56, 0x69, 0x24, 0xca, 0xe0, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x01, 0xe0, 0x80];
    const PPS_NAL: &[u8] = &[0x44, 0x01, 0xc1, 0x72, 0xb4, 0x62, 0x40];

    fn annex_b(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn a_record_carries_the_sps_profile_and_every_set() {
        let record = hvcc_from_annex_b(&annex_b(&[VPS_NAL, SPS_NAL, PPS_NAL])).unwrap();
        assert_eq!(record[0], 1);
        // Main profile (idc 1), compatibility flags 0x60000000, level 60 (2.0).
        assert_eq!(record[1], 0x01);
        assert_eq!(&record[2..6], &[0x60, 0, 0, 0]);
        assert_eq!(record[12], 0x3c);
        assert_eq!(record[21] & 3, 3);
        assert_eq!(record[22], 3);
        // The arrays hold the sets verbatim, VPS first.
        assert_eq!(record[23], 0x80 | 32);
        let len = u16::from_be_bytes([record[26], record[27]]) as usize;
        assert_eq!(&record[28..28 + len], VPS_NAL);
        assert!(hvcc_from_annex_b(&annex_b(&[VPS_NAL, PPS_NAL])).is_none());
    }

    #[test]
    fn a_sample_is_the_access_unit_without_parameter_sets() {
        let slice: &[u8] = &[0x26, 0x01, 0xaf, 0x00, 0x00, 0x03, 0x01];
        let sample = sample_from_annex_b(&annex_b(&[VPS_NAL, SPS_NAL, PPS_NAL, slice]));
        assert_eq!(&sample[..4], &(slice.len() as u32).to_be_bytes());
        assert_eq!(&sample[4..], slice);
    }

    /// A record and sample made here read back through the first-frame demuxer as the same
    /// access unit, at the declared size.
    #[cfg(any(target_vendor = "apple", target_os = "android"))]
    #[test]
    fn a_still_written_from_bitstreams_demuxes_back() {
        let slice: &[u8] = &[0x26, 0x01, 0xaf, 0x11, 0x22];
        let record = hvcc_from_annex_b(&annex_b(&[VPS_NAL, SPS_NAL, PPS_NAL])).unwrap();
        let sample = sample_from_annex_b(&annex_b(&[slice]));
        let mp4 = crate::mp4_single_frame::write_single_frame_mp4(&crate::mp4_single_frame::IntraSample {
            data: &sample,
            config: &record,
            sample_entry: *b"hvc1",
            config_atom: *b"hvcC",
            width: 2048,
            height: 1536,
            fps: 30,
        });
        let au = crate::mp4_first_frame::first_access_unit(&mp4).unwrap();
        assert_eq!((au.width, au.height), (2048, 1536));
        assert_eq!(au.annex_b, annex_b(&[VPS_NAL, SPS_NAL, PPS_NAL, slice]));
    }
}
