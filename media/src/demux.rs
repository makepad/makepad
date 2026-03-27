//! Minimal MP4/ISOBMFF demuxer for indexed video sample access.
//!
//! Reads the box structure, finds a supported video track, and extracts sample
//! data with timing and byte-offset information. Handles `mdat`-based layouts.

use std::io::{self, Read, Seek, SeekFrom};

/// A single video sample (one AV1 temporal unit / frame).
#[derive(Debug, Clone)]
pub struct Sample {
    /// Byte offset in the file.
    pub offset: u64,
    /// Size in bytes.
    pub size: u32,
    /// Decode timestamp in timescale units.
    pub dts: u64,
    /// Composition timestamp in timescale units.
    pub cts: u64,
    /// Whether this is a sync (key) frame.
    pub is_sync: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp4VideoCodec {
    Av1,
    H264,
}

/// Parsed MP4 video track info.
pub struct Mp4Track {
    pub codec: Mp4VideoCodec,
    pub width: u32,
    pub height: u32,
    pub timescale: u32,
    pub duration: u64,
    pub samples: Vec<Sample>,
}

impl Mp4Track {
    pub fn duration_ms(&self) -> u128 {
        if self.timescale == 0 {
            return 0;
        }
        (self.duration as u128 * 1000) / self.timescale as u128
    }

    /// Get sample PTS in milliseconds.
    pub fn sample_pts_ms(&self, idx: usize) -> u64 {
        if self.timescale == 0 || idx >= self.samples.len() {
            return 0;
        }
        self.samples[idx].cts * 1000 / self.timescale as u64
    }

    /// Return the first sample index whose presentation time is at or after the
    /// requested target. If the target is past the end, returns the final
    /// sample index.
    pub fn sample_index_for_time_ms(&self, target_ms: u64) -> Option<usize> {
        if self.samples.is_empty() {
            return None;
        }
        match self
            .samples
            .binary_search_by_key(&target_ms, |sample| sample.cts * 1000 / self.timescale.max(1) as u64)
        {
            Ok(index) => Some(index),
            Err(index) => Some(index.min(self.samples.len().saturating_sub(1))),
        }
    }

    /// Return a seek-safe sync sample index at or before the target time when
    /// possible. Falls back to the nearest earlier sample, or zero.
    pub fn sync_sample_index_for_time_ms(&self, target_ms: u64) -> Option<usize> {
        let target_index = self.sample_index_for_time_ms(target_ms)?;
        if self.samples[target_index].is_sync {
            return Some(target_index);
        }
        self.samples[..=target_index]
            .iter()
            .rposition(|sample| sample.is_sync)
            .or(Some(0))
    }

    /// Byte offset of the preferred sync sample at or before the target time.
    pub fn byte_offset_for_time_ms(&self, target_ms: u64) -> Option<u64> {
        let index = self.sync_sample_index_for_time_ms(target_ms)?;
        Some(self.samples[index].offset)
    }

    /// Byte range for the preferred sync sample at or before the target time.
    pub fn byte_range_for_time_ms(&self, target_ms: u64) -> Option<(u64, u64)> {
        let index = self.sync_sample_index_for_time_ms(target_ms)?;
        let sample = &self.samples[index];
        Some((sample.offset, sample.offset + sample.size as u64))
    }

    /// Read sample data from the reader.
    pub fn read_sample<R: Read + Seek>(&self, reader: &mut R, idx: usize) -> io::Result<Vec<u8>> {
        let s = &self.samples[idx];
        reader.seek(SeekFrom::Start(s.offset))?;
        let mut buf = vec![0u8; s.size as usize];
        reader.read_exact(&mut buf)?;
        Ok(buf)
    }
}

/// Parse an MP4 file and extract the first AV1 video track.
pub fn parse_mp4<R: Read + Seek>(reader: &mut R) -> io::Result<Mp4Track> {
    parse_video_mp4_with_codecs(reader, &[Mp4VideoCodec::Av1])
}

/// Parse an MP4 file and extract the first supported video track.
pub fn parse_supported_video_mp4<R: Read + Seek>(reader: &mut R) -> io::Result<Mp4Track> {
    parse_video_mp4_with_codecs(reader, &[Mp4VideoCodec::Av1, Mp4VideoCodec::H264])
}

fn parse_video_mp4_with_codecs<R: Read + Seek>(reader: &mut R, codecs: &[Mp4VideoCodec]) -> io::Result<Mp4Track> {
    let file_size = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;

    let mut moov_data: Option<Vec<u8>> = None;

    // First pass: find the moov box
    let mut pos: u64 = 0;
    while pos < file_size {
        reader.seek(SeekFrom::Start(pos))?;
        let (box_size, box_type) = read_box_header(reader)?;
        if box_size == 0 {
            break;
        }

        if &box_type == b"moov" {
            let data_size = (box_size - 8) as usize;
            let mut data = vec![0u8; data_size];
            reader.read_exact(&mut data)?;
            moov_data = Some(data);
            break;
        }

        pos += box_size;
    }

    let moov =
        moov_data.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no moov box"))?;

    // Parse moov to find video trak
    parse_moov(&moov, reader, codecs)
}

fn read_box_header<R: Read>(reader: &mut R) -> io::Result<(u64, [u8; 4])> {
    let mut buf = [0u8; 8];
    if reader.read_exact(&mut buf).is_err() {
        return Ok((0, [0; 4]));
    }
    let size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
    let box_type = [buf[4], buf[5], buf[6], buf[7]];

    if size == 1 {
        // 64-bit extended size
        let mut ext = [0u8; 8];
        reader.read_exact(&mut ext)?;
        let ext_size = u64::from_be_bytes(ext);
        return Ok((ext_size, box_type));
    }

    Ok((size, box_type))
}

fn read_u16(d: &[u8], o: &mut usize) -> u16 {
    let v = u16::from_be_bytes([d[*o], d[*o + 1]]);
    *o += 2;
    v
}

fn read_u32(d: &[u8], o: &mut usize) -> u32 {
    let v = u32::from_be_bytes([d[*o], d[*o + 1], d[*o + 2], d[*o + 3]]);
    *o += 4;
    v
}

fn read_u64(d: &[u8], o: &mut usize) -> u64 {
    let v = u64::from_be_bytes([
        d[*o],
        d[*o + 1],
        d[*o + 2],
        d[*o + 3],
        d[*o + 4],
        d[*o + 5],
        d[*o + 6],
        d[*o + 7],
    ]);
    *o += 8;
    v
}

fn read_i32(d: &[u8], o: &mut usize) -> i32 {
    let v = i32::from_be_bytes([d[*o], d[*o + 1], d[*o + 2], d[*o + 3]]);
    *o += 4;
    v
}

fn box_type_at(data: &[u8], offset: usize) -> [u8; 4] {
    [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]
}

/// Iterator over child boxes in a container box.
struct BoxIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BoxIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        BoxIter { data, pos: 0 }
    }
}

impl<'a> Iterator for BoxIter<'a> {
    type Item = ([u8; 4], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + 8 > self.data.len() {
            return None;
        }
        let size = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]) as usize;
        if size < 8 || self.pos + size > self.data.len() {
            return None;
        }
        let bt = box_type_at(self.data, self.pos + 4);
        let body = &self.data[self.pos + 8..self.pos + size];
        self.pos += size;
        Some((bt, body))
    }
}

fn parse_moov<R: Read + Seek>(
    moov: &[u8],
    _reader: &mut R,
    codecs: &[Mp4VideoCodec],
) -> io::Result<Mp4Track> {
    for (bt, body) in BoxIter::new(moov) {
        if &bt == b"trak" {
            if let Ok(Some(track)) = parse_trak(body, codecs) {
                return Ok(track);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "no supported video track",
    ))
}

fn parse_trak(trak: &[u8], codecs: &[Mp4VideoCodec]) -> io::Result<Option<Mp4Track>> {
    let mut tkhd_width = 0u32;
    let mut tkhd_height = 0u32;
    let mut mdia_data: Option<&[u8]> = None;

    for (bt, body) in BoxIter::new(trak) {
        match &bt {
            b"tkhd" => {
                if body.len() >= 84 {
                    let version = body[0];
                    let off = if version == 1 { 78 } else { 70 };
                    if off + 8 <= body.len() {
                        let mut o = off;
                        tkhd_width = read_u32(body, &mut o) >> 16;
                        tkhd_height = read_u32(body, &mut o) >> 16;
                    }
                }
            }
            b"mdia" => {
                mdia_data = Some(body);
            }
            _ => {}
        }
    }

    let mdia = match mdia_data {
        Some(d) => d,
        None => return Ok(None),
    };

    let mut timescale = 0u32;
    let mut duration = 0u64;
    let mut minf_data: Option<&[u8]> = None;
    let mut is_video = false;

    for (bt, body) in BoxIter::new(mdia) {
        match &bt {
            b"mdhd" => {
                if body.len() >= 20 {
                    let version = body[0];
                    let mut o = 4;
                    if version == 1 {
                        o += 16;
                        timescale = read_u32(body, &mut o);
                        duration = read_u64(body, &mut o);
                    } else {
                        o += 8;
                        timescale = read_u32(body, &mut o);
                        duration = read_u32(body, &mut o) as u64;
                    }
                }
            }
            b"hdlr" => {
                if body.len() >= 12 {
                    let handler = box_type_at(body, 8);
                    is_video = &handler == b"vide";
                }
            }
            b"minf" => {
                minf_data = Some(body);
            }
            _ => {}
        }
    }

    if !is_video {
        return Ok(None);
    }

    let minf = match minf_data {
        Some(d) => d,
        None => return Ok(None),
    };

    let mut stbl_data: Option<&[u8]> = None;
    for (bt, body) in BoxIter::new(minf) {
        if &bt == b"stbl" {
            stbl_data = Some(body);
            break;
        }
    }

    let stbl = match stbl_data {
        Some(d) => d,
        None => return Ok(None),
    };

    // Check stsd for supported codec
    let mut codec: Option<Mp4VideoCodec> = None;
    let mut stsd_width = 0u32;
    let mut stsd_height = 0u32;

    // Parse sample table
    let mut stts_entries: Vec<(u32, u32)> = Vec::new(); // (count, delta)
    let mut stsc_entries: Vec<(u32, u32, u32)> = Vec::new(); // (first_chunk, samples_per_chunk, _)
    let mut stsz_sizes: Vec<u32> = Vec::new();
    let mut stco_offsets: Vec<u64> = Vec::new();
    let mut ctts_entries: Vec<(u32, i32)> = Vec::new(); // (count, offset)
    let mut stss_sync: Vec<u32> = Vec::new(); // 1-based sample numbers

    for (bt, body) in BoxIter::new(stbl) {
        match &bt {
            b"stsd" => {
                // version(1) + flags(3) + entry_count(4) = 8 bytes header
                if body.len() > 8 {
                    let entries = &body[8..];
                    // First entry: size(4) + type(4) + reserved(6) + data_ref_idx(2) = 16
                    if entries.len() >= 16 {
                        let sample_codec = box_type_at(entries, 4);
                        let parsed_codec = if &sample_codec == b"av01" {
                            Some(Mp4VideoCodec::Av1)
                        } else if &sample_codec == b"avc1" || &sample_codec == b"avc3" {
                            Some(Mp4VideoCodec::H264)
                        } else {
                            None
                        };
                        if let Some(parsed_codec) = parsed_codec.filter(|codec| codecs.contains(codec)) {
                            codec = Some(parsed_codec);
                            // Visual sample entry: after 16 bytes base, skip 16 bytes reserved
                            if entries.len() >= 40 {
                                let mut o = 32; // 16 base + 16 reserved
                                stsd_width = read_u16(entries, &mut o) as u32;
                                stsd_height = read_u16(entries, &mut o) as u32;
                            }
                        }
                    }
                }
            }
            b"stts" => {
                if body.len() >= 8 {
                    let mut o = 4; // skip version+flags
                    let count = read_u32(body, &mut o) as usize;
                    for _ in 0..count {
                        if o + 8 > body.len() {
                            break;
                        }
                        let sc = read_u32(body, &mut o);
                        let delta = read_u32(body, &mut o);
                        stts_entries.push((sc, delta));
                    }
                }
            }
            b"stsc" => {
                if body.len() >= 8 {
                    let mut o = 4;
                    let count = read_u32(body, &mut o) as usize;
                    for _ in 0..count {
                        if o + 12 > body.len() {
                            break;
                        }
                        let first = read_u32(body, &mut o);
                        let spc = read_u32(body, &mut o);
                        let sdi = read_u32(body, &mut o);
                        stsc_entries.push((first, spc, sdi));
                    }
                }
            }
            b"stsz" => {
                if body.len() >= 12 {
                    let mut o = 4;
                    let default_size = read_u32(body, &mut o);
                    let count = read_u32(body, &mut o) as usize;
                    if default_size == 0 {
                        for _ in 0..count {
                            if o + 4 > body.len() {
                                break;
                            }
                            stsz_sizes.push(read_u32(body, &mut o));
                        }
                    } else {
                        stsz_sizes = vec![default_size; count];
                    }
                }
            }
            b"stco" => {
                if body.len() >= 8 {
                    let mut o = 4;
                    let count = read_u32(body, &mut o) as usize;
                    for _ in 0..count {
                        if o + 4 > body.len() {
                            break;
                        }
                        stco_offsets.push(read_u32(body, &mut o) as u64);
                    }
                }
            }
            b"co64" => {
                if body.len() >= 8 {
                    let mut o = 4;
                    let count = read_u32(body, &mut o) as usize;
                    for _ in 0..count {
                        if o + 8 > body.len() {
                            break;
                        }
                        stco_offsets.push(read_u64(body, &mut o));
                    }
                }
            }
            b"ctts" => {
                if body.len() >= 8 {
                    let version = body[0];
                    let mut o = 4;
                    let count = read_u32(body, &mut o) as usize;
                    for _ in 0..count {
                        if o + 8 > body.len() {
                            break;
                        }
                        let sc = read_u32(body, &mut o);
                        let offset = if version == 0 {
                            read_u32(body, &mut o) as i32
                        } else {
                            read_i32(body, &mut o)
                        };
                        ctts_entries.push((sc, offset));
                    }
                }
            }
            b"stss" => {
                if body.len() >= 8 {
                    let mut o = 4;
                    let count = read_u32(body, &mut o) as usize;
                    for _ in 0..count {
                        if o + 4 > body.len() {
                            break;
                        }
                        stss_sync.push(read_u32(body, &mut o));
                    }
                }
            }
            _ => {}
        }
    }

    let Some(codec) = codec else {
        return Ok(None);
    };
    if stsz_sizes.is_empty() {
        return Ok(None);
    }

    let width = if stsd_width > 0 {
        stsd_width
    } else {
        tkhd_width
    };
    let height = if stsd_height > 0 {
        stsd_height
    } else {
        tkhd_height
    };

    // Build sample list
    let num_samples = stsz_sizes.len();
    let mut samples = Vec::with_capacity(num_samples);

    // Compute offsets from stsc + stco
    let offsets = compute_sample_offsets(&stsc_entries, &stco_offsets, &stsz_sizes, num_samples);

    // Compute DTS from stts
    let mut dts_values = Vec::with_capacity(num_samples);
    let mut dts: u64 = 0;
    let mut sample_idx = 0;
    for &(count, delta) in &stts_entries {
        for _ in 0..count {
            if sample_idx >= num_samples {
                break;
            }
            dts_values.push(dts);
            dts += delta as u64;
            sample_idx += 1;
        }
    }
    while dts_values.len() < num_samples {
        dts_values.push(dts);
    }

    // Compute CTS from ctts
    let mut cts_offsets = vec![0i32; num_samples];
    if !ctts_entries.is_empty() {
        let mut idx = 0;
        for &(count, offset) in &ctts_entries {
            for _ in 0..count {
                if idx >= num_samples {
                    break;
                }
                cts_offsets[idx] = offset;
                idx += 1;
            }
        }
    }

    // Build sync sample set
    let has_stss = !stss_sync.is_empty();

    for i in 0..num_samples {
        let dts_val = dts_values[i];
        let cts_val = (dts_val as i64 + cts_offsets[i] as i64).max(0) as u64;
        let is_sync = if has_stss {
            stss_sync.contains(&(i as u32 + 1))
        } else {
            true // no stss means all samples are sync
        };

        samples.push(Sample {
            offset: offsets[i],
            size: stsz_sizes[i],
            dts: dts_val,
            cts: cts_val,
            is_sync,
        });
    }

    Ok(Some(Mp4Track {
        codec,
        width,
        height,
        timescale,
        duration,
        samples,
    }))
}

fn compute_sample_offsets(
    stsc: &[(u32, u32, u32)],
    stco: &[u64],
    stsz: &[u32],
    num_samples: usize,
) -> Vec<u64> {
    let mut offsets = vec![0u64; num_samples];
    let mut sample_idx = 0usize;

    for chunk_idx in 0..stco.len() {
        let chunk_1based = chunk_idx as u32 + 1;

        // Find how many samples in this chunk
        let mut spc = 1u32;
        for i in 0..stsc.len() {
            if stsc[i].0 <= chunk_1based {
                spc = stsc[i].1;
            }
        }

        let mut offset = stco[chunk_idx];
        for _ in 0..spc {
            if sample_idx >= num_samples {
                return offsets;
            }
            offsets[sample_idx] = offset;
            offset += stsz[sample_idx] as u64;
            sample_idx += 1;
        }
    }

    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Mp4Track {
        Mp4Track {
            codec: Mp4VideoCodec::H264,
            width: 160,
            height: 90,
            timescale: 1000,
            duration: 4000,
            samples: vec![
                Sample { offset: 100, size: 10, dts: 0, cts: 0, is_sync: true },
                Sample { offset: 110, size: 10, dts: 1000, cts: 1000, is_sync: false },
                Sample { offset: 120, size: 10, dts: 2000, cts: 2000, is_sync: true },
                Sample { offset: 130, size: 10, dts: 3000, cts: 3000, is_sync: false },
            ],
        }
    }

    #[test]
    fn sample_index_for_time_ms_clamps_to_end() {
        let track = track();
        assert_eq!(track.sample_index_for_time_ms(0), Some(0));
        assert_eq!(track.sample_index_for_time_ms(1500), Some(2));
        assert_eq!(track.sample_index_for_time_ms(9999), Some(3));
    }

    #[test]
    fn sync_sample_index_for_time_ms_walks_back_to_keyframe() {
        let track = track();
        assert_eq!(track.sync_sample_index_for_time_ms(0), Some(0));
        assert_eq!(track.sync_sample_index_for_time_ms(1000), Some(0));
        assert_eq!(track.sync_sample_index_for_time_ms(2500), Some(2));
        assert_eq!(track.byte_offset_for_time_ms(2500), Some(120));
        assert_eq!(track.byte_range_for_time_ms(2500), Some((120, 130)));
    }
}
