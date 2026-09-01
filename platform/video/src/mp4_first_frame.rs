//! Pulls the FIRST video sample out of an in-memory mp4/mov and reframes it
//! as one Annex-B access unit (parameter sets + sample NALs), so a single
//! intra frame can be decoded straight from RAM through the stream decoder —
//! no temp file, no OS demuxer. We own both ends: these files come from
//! [`crate::VideoFileEncoder`], but the parsing sticks to plain ISO-BMFF so
//! any well-formed avc1/hvc1/hev1 file works.
//!
//! Plain byte-slicing, no unsafe, no platform dependency — usable and tested
//! on every OS.

use crate::stream_encoder::StreamVideoCodec;
use crate::VideoFileError;

/// The first video sample of a container, ready for a stream decoder.
pub struct FirstAccessUnit {
    pub codec: StreamVideoCodec,
    /// One Annex-B access unit: VPS/SPS/PPS (as the container's codec config
    /// carried them) followed by every NAL of the first sample.
    pub annex_b: Vec<u8>,
}

fn err(what: &str) -> VideoFileError {
    VideoFileError::new(format!("mp4 first frame: {what}"))
}

/// One ISO-BMFF box: `(fourcc, payload)` slices over the input.
struct BoxIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BoxIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
}

impl<'a> Iterator for BoxIter<'a> {
    type Item = ([u8; 4], &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        if rest.len() < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes(rest[0..4].try_into().unwrap()) as u64;
        let fourcc: [u8; 4] = rest[4..8].try_into().unwrap();
        let (header, size) = match size32 {
            // size==0: box runs to end of enclosing container.
            0 => (8usize, rest.len() as u64),
            1 => {
                if rest.len() < 16 {
                    return None;
                }
                (16usize, u64::from_be_bytes(rest[8..16].try_into().unwrap()))
            }
            n => (8usize, n),
        };
        if size < header as u64 || size > rest.len() as u64 {
            return None;
        }
        let payload = &rest[header..size as usize];
        self.pos += size as usize;
        Some((fourcc, payload))
    }
}

fn find_box<'a>(data: &'a [u8], fourcc: &[u8; 4]) -> Option<&'a [u8]> {
    BoxIter::new(data).find(|(f, _)| f == fourcc).map(|(_, p)| p)
}

/// Full-box payloads start with version(1) + flags(3).
fn full_box(payload: &[u8]) -> Option<(u8, &[u8])> {
    if payload.len() < 4 {
        return None;
    }
    Some((payload[0], &payload[4..]))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()))
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    data.get(at..at + 8).map(|b| u64::from_be_bytes(b.try_into().unwrap()))
}

/// The pieces of the video track's sample table this module needs.
struct VideoTrack<'a> {
    codec: StreamVideoCodec,
    /// avcC / hvcC payload.
    config: &'a [u8],
    first_sample_offset: u64,
    first_sample_size: u32,
}

fn video_track<'a>(moov: &'a [u8]) -> Result<VideoTrack<'a>, VideoFileError> {
    for (fourcc, trak) in BoxIter::new(moov) {
        if &fourcc != b"trak" {
            continue;
        }
        let Some(mdia) = find_box(trak, b"mdia") else { continue };
        // hdlr: version/flags(4) + pre_defined(4) + handler_type(4).
        let is_video = find_box(mdia, b"hdlr")
            .and_then(|h| h.get(8..12))
            .map(|t| t == b"vide")
            .unwrap_or(false);
        if !is_video {
            continue;
        }
        let stbl = find_box(mdia, b"minf")
            .and_then(|minf| find_box(minf, b"stbl"))
            .ok_or_else(|| err("video track has no sample table"))?;

        // stsd → first entry → avc1/hvc1/hev1 → its avcC/hvcC child box.
        let (_, stsd) = full_box(find_box(stbl, b"stsd").ok_or_else(|| err("no stsd"))?)
            .ok_or_else(|| err("short stsd"))?;
        if be32(stsd, 0).unwrap_or(0) == 0 {
            return Err(err("empty stsd"));
        }
        let entries = &stsd[4..];
        let (entry_type, entry) = BoxIter::new(entries).next().ok_or_else(|| err("no stsd entry"))?;
        let codec = match &entry_type {
            b"avc1" | b"avc3" => StreamVideoCodec::H264,
            b"hvc1" | b"hev1" => StreamVideoCodec::Hevc,
            other => {
                return Err(err(&format!(
                    "unsupported sample entry {:?}",
                    String::from_utf8_lossy(other)
                )))
            }
        };
        // VisualSampleEntry: 6 reserved + 2 data_ref_index + 70 bytes of
        // fixed fields before the child boxes.
        let children = entry.get(78..).ok_or_else(|| err("short sample entry"))?;
        let config_fourcc: &[u8; 4] = match codec {
            StreamVideoCodec::H264 => b"avcC",
            StreamVideoCodec::Hevc => b"hvcC",
        };
        let config = find_box(children, config_fourcc)
            .ok_or_else(|| err("no codec configuration record"))?;

        // stsz: sample_size (uniform if non-zero) + count + per-sample sizes.
        let (_, stsz) = full_box(find_box(stbl, b"stsz").ok_or_else(|| err("no stsz"))?)
            .ok_or_else(|| err("short stsz"))?;
        let uniform = be32(stsz, 0).ok_or_else(|| err("short stsz"))?;
        let count = be32(stsz, 4).ok_or_else(|| err("short stsz"))?;
        if count == 0 {
            return Err(err("no samples"));
        }
        let first_sample_size = if uniform != 0 {
            uniform
        } else {
            be32(stsz, 8).ok_or_else(|| err("stsz has no entries"))?
        };

        // First chunk offset == first sample offset (sample 1 leads chunk 1).
        let first_sample_offset = if let Some(stco) = find_box(stbl, b"stco") {
            let (_, p) = full_box(stco).ok_or_else(|| err("short stco"))?;
            if be32(p, 0).unwrap_or(0) == 0 {
                return Err(err("empty stco"));
            }
            be32(p, 4).ok_or_else(|| err("short stco"))? as u64
        } else if let Some(co64) = find_box(stbl, b"co64") {
            let (_, p) = full_box(co64).ok_or_else(|| err("short co64"))?;
            if be32(p, 0).unwrap_or(0) == 0 {
                return Err(err("empty co64"));
            }
            be64(p, 4).ok_or_else(|| err("short co64"))?
        } else {
            return Err(err("no chunk offsets"));
        };

        return Ok(VideoTrack { codec, config, first_sample_offset, first_sample_size });
    }
    Err(err("no video track"))
}

/// Parameter sets and NAL length prefix size out of an avcC record.
fn avcc_params(config: &[u8]) -> Result<(Vec<Vec<u8>>, usize), VideoFileError> {
    // configurationVersion(1) profile(1) compat(1) level(1)
    // lengthSizeMinusOne(1, low 2 bits) numSPS(1, low 5 bits).
    if config.len() < 6 {
        return Err(err("short avcC"));
    }
    let nal_len_size = (config[4] & 0x03) as usize + 1;
    let mut sets = Vec::new();
    let mut at = 6usize;
    let mut counts = [config[5] & 0x1f, 0];
    // After the SPS array comes numPPS(1).
    for pass in 0..2 {
        if pass == 1 {
            counts[1] = *config.get(at).ok_or_else(|| err("short avcC"))?;
            at += 1;
        }
        for _ in 0..counts[pass] {
            let len = config
                .get(at..at + 2)
                .map(|b| u16::from_be_bytes(b.try_into().unwrap()) as usize)
                .ok_or_else(|| err("short avcC set"))?;
            at += 2;
            let set = config.get(at..at + len).ok_or_else(|| err("short avcC set"))?;
            sets.push(set.to_vec());
            at += len;
        }
    }
    Ok((sets, nal_len_size))
}

/// Parameter sets (VPS/SPS/PPS arrays, in record order) and NAL length
/// prefix size out of an hvcC record.
fn hvcc_params(config: &[u8]) -> Result<(Vec<Vec<u8>>, usize), VideoFileError> {
    // 22 fixed bytes; byte 21 low 2 bits = lengthSizeMinusOne; byte 22 =
    // numOfArrays, then arrays of (type(1) numNalus(2) (len(2) nal)*).
    if config.len() < 23 {
        return Err(err("short hvcC"));
    }
    let nal_len_size = (config[21] & 0x03) as usize + 1;
    let num_arrays = config[22] as usize;
    let mut sets = Vec::new();
    let mut at = 23usize;
    for _ in 0..num_arrays {
        let num_nalus = config
            .get(at + 1..at + 3)
            .map(|b| u16::from_be_bytes(b.try_into().unwrap()) as usize)
            .ok_or_else(|| err("short hvcC array"))?;
        at += 3;
        for _ in 0..num_nalus {
            let len = config
                .get(at..at + 2)
                .map(|b| u16::from_be_bytes(b.try_into().unwrap()) as usize)
                .ok_or_else(|| err("short hvcC set"))?;
            at += 2;
            let set = config.get(at..at + len).ok_or_else(|| err("short hvcC set"))?;
            sets.push(set.to_vec());
            at += len;
        }
    }
    Ok((sets, nal_len_size))
}

/// Demuxes the first video sample of `bytes` into one Annex-B access unit.
pub fn first_access_unit(bytes: &[u8]) -> Result<FirstAccessUnit, VideoFileError> {
    let moov = find_box(bytes, b"moov").ok_or_else(|| err("no moov box"))?;
    let track = video_track(moov)?;
    let (sets, nal_len_size) = match track.codec {
        StreamVideoCodec::H264 => avcc_params(track.config)?,
        StreamVideoCodec::Hevc => hvcc_params(track.config)?,
    };
    if sets.is_empty() {
        return Err(err("codec configuration carries no parameter sets"));
    }

    let start = track.first_sample_offset as usize;
    let sample = bytes
        .get(start..start + track.first_sample_size as usize)
        .ok_or_else(|| err("first sample lies outside the file"))?;

    let mut annex_b = Vec::with_capacity(sample.len() + 256);
    for set in &sets {
        crate::annex_b::push_annex_b_nal(&mut annex_b, set);
    }
    // Sample NALs are length-prefixed; reframe with start codes.
    let mut at = 0usize;
    while at < sample.len() {
        if at + nal_len_size > sample.len() {
            return Err(err("truncated NAL length prefix"));
        }
        let mut len = 0usize;
        for &b in &sample[at..at + nal_len_size] {
            len = (len << 8) | b as usize;
        }
        at += nal_len_size;
        let nal = sample.get(at..at + len).ok_or_else(|| err("truncated sample NAL"))?;
        crate::annex_b::push_annex_b_nal(&mut annex_b, nal);
        at += len;
    }
    Ok(FirstAccessUnit { codec: track.codec, annex_b })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box is size + fourcc + payload.
    fn mkbox(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
        out.extend_from_slice(fourcc);
        out.extend_from_slice(payload);
        out
    }

    fn full(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 0];
        out.extend_from_slice(payload);
        out
    }

    /// Builds a minimal hvc1 file: ftyp, mdat with one 2-NAL sample,
    /// moov/trak/mdia/{hdlr,minf/stbl/{stsd,stsz,stco}}.
    fn synthetic_hevc(sample: &[u8], vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
        let ftyp = mkbox(b"ftyp", b"isom\x00\x00\x02\x00isomiso2");
        let mdat = mkbox(b"mdat", sample);
        let sample_offset = (ftyp.len() + 8) as u32;

        let mut hvcc = vec![0u8; 22];
        hvcc[21] = 0x03; // 4-byte NAL lengths
        hvcc.push(3); // three arrays
        for (kind, set) in [(32u8, vps), (33, sps), (34, pps)] {
            hvcc.push(kind);
            hvcc.extend_from_slice(&1u16.to_be_bytes());
            hvcc.extend_from_slice(&(set.len() as u16).to_be_bytes());
            hvcc.extend_from_slice(set);
        }
        let mut entry = vec![0u8; 78];
        entry.extend_from_slice(&mkbox(b"hvcC", &hvcc));
        let mut stsd = full(&[0, 0, 0, 1]);
        stsd.extend_from_slice(&mkbox(b"hvc1", &entry));

        let mut stsz = full(&[]);
        stsz.extend_from_slice(&0u32.to_be_bytes()); // per-sample sizes
        stsz.extend_from_slice(&1u32.to_be_bytes()); // one sample
        stsz.extend_from_slice(&(sample.len() as u32).to_be_bytes());

        let mut stco = full(&[]);
        stco.extend_from_slice(&1u32.to_be_bytes());
        stco.extend_from_slice(&sample_offset.to_be_bytes());

        let mut stbl = mkbox(b"stsd", &stsd);
        stbl.extend_from_slice(&mkbox(b"stsz", &stsz));
        stbl.extend_from_slice(&mkbox(b"stco", &stco));
        let minf = mkbox(b"stbl", &stbl);

        let mut hdlr = full(&[0, 0, 0, 0]);
        hdlr.extend_from_slice(b"vide");
        hdlr.extend_from_slice(&[0u8; 13]);

        let mut mdia = mkbox(b"hdlr", &hdlr);
        mdia.extend_from_slice(&mkbox(b"minf", &minf));
        let trak = mkbox(b"mdia", &mdia);
        let moov = mkbox(b"trak", &trak);

        let mut file = ftyp;
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&mkbox(b"moov", &moov));
        file
    }

    #[test]
    fn hevc_first_sample_reframes_to_annex_b() {
        let vps = [0x40, 0x01, 0xaa];
        let sps = [0x42, 0x01, 0xbb, 0xcc];
        let pps = [0x44, 0x01, 0xdd];
        // One sample of two length-prefixed NALs (IDR_W_RADL type 19 = 0x26).
        let mut sample = Vec::new();
        sample.extend_from_slice(&5u32.to_be_bytes());
        sample.extend_from_slice(&[0x26, 0x01, 1, 2, 3]);
        sample.extend_from_slice(&3u32.to_be_bytes());
        sample.extend_from_slice(&[0x26, 0x01, 4]);

        let file = synthetic_hevc(&sample, &vps, &sps, &pps);
        let au = first_access_unit(&file).unwrap();
        assert!(matches!(au.codec, StreamVideoCodec::Hevc));

        let nals = crate::annex_b::split_annex_b(&au.annex_b);
        assert_eq!(nals.len(), 5);
        assert_eq!(nals[0], &vps);
        assert_eq!(nals[1], &sps);
        assert_eq!(nals[2], &pps);
        assert_eq!(nals[3], &[0x26, 0x01, 1, 2, 3]);
        assert_eq!(nals[4], &[0x26, 0x01, 4]);
        assert_eq!(crate::annex_b::hevc_nal_unit_type(nals[0]), 32);
        assert_eq!(crate::annex_b::hevc_nal_unit_type(nals[3]), 19);
    }

    #[test]
    fn garbage_is_refused_loudly() {
        assert!(first_access_unit(b"not a container").is_err());
        assert!(first_access_unit(&[]).is_err());
        // A real-looking file whose sample points past EOF.
        let file = synthetic_hevc(&[0, 0, 0, 200, 1], &[0x40], &[0x42], &[0x44]);
        assert!(first_access_unit(&file).is_err());
    }
}
