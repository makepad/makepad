//! MP4 sample index for range-streaming playback.
//!
//! A remote MP4 can be played without fetching it whole: the `moov` box
//! (a few hundred KB to a few MB, usually at the front) lists every video
//! sample's byte range, decode/presentation time and whether it is a
//! keyframe. With that index a player asks the server for exactly the
//! bytes it is about to decode — and can start at minute forty with one
//! request — while the container's own decoder-facing bits (the H.264
//! SPS/PPS from `avcC`, the NAL length size) come along for the ride.
//!
//! Scope: unfragmented files, one video track (`avc1`/`avc3` = H.264;
//! other codecs are reported, not decoded). Edit lists are ignored beyond
//! normalising timestamps to start at zero. Everything is bounds-checked;
//! a hostile file yields an error, never a panic.

use std::fmt;

/// Time unit used throughout: 100-nanosecond ticks (the platform
/// decoders' unit).
pub const HNS_PER_SECOND: i64 = 10_000_000;

/// Largest `moov` a caller should fetch. A ten-hour tape indexes in a few
/// MB; sixty-four leaves room and still refuses nonsense.
pub const MAX_MOOV_BYTES: u64 = 64 * 1024 * 1024;
/// Sample count ceiling (a day of 60 fps is ~5M).
pub const MAX_SAMPLES: usize = 8_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mp4Error {
    /// The bytes are not an ISO BMFF box structure at all.
    NotMp4,
    /// A required box is missing (`stbl`, `stsz`…).
    Missing(&'static str),
    /// A box is malformed or truncated.
    Malformed(&'static str),
    /// Fragmented (`moof`) files carry their samples elsewhere.
    Fragmented,
    /// No track with a video handler.
    NoVideoTrack,
    TooLarge,
}

impl fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mp4Error::NotMp4 => write!(f, "not an mp4 file"),
            Mp4Error::Missing(what) => write!(f, "mp4 index: missing {what}"),
            Mp4Error::Malformed(what) => write!(f, "mp4 index: malformed {what}"),
            Mp4Error::Fragmented => write!(f, "fragmented mp4 is not supported"),
            Mp4Error::NoVideoTrack => write!(f, "mp4 has no video track"),
            Mp4Error::TooLarge => write!(f, "mp4 index over the size limit"),
        }
    }
}

impl std::error::Error for Mp4Error {}

/// A top-level box header as it sits in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxHeader {
    pub kind: [u8; 4],
    /// Offset of the box (its header) in the file.
    pub offset: u64,
    /// Whole box size, header included. `None` = extends to end of file.
    pub size: Option<u64>,
    /// Header length: 8, or 16 with a 64-bit size.
    pub header_len: u64,
}

impl BoxHeader {
    pub fn kind_str(&self) -> String {
        String::from_utf8_lossy(&self.kind).into_owned()
    }
}

/// Parse one box header at the start of `bytes` (which must hold at least
/// 8 bytes, 16 for a large box). `offset` is where those bytes sit in the
/// file, for the header's own record.
pub fn parse_box_header(bytes: &[u8], offset: u64) -> Result<BoxHeader, Mp4Error> {
    if bytes.len() < 8 {
        return Err(Mp4Error::Malformed("box header"));
    }
    let size32 = be_u32(bytes, 0) as u64;
    let kind = [bytes[4], bytes[5], bytes[6], bytes[7]];
    if !kind.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return Err(Mp4Error::NotMp4);
    }
    match size32 {
        0 => Ok(BoxHeader { kind, offset, size: None, header_len: 8 }),
        1 => {
            if bytes.len() < 16 {
                return Err(Mp4Error::Malformed("large box header"));
            }
            let size = be_u64(bytes, 8);
            if size < 16 {
                return Err(Mp4Error::Malformed("large box size"));
            }
            Ok(BoxHeader { kind, offset, size: Some(size), header_len: 16 })
        }
        s if s < 8 => Err(Mp4Error::Malformed("box size")),
        s => Ok(BoxHeader { kind, offset, size: Some(s), header_len: 8 }),
    }
}

/// The codec of the video track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264 {
        /// Raw SPS / PPS NAL units (no start codes, no length prefixes).
        sps: Vec<Vec<u8>>,
        pps: Vec<Vec<u8>>,
        /// Bytes of the length prefix before each NAL in a sample (1, 2 or 4).
        nal_length_size: u8,
    },
    /// Something this index does not decode; the sample entry's fourcc.
    Other(String),
}

/// One video sample: where it is and when it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub offset: u64,
    pub size: u32,
    /// Decode timestamp; samples are listed in decode order.
    pub dts_100ns: i64,
    /// Presentation timestamp (dts + composition offset), zero-based.
    pub pts_100ns: i64,
    pub sync: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mp4Index {
    pub timescale: u32,
    pub duration_100ns: i64,
    pub width: u32,
    pub height: u32,
    pub codec: VideoCodec,
    /// Video samples in decode order.
    pub samples: Vec<Sample>,
}

impl Mp4Index {
    /// Index of the last keyframe at or before `pts` — where a seek to
    /// `pts` must start decoding. The first sample when nothing precedes.
    pub fn sync_sample_before(&self, pts_100ns: i64) -> usize {
        let mut best = 0;
        for (i, s) in self.samples.iter().enumerate() {
            if s.sync && s.pts_100ns <= pts_100ns {
                best = i;
            }
            if s.dts_100ns > pts_100ns {
                break;
            }
        }
        best
    }

    /// Total bytes of video samples (audio and everything else excluded).
    pub fn video_bytes(&self) -> u64 {
        self.samples.iter().map(|s| s.size as u64).sum()
    }
}

// ------------------------------------------------------------------ bytes

fn be_u16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn be_u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be_i32(b: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be_u64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes([
        b[at], b[at + 1], b[at + 2], b[at + 3], b[at + 4], b[at + 5], b[at + 6], b[at + 7],
    ])
}

fn need(b: &[u8], at: usize, len: usize, what: &'static str) -> Result<(), Mp4Error> {
    if at.checked_add(len).map(|end| end <= b.len()).unwrap_or(false) {
        Ok(())
    } else {
        Err(Mp4Error::Malformed(what))
    }
}

/// Iterate the child boxes inside `data` (the payload of a container box).
fn children(data: &[u8]) -> Result<Vec<([u8; 4], &[u8])>, Mp4Error> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let header = parse_box_header(&data[pos..], pos as u64)?;
        let size = match header.size {
            Some(s) => s as usize,
            None => data.len() - pos,
        };
        if size < header.header_len as usize || pos + size > data.len() {
            return Err(Mp4Error::Malformed("child box size"));
        }
        out.push((header.kind, &data[pos + header.header_len as usize..pos + size]));
        pos += size;
        if out.len() > 4096 {
            return Err(Mp4Error::Malformed("too many child boxes"));
        }
    }
    Ok(out)
}

fn child<'a>(kids: &[([u8; 4], &'a [u8])], kind: &[u8; 4]) -> Option<&'a [u8]> {
    kids.iter().find(|(k, _)| k == kind).map(|(_, d)| *d)
}

/// Full-box version byte + 3 flag bytes, then the payload.
fn full_box<'a>(data: &'a [u8], what: &'static str) -> Result<(u8, &'a [u8]), Mp4Error> {
    need(data, 0, 4, what)?;
    Ok((data[0], &data[4..]))
}

fn to_hns(value: i64, timescale: u32) -> i64 {
    if timescale == 0 {
        return 0;
    }
    ((value as i128 * HNS_PER_SECOND as i128) / timescale as i128) as i64
}

// ------------------------------------------------------------------ moov

/// Parse the payload of a `moov` box (everything after its 8/16-byte
/// header) into the video track's sample index.
pub fn parse_moov(moov: &[u8]) -> Result<Mp4Index, Mp4Error> {
    if moov.len() as u64 > MAX_MOOV_BYTES {
        return Err(Mp4Error::TooLarge);
    }
    let kids = children(moov)?;
    if kids.iter().any(|(k, _)| k == b"mvex") {
        return Err(Mp4Error::Fragmented);
    }
    let mut first_error = None;
    for (kind, trak) in &kids {
        if kind != b"trak" {
            continue;
        }
        match parse_trak(trak) {
            Ok(Some(index)) => return Ok(index),
            Ok(None) => {}
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }
    Err(first_error.unwrap_or(Mp4Error::NoVideoTrack))
}

/// `Ok(None)` for a track that is not video.
fn parse_trak(trak: &[u8]) -> Result<Option<Mp4Index>, Mp4Error> {
    let kids = children(trak)?;
    let mdia = child(&kids, b"mdia").ok_or(Mp4Error::Missing("mdia"))?;
    let mdia_kids = children(mdia)?;
    let hdlr = child(&mdia_kids, b"hdlr").ok_or(Mp4Error::Missing("hdlr"))?;
    let (_, hdlr) = full_box(hdlr, "hdlr")?;
    need(hdlr, 4, 4, "hdlr handler")?;
    if &hdlr[4..8] != b"vide" {
        return Ok(None);
    }
    let mdhd = child(&mdia_kids, b"mdhd").ok_or(Mp4Error::Missing("mdhd"))?;
    let (version, mdhd) = full_box(mdhd, "mdhd")?;
    let (timescale, duration) = if version == 1 {
        need(mdhd, 0, 28, "mdhd v1")?;
        (be_u32(mdhd, 16), be_u64(mdhd, 20) as i64)
    } else {
        need(mdhd, 0, 16, "mdhd v0")?;
        (be_u32(mdhd, 8), be_u32(mdhd, 12) as i64)
    };
    if timescale == 0 {
        return Err(Mp4Error::Malformed("mdhd timescale"));
    }
    let (mut width, mut height) = (0u32, 0u32);
    if let Some(tkhd) = child(&kids, b"tkhd") {
        let (version, tkhd) = full_box(tkhd, "tkhd")?;
        let base = if version == 1 { 32 } else { 20 };
        // reserved(8) layer(2) alt_group(2) volume(2) reserved(2) matrix(36)
        let at = base + 8 + 2 + 2 + 2 + 2 + 36;
        if tkhd.len() >= at + 8 {
            width = be_u32(tkhd, at) >> 16;
            height = be_u32(tkhd, at + 4) >> 16;
        }
    }
    let minf = child(&mdia_kids, b"minf").ok_or(Mp4Error::Missing("minf"))?;
    let minf_kids = children(minf)?;
    let stbl = child(&minf_kids, b"stbl").ok_or(Mp4Error::Missing("stbl"))?;
    let stbl_kids = children(stbl)?;

    // ---- codec
    let stsd = child(&stbl_kids, b"stsd").ok_or(Mp4Error::Missing("stsd"))?;
    let (_, stsd) = full_box(stsd, "stsd")?;
    need(stsd, 0, 4, "stsd count")?;
    let entries = children(&stsd[4..])?;
    let (fourcc, entry) = entries.first().ok_or(Mp4Error::Malformed("stsd entries"))?;
    let codec = if fourcc == b"avc1" || fourcc == b"avc3" {
        // VisualSampleEntry: 78 bytes before the child boxes.
        need(entry, 0, 78, "visual sample entry")?;
        if width == 0 || height == 0 {
            width = be_u16(entry, 24) as u32;
            height = be_u16(entry, 26) as u32;
        }
        let ext = children(&entry[78..])?;
        let avcc = child(&ext, b"avcC").ok_or(Mp4Error::Missing("avcC"))?;
        parse_avcc(avcc)?
    } else {
        VideoCodec::Other(String::from_utf8_lossy(fourcc).into_owned())
    };

    // ---- timing
    let stts = child(&stbl_kids, b"stts").ok_or(Mp4Error::Missing("stts"))?;
    let (_, stts) = full_box(stts, "stts")?;
    need(stts, 0, 4, "stts count")?;
    let stts_count = be_u32(stts, 0) as usize;
    need(stts, 4, stts_count.checked_mul(8).ok_or(Mp4Error::TooLarge)?, "stts entries")?;
    let mut dts: Vec<i64> = Vec::new();
    let mut t: i64 = 0;
    for i in 0..stts_count {
        let count = be_u32(stts, 4 + i * 8) as usize;
        let delta = be_u32(stts, 8 + i * 8) as i64;
        if dts.len() + count > MAX_SAMPLES {
            return Err(Mp4Error::TooLarge);
        }
        for _ in 0..count {
            dts.push(t);
            t += delta;
        }
    }
    let sample_count = dts.len();

    let mut ctts_offsets: Vec<i64> = Vec::new();
    if let Some(ctts) = child(&stbl_kids, b"ctts") {
        let (version, ctts) = full_box(ctts, "ctts")?;
        need(ctts, 0, 4, "ctts count")?;
        let n = be_u32(ctts, 0) as usize;
        need(ctts, 4, n.checked_mul(8).ok_or(Mp4Error::TooLarge)?, "ctts entries")?;
        for i in 0..n {
            let count = be_u32(ctts, 4 + i * 8) as usize;
            let offset = if version == 1 {
                be_i32(ctts, 8 + i * 8) as i64
            } else {
                be_u32(ctts, 8 + i * 8) as i64
            };
            if ctts_offsets.len() + count > MAX_SAMPLES {
                return Err(Mp4Error::TooLarge);
            }
            for _ in 0..count {
                ctts_offsets.push(offset);
            }
        }
    }

    // ---- sizes
    let stsz = child(&stbl_kids, b"stsz").ok_or(Mp4Error::Missing("stsz"))?;
    let (_, stsz) = full_box(stsz, "stsz")?;
    need(stsz, 0, 8, "stsz header")?;
    let uniform = be_u32(stsz, 0);
    let stsz_count = be_u32(stsz, 4) as usize;
    if stsz_count != sample_count {
        return Err(Mp4Error::Malformed("stsz/stts sample count"));
    }
    let mut sizes: Vec<u32> = Vec::with_capacity(sample_count);
    if uniform != 0 {
        sizes.resize(sample_count, uniform);
    } else {
        need(stsz, 8, sample_count.checked_mul(4).ok_or(Mp4Error::TooLarge)?, "stsz entries")?;
        for i in 0..sample_count {
            sizes.push(be_u32(stsz, 8 + i * 4));
        }
    }

    // ---- chunks → offsets
    let stsc = child(&stbl_kids, b"stsc").ok_or(Mp4Error::Missing("stsc"))?;
    let (_, stsc) = full_box(stsc, "stsc")?;
    need(stsc, 0, 4, "stsc count")?;
    let stsc_count = be_u32(stsc, 0) as usize;
    need(stsc, 4, stsc_count.checked_mul(12).ok_or(Mp4Error::TooLarge)?, "stsc entries")?;
    let mut runs: Vec<(u32, u32)> = Vec::with_capacity(stsc_count); // (first_chunk 1-based, samples_per_chunk)
    for i in 0..stsc_count {
        runs.push((be_u32(stsc, 4 + i * 12), be_u32(stsc, 8 + i * 12)));
    }
    let chunk_offsets: Vec<u64> = if let Some(stco) = child(&stbl_kids, b"stco") {
        let (_, stco) = full_box(stco, "stco")?;
        need(stco, 0, 4, "stco count")?;
        let n = be_u32(stco, 0) as usize;
        need(stco, 4, n.checked_mul(4).ok_or(Mp4Error::TooLarge)?, "stco entries")?;
        (0..n).map(|i| be_u32(stco, 4 + i * 4) as u64).collect()
    } else if let Some(co64) = child(&stbl_kids, b"co64") {
        let (_, co64) = full_box(co64, "co64")?;
        need(co64, 0, 4, "co64 count")?;
        let n = be_u32(co64, 0) as usize;
        need(co64, 4, n.checked_mul(8).ok_or(Mp4Error::TooLarge)?, "co64 entries")?;
        (0..n).map(|i| be_u64(co64, 4 + i * 8)).collect()
    } else {
        return Err(Mp4Error::Missing("stco"));
    };
    let mut offsets: Vec<u64> = Vec::with_capacity(sample_count);
    'chunks: for (chunk_ix, chunk_offset) in chunk_offsets.iter().enumerate() {
        let chunk_no = chunk_ix as u32 + 1;
        // The run whose first_chunk is the last one <= this chunk.
        let per_chunk = runs
            .iter()
            .filter(|(first, _)| *first <= chunk_no)
            .last()
            .map(|(_, n)| *n)
            .unwrap_or(0);
        let mut at = *chunk_offset;
        for _ in 0..per_chunk {
            if offsets.len() >= sample_count {
                break 'chunks;
            }
            offsets.push(at);
            at = at.checked_add(sizes[offsets.len() - 1] as u64).ok_or(Mp4Error::Malformed("sample offset"))?;
        }
    }
    if offsets.len() != sample_count {
        return Err(Mp4Error::Malformed("stsc/stco chunk map covers fewer samples than stsz"));
    }

    // ---- keyframes
    let mut sync = vec![true; sample_count];
    if let Some(stss) = child(&stbl_kids, b"stss") {
        let (_, stss) = full_box(stss, "stss")?;
        need(stss, 0, 4, "stss count")?;
        let n = be_u32(stss, 0) as usize;
        need(stss, 4, n.checked_mul(4).ok_or(Mp4Error::TooLarge)?, "stss entries")?;
        sync = vec![false; sample_count];
        for i in 0..n {
            let number = be_u32(stss, 4 + i * 4) as usize;
            if number >= 1 && number <= sample_count {
                sync[number - 1] = true;
            }
        }
    }

    // ---- assemble, zero-based pts (shifted in ticks, so no rounding drift)
    let cts: Vec<i64> = (0..sample_count)
        .map(|i| dts[i] + ctts_offsets.get(i).copied().unwrap_or(0))
        .collect();
    let min_cts = cts.iter().copied().min().unwrap_or(0);
    let mut samples: Vec<Sample> = Vec::with_capacity(sample_count);
    for i in 0..sample_count {
        samples.push(Sample {
            offset: offsets[i],
            size: sizes[i],
            dts_100ns: to_hns(dts[i], timescale),
            pts_100ns: to_hns(cts[i] - min_cts, timescale),
            sync: sync[i],
        });
    }
    let duration_100ns = to_hns(duration, timescale).max(
        samples.last().map(|s| s.pts_100ns).unwrap_or(0),
    );
    Ok(Some(Mp4Index { timescale, duration_100ns, width, height, codec, samples }))
}

fn parse_avcc(avcc: &[u8]) -> Result<VideoCodec, Mp4Error> {
    need(avcc, 0, 6, "avcC")?;
    let nal_length_size = (avcc[4] & 0x03) + 1;
    let sps_count = (avcc[5] & 0x1f) as usize;
    let mut pos = 6;
    let mut sps = Vec::new();
    for _ in 0..sps_count {
        need(avcc, pos, 2, "avcC sps length")?;
        let len = be_u16(avcc, pos) as usize;
        pos += 2;
        need(avcc, pos, len, "avcC sps")?;
        sps.push(avcc[pos..pos + len].to_vec());
        pos += len;
    }
    need(avcc, pos, 1, "avcC pps count")?;
    let pps_count = avcc[pos] as usize;
    pos += 1;
    let mut pps = Vec::new();
    for _ in 0..pps_count {
        need(avcc, pos, 2, "avcC pps length")?;
        let len = be_u16(avcc, pos) as usize;
        pos += 2;
        need(avcc, pos, len, "avcC pps")?;
        pps.push(avcc[pos..pos + len].to_vec());
        pos += len;
    }
    if sps.is_empty() || pps.is_empty() {
        return Err(Mp4Error::Malformed("avcC without sps/pps"));
    }
    Ok(VideoCodec::H264 { sps, pps, nal_length_size })
}

/// Split one sample's payload (NALs with `nal_length_size`-byte length
/// prefixes) into an Annex-B access unit (4-byte start codes).
pub fn sample_to_annex_b(sample: &[u8], nal_length_size: u8, out: &mut Vec<u8>) -> Result<(), Mp4Error> {
    let n = nal_length_size as usize;
    if !matches!(n, 1 | 2 | 4) {
        return Err(Mp4Error::Malformed("nal length size"));
    }
    let mut pos = 0usize;
    while pos + n <= sample.len() {
        let len = match n {
            1 => sample[pos] as usize,
            2 => be_u16(sample, pos) as usize,
            _ => be_u32(sample, pos) as usize,
        };
        pos += n;
        if pos + len > sample.len() {
            return Err(Mp4Error::Malformed("nal length"));
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&sample[pos..pos + len]);
        pos += len;
    }
    Ok(())
}

/// Parameter sets as one Annex-B packet (what a stream decoder wants
/// before the first keyframe, and again after a seek).
pub fn parameter_sets_annex_b(codec: &VideoCodec) -> Vec<u8> {
    let mut out = Vec::new();
    if let VideoCodec::H264 { sps, pps, .. } = codec {
        for nal in sps.iter().chain(pps.iter()) {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        }
    }
    out
}

/// Read an mp4 from any byte source (a file, a range fetcher): walk the
/// top-level boxes for `moov`, refusing fragmented files. `read(offset,
/// len)` returns up to `len` bytes at `offset` (fewer at end of file).
pub fn locate_moov(
    file_size: u64,
    read: &mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>,
) -> Result<BoxHeader, Mp4Error> {
    let mut pos: u64 = 0;
    let mut hops = 0;
    while pos + 8 <= file_size {
        let head = read(pos, 16).map_err(|_| Mp4Error::Malformed("read box header"))?;
        let header = parse_box_header(&head, pos)?;
        if &header.kind == b"moof" {
            return Err(Mp4Error::Fragmented);
        }
        if &header.kind == b"moov" {
            return Ok(header);
        }
        let size = match header.size {
            Some(s) => s,
            None => return Err(Mp4Error::Missing("moov")),
        };
        pos = pos.checked_add(size).ok_or(Mp4Error::Malformed("box size"))?;
        hops += 1;
        if hops > 64 {
            return Err(Mp4Error::Malformed("too many top-level boxes"));
        }
    }
    Err(Mp4Error::Missing("moov"))
}

/// Parse a whole in-memory mp4 (tests, small files): find `moov`, index it.
pub fn parse_file(bytes: &[u8]) -> Result<Mp4Index, Mp4Error> {
    let header = locate_moov(bytes.len() as u64, &mut |offset, len| {
        let start = (offset as usize).min(bytes.len());
        let end = start.saturating_add(len).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    })?;
    let size = header.size.unwrap_or(bytes.len() as u64 - header.offset) as usize;
    let start = header.offset as usize + header.header_len as usize;
    let end = header.offset as usize + size;
    if end > bytes.len() || start > end {
        return Err(Mp4Error::Malformed("moov extent"));
    }
    parse_moov(&bytes[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- a tiny box builder for synthetic files
    fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }
    fn full(kind: &[u8; 4], version: u8, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![version, 0, 0, 0];
        p.extend_from_slice(payload);
        bx(kind, &p)
    }
    fn u32s(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    /// Two chunks: chunk 1 holds samples 0,1 at 1000; chunk 2 holds sample
    /// 2 at 5000. 30 fps at timescale 90000; sample 1 is a B-frame shown
    /// after sample 2 (ctts).
    fn synthetic(codec_box: Vec<u8>) -> Vec<u8> {
        let hdlr = full(b"hdlr", 0, &[&[0u8; 4][..], b"vide", &[0u8; 12], b"\0"].concat());
        let mut mdhd = u32s(&[0, 0, 90000, 9000]); // creation, modification, timescale, duration
        mdhd.extend_from_slice(&[0, 0, 0, 0]);
        let mdhd = full(b"mdhd", 0, &mdhd);
        let mut tkhd = vec![0u8; 20 + 8 + 2 + 2 + 2 + 2 + 36];
        tkhd.extend_from_slice(&(640u32 << 16).to_be_bytes());
        tkhd.extend_from_slice(&(360u32 << 16).to_be_bytes());
        let tkhd = full(b"tkhd", 0, &tkhd);
        let stsd = full(b"stsd", 0, &[u32s(&[1]), codec_box].concat());
        let stts = full(b"stts", 0, &u32s(&[1, 3, 3000]));
        let ctts = full(b"ctts", 0, &u32s(&[3, 1, 3000, 1, 9000, 1, 3000]));
        let stss = full(b"stss", 0, &u32s(&[1, 1]));
        let stsc = full(b"stsc", 0, &u32s(&[2, 1, 2, 1, 2, 1, 1]));
        let stsz = full(b"stsz", 0, &u32s(&[0, 3, 100, 50, 70]));
        let stco = full(b"stco", 0, &u32s(&[2, 1000, 5000]));
        let stbl = bx(b"stbl", &[stsd, stts, ctts, stss, stsc, stsz, stco].concat());
        let minf = bx(b"minf", &stbl);
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        let trak = bx(b"trak", &[tkhd, mdia].concat());
        let audio_trak = {
            let hdlr = full(b"hdlr", 0, &[&[0u8; 4][..], b"soun", &[0u8; 12], b"\0"].concat());
            bx(b"trak", &bx(b"mdia", &hdlr))
        };
        let moov = bx(b"moov", &[audio_trak, trak].concat());
        let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
        let mdat = bx(b"mdat", &[0u8; 8000]);
        [ftyp, moov, mdat].concat()
    }

    fn avc1() -> Vec<u8> {
        let mut entry = vec![0u8; 78];
        entry[24..26].copy_from_slice(&640u16.to_be_bytes());
        entry[26..28].copy_from_slice(&360u16.to_be_bytes());
        let sps = [0x67, 0x64, 0x00, 0x1e, 0xac];
        let pps = [0x68, 0xeb, 0xe3, 0xcb];
        let mut avcc = vec![1, 0x64, 0x00, 0x1e, 0xff, 0xe1];
        avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&sps);
        avcc.push(1);
        avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&pps);
        entry.extend_from_slice(&bx(b"avcC", &avcc));
        bx(b"avc1", &entry)
    }

    #[test]
    fn indexes_a_synthetic_file() {
        let file = synthetic(avc1());
        let index = parse_file(&file).unwrap();
        assert_eq!(index.timescale, 90000);
        assert_eq!((index.width, index.height), (640, 360));
        assert_eq!(index.samples.len(), 3);
        let s = &index.samples;
        assert_eq!((s[0].offset, s[0].size), (1000, 100));
        assert_eq!((s[1].offset, s[1].size), (1100, 50));
        assert_eq!((s[2].offset, s[2].size), (5000, 70));
        // dts 0, 1/30, 2/30; pts shifted so the smallest is zero.
        assert_eq!(s[0].dts_100ns, 0);
        assert_eq!(s[1].dts_100ns, 333_333);
        assert_eq!(s[0].pts_100ns, 0);
        assert_eq!(s[1].pts_100ns, 1_000_000);
        assert_eq!(s[2].pts_100ns, 666_666);
        assert!(s[0].sync && !s[1].sync && !s[2].sync);
        assert_eq!(index.duration_100ns, 1_000_000);
        match &index.codec {
            VideoCodec::H264 { sps, pps, nal_length_size } => {
                assert_eq!(sps.len(), 1);
                assert_eq!(pps.len(), 1);
                assert_eq!(*nal_length_size, 4);
                assert_eq!(sps[0][0], 0x67);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(index.sync_sample_before(900_000), 0);
        assert_eq!(index.video_bytes(), 220);
        let ps = parameter_sets_annex_b(&index.codec);
        assert_eq!(&ps[..5], &[0, 0, 0, 1, 0x67]);
    }

    #[test]
    fn other_codecs_are_named_not_decoded() {
        let mut entry = vec![0u8; 78];
        entry[24..26].copy_from_slice(&320u16.to_be_bytes());
        entry[26..28].copy_from_slice(&240u16.to_be_bytes());
        let file = synthetic(bx(b"mp4v", &entry));
        let index = parse_file(&file).unwrap();
        assert_eq!(index.codec, VideoCodec::Other("mp4v".into()));
    }

    #[test]
    fn nal_splitting() {
        let sample = [0, 0, 0, 2, 0x65, 0xaa, 0, 0, 0, 1, 0x41];
        let mut out = Vec::new();
        sample_to_annex_b(&sample, 4, &mut out).unwrap();
        assert_eq!(out, vec![0, 0, 0, 1, 0x65, 0xaa, 0, 0, 0, 1, 0x41]);
        let mut out = Vec::new();
        assert!(sample_to_annex_b(&[0, 0, 0, 9, 1], 4, &mut out).is_err());
        assert!(sample_to_annex_b(&[1, 0x65], 3, &mut out).is_err());
    }

    #[test]
    fn headers_and_hostile_input() {
        assert!(matches!(parse_box_header(b"\0\0\0\x03abcd", 0), Err(Mp4Error::Malformed(_))));
        assert!(matches!(parse_box_header(b"\0\0\0\x10\x01\x02\x03\x04", 0), Err(Mp4Error::NotMp4)));
        let h = parse_box_header(b"\0\0\0\0mdat", 7).unwrap();
        assert_eq!(h.size, None);
        assert_eq!(h.offset, 7);
        assert!(matches!(parse_file(b"hello world, not an mp4"), Err(_)));
        let mut file = synthetic(avc1());
        // Truncate the moov: must error, never panic.
        let cut = file.len() - 8100;
        file.truncate(cut);
        assert!(parse_file(&file).is_err());
        let frag = [bx(b"ftyp", b"iso5"), bx(b"moof", b""), bx(b"mdat", b"")].concat();
        assert_eq!(parse_file(&frag).unwrap_err(), Mp4Error::Fragmented);
    }

    #[test]
    fn locate_moov_walks_past_mdat() {
        let file = synthetic(avc1());
        let mut reads = 0;
        let header = locate_moov(file.len() as u64, &mut |offset, len| {
            reads += 1;
            let start = offset as usize;
            Ok(file[start..(start + len).min(file.len())].to_vec())
        })
        .unwrap();
        assert_eq!(&header.kind, b"moov");
        assert_eq!(header.offset, 20);
        assert_eq!(reads, 2, "ftyp header, then moov header");
    }
}
