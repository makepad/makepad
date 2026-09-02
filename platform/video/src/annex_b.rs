//! H.264 NAL-unit framing helpers shared by both platform backends
//! (`apple_stream_encoder`/`apple_stream_decoder` produce/consume AVCC —
//! 4-byte big-endian length prefixes — internally via VideoToolbox's
//! `CMSampleBuffer`/`CMBlockBuffer`; the Windows H.264 MFTs speak Annex-B —
//! `00 00 00 01` / `00 00 01` start codes — directly). The wire format
//! (`realtime_wire`'s frame kind 2 in `makepad-asset-ai`) is always
//! Annex-B, so the Apple backend converts at its boundary; the Windows
//! backend does not need to.
//!
//! Plain byte-slicing, no unsafe, no platform dependency — usable and
//! tested on every OS.

/// Splits an Annex-B byte stream into its NAL units (each returned slice
/// EXCLUDES the start code). Accepts a mix of 3-byte (`00 00 01`) and 4-byte
/// (`00 00 00 01`) start codes, as real encoders do (the first start code in
/// a stream is conventionally 4 bytes, subsequent ones are often 3).
pub fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let starts = find_start_codes(data);
    let mut nals = Vec::with_capacity(starts.len());
    for i in 0..starts.len() {
        let (nal_start, _) = starts[i];
        let nal_end = if i + 1 < starts.len() {
            starts[i + 1].0 - starts[i + 1].1
        } else {
            data.len()
        };
        if nal_end > nal_start {
            nals.push(&data[nal_start..nal_end]);
        }
    }
    nals
}

/// Returns `(nal_data_start_offset, start_code_len)` for every start code in
/// `data`, in order.
fn find_start_codes(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 1 {
                out.push((i + 3, 3));
                i += 3;
                continue;
            }
            if i + 3 < data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                out.push((i + 4, 4));
                i += 4;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// H.264 NAL unit type (low 5 bits of the first byte after the start code).
pub fn nal_unit_type(nal: &[u8]) -> u8 {
    nal.first().map(|b| b & 0x1F).unwrap_or(0)
}

pub const NAL_TYPE_SPS: u8 = 7;
pub const NAL_TYPE_PPS: u8 = 8;

/// HEVC NAL unit type: bits 1..6 of the first header byte (H.265 uses a
/// two-byte NAL header, unlike H.264's one).
pub fn hevc_nal_unit_type(nal: &[u8]) -> u8 {
    if nal.is_empty() {
        return 0;
    }
    (nal[0] >> 1) & 0x3f
}

pub const HEVC_NAL_TYPE_VPS: u8 = 32;
pub const HEVC_NAL_TYPE_SPS: u8 = 33;
pub const HEVC_NAL_TYPE_PPS: u8 = 34;
pub const NAL_TYPE_IDR: u8 = 5;

/// Prepends a 4-byte Annex-B start code to `nal` and appends it to `out`.
pub fn push_annex_b_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

/// Converts AVCC (each NAL prefixed by its own 4-byte big-endian length,
/// no start codes — VideoToolbox's native `CMBlockBuffer` layout) into
/// Annex-B (4-byte start codes, no length prefixes).
pub fn avcc_to_annex_b(avcc: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(avcc.len() + 16);
    let mut offset = 0usize;
    while offset + 4 <= avcc.len() {
        let len = u32::from_be_bytes([avcc[offset], avcc[offset + 1], avcc[offset + 2], avcc[offset + 3]]) as usize;
        offset += 4;
        if offset + len > avcc.len() {
            break; // malformed/truncated — stop rather than panic on bad input
        }
        push_annex_b_nal(&mut out, &avcc[offset..offset + len]);
        offset += len;
    }
    out
}

/// Converts Annex-B NAL units into AVCC (4-byte big-endian length prefixes,
/// no start codes) — the format `CMBlockBuffer`/`CMSampleBufferCreateReady`
/// expects for VideoToolbox decode input.
pub fn annex_b_to_avcc(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// The access unit with every SPS's `level_idc` (the third payload byte:
/// profile_idc, constraint flags, level_idc) replaced, re-framed with
/// 4-byte start codes; `None` when it carries no SPS so callers can pass
/// the original bytes through untouched. A decoder that sizes its picture
/// reorder buffer from the level (the Microsoft H.264 MFT) is told a
/// lower level to make it emit pictures one access unit later.
pub fn with_sps_level_idc(access_unit: &[u8], level_idc: u8) -> Option<Vec<u8>> {
    let nals = split_annex_b(access_unit);
    if !nals.iter().any(|nal| nal_unit_type(nal) == NAL_TYPE_SPS && nal.len() > 3) {
        return None;
    }
    let mut out = Vec::with_capacity(access_unit.len() + 4);
    for nal in nals {
        if nal_unit_type(nal) == NAL_TYPE_SPS && nal.len() > 3 {
            let mut sps = nal.to_vec();
            sps[3] = level_idc;
            push_annex_b_nal(&mut out, &sps);
        } else {
            push_annex_b_nal(&mut out, nal);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sps_level_rewrite_touches_only_level_idc() {
        let mut au = Vec::new();
        push_annex_b_nal(&mut au, &[0x67, 0x4d, 0x00, 0x16, 0xab, 0x40]); // SPS, level 2.2
        push_annex_b_nal(&mut au, &[0x68, 0xee, 0x3c, 0x80]); // PPS
        push_annex_b_nal(&mut au, &[0x65, 0x88, 0x84]); // IDR
        let rewritten = with_sps_level_idc(&au, 10).expect("an SPS is present");
        assert_eq!(rewritten.len(), au.len());
        let changed: Vec<usize> = (0..au.len()).filter(|&i| au[i] != rewritten[i]).collect();
        assert_eq!(changed, vec![7], "only level_idc may change");
        assert_eq!(rewritten[7], 10);
        assert!(with_sps_level_idc(&rewritten[4 + 6..], 10).is_none(), "no SPS -> None");
    }

    #[test]
    fn split_annex_b_mixed_start_codes() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 0, 1]); // 4-byte start code
        data.extend_from_slice(&[0x67, 0xAA, 0xBB]); // SPS-ish
        data.extend_from_slice(&[0, 0, 1]); // 3-byte start code
        data.extend_from_slice(&[0x68, 0xCC]); // PPS-ish
        data.extend_from_slice(&[0, 0, 0, 1]);
        data.extend_from_slice(&[0x65, 0x01, 0x02, 0x03]); // IDR-ish

        let nals = split_annex_b(&data);
        assert_eq!(nals.len(), 3);
        assert_eq!(nals[0], &[0x67, 0xAA, 0xBB]);
        assert_eq!(nals[1], &[0x68, 0xCC]);
        assert_eq!(nals[2], &[0x65, 0x01, 0x02, 0x03]);
        assert_eq!(nal_unit_type(nals[0]), NAL_TYPE_SPS);
        assert_eq!(nal_unit_type(nals[1]), NAL_TYPE_PPS);
        assert_eq!(nal_unit_type(nals[2]), NAL_TYPE_IDR);
    }

    #[test]
    fn split_annex_b_empty_and_no_start_code() {
        assert!(split_annex_b(&[]).is_empty());
        assert!(split_annex_b(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn avcc_annex_b_round_trip() {
        let nals: [&[u8]; 2] = [&[0x67, 1, 2, 3, 4], &[0x65, 9, 9]];
        let avcc = annex_b_to_avcc(&nals);
        // 4-byte length + 5 bytes, then 4-byte length + 3 bytes.
        assert_eq!(avcc.len(), 4 + 5 + 4 + 3);
        let annex_b = avcc_to_annex_b(&avcc);
        let round_tripped = split_annex_b(&annex_b);
        assert_eq!(round_tripped.len(), 2);
        assert_eq!(round_tripped[0], nals[0]);
        assert_eq!(round_tripped[1], nals[1]);
    }

    #[test]
    fn avcc_to_annex_b_stops_on_truncated_length() {
        // Claims a 100-byte NAL but only 2 bytes follow — must not panic.
        let mut avcc = (100u32).to_be_bytes().to_vec();
        avcc.extend_from_slice(&[1, 2]);
        let out = avcc_to_annex_b(&avcc);
        assert!(out.is_empty());
    }

    #[test]
    fn push_annex_b_nal_uses_four_byte_start_code() {
        let mut out = Vec::new();
        push_annex_b_nal(&mut out, &[0x65, 1, 2]);
        assert_eq!(&out[..4], &[0, 0, 0, 1]);
        assert_eq!(&out[4..], &[0x65, 1, 2]);
    }
}
