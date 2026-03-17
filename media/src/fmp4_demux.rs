//! Incremental fragmented MP4 (CMAF/fMP4) demuxer.
//!
//! Accepts push-based input: init segment (`ftyp`+`moov`) followed by media
//! segments (`moof`+`mdat`). Emits parsed init metadata and per-track media
//! samples. Parsing stays below playback policy.

use std::collections::HashMap;

/// Codec detected from a sample entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FMp4Codec {
    Av1,
    H264,
    Aac,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FMp4VideoTrackInfo {
    pub track_id: u32,
    pub codec: FMp4Codec,
    pub width: u32,
    pub height: u32,
    pub timescale: u32,
    pub codec_config: Vec<u8>,
    pub nal_length_size: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FMp4AudioTrackInfo {
    pub track_id: u32,
    pub codec: FMp4Codec,
    pub timescale: u32,
    pub sample_rate: u32,
    pub channels: u16,
    pub codec_config: Vec<u8>,
}

/// Init segment metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FMp4Init {
    /// Movie duration in milliseconds. Zero means live or unknown.
    pub duration_ms: u128,
    pub video_tracks: Vec<FMp4VideoTrackInfo>,
    pub audio_tracks: Vec<FMp4AudioTrackInfo>,
}

/// A single media sample extracted from a media segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FMp4Sample {
    pub track_id: u32,
    pub codec: FMp4Codec,
    /// Decode timestamp in track timescale units.
    pub dts: u64,
    /// Presentation timestamp in track timescale units.
    pub pts: u64,
    /// Duration in track timescale units.
    pub duration: u32,
    pub is_sync: bool,
    pub data: Vec<u8>,
}

impl FMp4Init {
    pub fn primary_video_track(&self) -> Option<&FMp4VideoTrackInfo> {
        self.video_tracks.first()
    }

    pub fn primary_audio_track(&self) -> Option<&FMp4AudioTrackInfo> {
        self.audio_tracks.first()
    }

    pub fn track_timescale(&self, track_id: u32) -> Option<u32> {
        self.video_tracks
            .iter()
            .find(|track| track.track_id == track_id)
            .map(|track| track.timescale)
            .or_else(|| {
                self.audio_tracks
                    .iter()
                    .find(|track| track.track_id == track_id)
                    .map(|track| track.timescale)
            })
    }

    pub fn track_codec(&self, track_id: u32) -> Option<FMp4Codec> {
        self.video_tracks
            .iter()
            .find(|track| track.track_id == track_id)
            .map(|track| track.codec)
            .or_else(|| {
                self.audio_tracks
                    .iter()
                    .find(|track| track.track_id == track_id)
                    .map(|track| track.codec)
            })
    }

    pub fn track_ts_to_ms(&self, track_id: u32, ts: u64) -> u64 {
        let Some(timescale) = self.track_timescale(track_id) else {
            return 0;
        };
        if timescale == 0 {
            return 0;
        }
        ts.saturating_mul(1000) / timescale as u64
    }
}

/// Events emitted by the incremental demuxer.
#[derive(Debug)]
pub enum FMp4Event {
    InitSegment(FMp4Init),
    MediaSamples(Vec<FMp4Sample>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentSeekPoint {
    pub start_ms: u64,
    pub moof_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentSeekIndex {
    pub init_end: u64,
    pub points: Vec<FragmentSeekPoint>,
}

impl FragmentSeekIndex {
    pub fn moof_offset_for_time_ms(&self, target_ms: u64) -> Option<u64> {
        self.points
            .iter()
            .rfind(|point| point.start_ms <= target_ms)
            .or_else(|| self.points.first())
            .map(|point| point.moof_offset)
    }
}

pub struct IncrementalDemuxer {
    buf: Vec<u8>,
    state: DemuxState,
    init: Option<FMp4Init>,
    trex_defaults: HashMap<u32, TrexDefaults>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemuxState {
    WaitingInit,
    WaitingMedia,
}

#[derive(Clone, Copy, Debug, Default)]
struct TrexDefaults {
    duration: u32,
    size: u32,
    flags: u32,
}

#[derive(Clone)]
enum ParsedTrack {
    Video(FMp4VideoTrackInfo),
    Audio(FMp4AudioTrackInfo),
}

pub fn index_fragmented_mp4(data: &[u8]) -> Option<FragmentSeekIndex> {
    let init_end = find_init_end(data)?;
    let mut demuxer = IncrementalDemuxer::new();
    demuxer.push_data(&data[..init_end]);
    let init = demuxer.init()?.clone();
    let mut points = Vec::new();

    let mut pos = init_end;
    while pos + 8 <= data.len() {
        let mut scan = pos;
        let mut moof_body: Option<&[u8]> = None;
        let mut moof_start = 0usize;
        let mut moof_box_size = 0usize;
        let media_segment = loop {
            if scan + 8 > data.len() {
                return Some(FragmentSeekIndex {
                    init_end: init_end as u64,
                    points,
                });
            }
            let (box_size, box_type, header_size) = read_box_header_at(data, scan);
            if box_size == 0 || box_size as usize > 256 * 1024 * 1024 {
                return Some(FragmentSeekIndex {
                    init_end: init_end as u64,
                    points,
                });
            }
            let end = scan + box_size as usize;
            if end > data.len() {
                return Some(FragmentSeekIndex {
                    init_end: init_end as u64,
                    points,
                });
            }

            match &box_type {
                b"styp" | b"free" | b"skip" | b"sidx" => scan = end,
                b"moof" => {
                    moof_body = Some(&data[scan + header_size..end]);
                    moof_start = scan;
                    moof_box_size = end - scan;
                    scan = end;
                }
                b"mdat" => {
                    if moof_body.is_none() {
                        scan = end;
                        continue;
                    }
                    break Some((&data[scan + header_size..end], scan + header_size - moof_start, end));
                }
                _ => scan = end,
            }
        };

        let Some(moof_body) = moof_body else {
            break;
        };
        let Some((mdat_body, mdat_body_offset_from_moof, segment_end)) = media_segment else {
            break;
        };

        let samples = demuxer.parse_moof_mdat(
            moof_body,
            mdat_body,
            moof_box_size,
            mdat_body_offset_from_moof,
        );
        let start_ms = samples
            .iter()
            .filter(|sample| matches!(sample.codec, FMp4Codec::Av1 | FMp4Codec::H264))
            .map(|sample| init.track_ts_to_ms(sample.track_id, sample.pts))
            .min();
        if let Some(start_ms) = start_ms {
            points.push(FragmentSeekPoint {
                start_ms,
                moof_offset: moof_start as u64,
            });
        }
        pos = segment_end;
    }

    Some(FragmentSeekIndex {
        init_end: init_end as u64,
        points,
    })
}

impl IncrementalDemuxer {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            state: DemuxState::WaitingInit,
            init: None,
            trex_defaults: HashMap::new(),
        }
    }

    pub fn init(&self) -> Option<&FMp4Init> {
        self.init.as_ref()
    }

    pub fn push_data(&mut self, data: &[u8]) -> Vec<FMp4Event> {
        self.buf.extend_from_slice(data);
        let mut events = Vec::new();

        loop {
            let progressed = match self.state {
                DemuxState::WaitingInit => self.try_parse_init(&mut events),
                DemuxState::WaitingMedia => self.try_parse_media_segment(&mut events),
            };
            if !progressed {
                break;
            }
        }

        events
    }

    fn try_parse_init(&mut self, events: &mut Vec<FMp4Event>) -> bool {
        let mut pos = 0;
        loop {
            if pos + 8 > self.buf.len() {
                return false;
            }
            let (box_size, box_type, header_size) = read_box_header_at(&self.buf, pos);
            if box_size == 0 || box_size as usize > 256 * 1024 * 1024 {
                return false;
            }
            let end = pos + box_size as usize;
            if end > self.buf.len() {
                return false;
            }

            match &box_type {
                b"ftyp" | b"styp" | b"free" | b"skip" => pos = end,
                b"moov" => {
                    let moov_body = self.buf[pos + header_size..end].to_vec();
                    let Some(init) = self.parse_moov(&moov_body) else {
                        return false;
                    };
                    self.init = Some(init.clone());
                    self.state = DemuxState::WaitingMedia;
                    self.buf.drain(..end);
                    events.push(FMp4Event::InitSegment(init));
                    return true;
                }
                _ => pos = end,
            }
        }
    }

    fn try_parse_media_segment(&mut self, events: &mut Vec<FMp4Event>) -> bool {
        let mut pos = 0;
        let mut moof_body: Option<&[u8]> = None;
        let mut moof_start = 0usize;
        let mut moof_box_size = 0usize;

        let media_segment = loop {
            if pos + 8 > self.buf.len() {
                return false;
            }
            let (box_size, box_type, header_size) = read_box_header_at(&self.buf, pos);
            if box_size == 0 || box_size as usize > 256 * 1024 * 1024 {
                return false;
            }
            let end = pos + box_size as usize;
            if end > self.buf.len() {
                return false;
            }

            match &box_type {
                b"styp" | b"free" | b"skip" | b"sidx" => pos = end,
                b"moof" => {
                    moof_body = Some(&self.buf[pos + header_size..end]);
                    moof_start = pos;
                    moof_box_size = end - pos;
                    pos = end;
                }
                b"mdat" => {
                    if moof_body.is_none() {
                        pos = end;
                        continue;
                    }
                    break Some((
                        &self.buf[pos + header_size..end],
                        pos + header_size - moof_start,
                        end,
                    ));
                }
                _ => pos = end,
            }
        };

        let Some(moof_body) = moof_body else {
            return false;
        };
        let Some((mdat_body, mdat_body_offset_from_moof, segment_end)) = media_segment else {
            return false;
        };

        let samples = self.parse_moof_mdat(
            moof_body,
            mdat_body,
            moof_box_size,
            mdat_body_offset_from_moof,
        );
        self.buf.drain(..segment_end);
        if !samples.is_empty() {
            events.push(FMp4Event::MediaSamples(samples));
        }
        true
    }

    fn parse_moov(&mut self, moov: &[u8]) -> Option<FMp4Init> {
        self.trex_defaults.clear();
        let mut mvhd_timescale = 0u32;
        let mut mvhd_duration = 0u64;
        let mut video_tracks = Vec::new();
        let mut audio_tracks = Vec::new();

        for (bt, body) in BoxIter::new(moov) {
            match &bt {
                b"mvhd" => {
                    if body.len() >= 20 {
                        let version = body[0];
                        let mut o = 4;
                        if version == 1 {
                            o += 16;
                            mvhd_timescale = read_u32(body, &mut o);
                            mvhd_duration = read_u64(body, &mut o);
                        } else {
                            o += 8;
                            mvhd_timescale = read_u32(body, &mut o);
                            mvhd_duration = read_u32(body, &mut o) as u64;
                        }
                    }
                }
                b"mvex" => self.parse_mvex(body),
                b"trak" => match self.parse_trak(body) {
                    Some(ParsedTrack::Video(track)) => video_tracks.push(track),
                    Some(ParsedTrack::Audio(track)) => audio_tracks.push(track),
                    None => {}
                },
                _ => {}
            }
        }

        if video_tracks.is_empty() && audio_tracks.is_empty() {
            return None;
        }

        let duration_ms = if mvhd_timescale == 0 {
            0
        } else {
            mvhd_duration as u128 * 1000 / mvhd_timescale as u128
        };

        Some(FMp4Init {
            duration_ms,
            video_tracks,
            audio_tracks,
        })
    }

    fn parse_mvex(&mut self, mvex: &[u8]) {
        for (bt, body) in BoxIter::new(mvex) {
            if &bt != b"trex" || body.len() < 24 {
                continue;
            }
            let mut o = 4;
            let track_id = read_u32(body, &mut o);
            let _sample_description_index = read_u32(body, &mut o);
            let defaults = TrexDefaults {
                duration: read_u32(body, &mut o),
                size: read_u32(body, &mut o),
                flags: read_u32(body, &mut o),
            };
            self.trex_defaults.insert(track_id, defaults);
        }
    }

    fn parse_trak(&self, trak: &[u8]) -> Option<ParsedTrack> {
        let mut tkhd_width = 0u32;
        let mut tkhd_height = 0u32;
        let mut track_id = 0u32;
        let mut mdia: Option<&[u8]> = None;

        for (bt, body) in BoxIter::new(trak) {
            match &bt {
                b"tkhd" => {
                    if body.len() >= 4 {
                        let version = body[0];
                        let mut o = 4;
                        if version == 1 {
                            o += 16;
                            track_id = read_u32(body, &mut o);
                            o += 4;
                            o += 8;
                        } else {
                            o += 8;
                            track_id = read_u32(body, &mut o);
                            o += 4;
                            o += 4;
                        }
                        o += 8 + 2 + 2 + 2 + 2 + 36;
                        if o + 8 <= body.len() {
                            tkhd_width = read_u32(body, &mut o) >> 16;
                            tkhd_height = read_u32(body, &mut o) >> 16;
                        }
                    }
                }
                b"mdia" => mdia = Some(body),
                _ => {}
            }
        }

        let mdia = mdia?;
        let mut timescale = 0u32;
        let mut handler: Option<[u8; 4]> = None;
        let mut minf: Option<&[u8]> = None;

        for (bt, body) in BoxIter::new(mdia) {
            match &bt {
                b"mdhd" => {
                    if body.len() >= 20 {
                        let version = body[0];
                        let mut o = 4;
                        if version == 1 {
                            o += 16;
                            timescale = read_u32(body, &mut o);
                        } else {
                            o += 8;
                            timescale = read_u32(body, &mut o);
                        }
                    }
                }
                b"hdlr" => {
                    if body.len() >= 12 {
                        handler = Some(box_type_at(body, 8));
                    }
                }
                b"minf" => minf = Some(body),
                _ => {}
            }
        }

        let minf = minf?;
        let stbl = BoxIter::new(minf)
            .find_map(|(bt, body)| if &bt == b"stbl" { Some(body) } else { None })?;

        match handler? {
            [b'v', b'i', b'd', b'e'] => {
                let (codec, stsd_width, stsd_height, codec_config, nal_length_size) =
                    self.parse_video_stsd(stbl)?;
                Some(ParsedTrack::Video(FMp4VideoTrackInfo {
                    track_id,
                    codec,
                    width: if stsd_width > 0 { stsd_width } else { tkhd_width },
                    height: if stsd_height > 0 { stsd_height } else { tkhd_height },
                    timescale,
                    codec_config,
                    nal_length_size,
                }))
            }
            [b's', b'o', b'u', b'n'] => {
                let (codec, sample_rate, channels, codec_config) = self.parse_audio_stsd(stbl)?;
                Some(ParsedTrack::Audio(FMp4AudioTrackInfo {
                    track_id,
                    codec,
                    timescale,
                    sample_rate,
                    channels,
                    codec_config,
                }))
            }
            _ => None,
        }
    }

    fn parse_video_stsd(&self, stbl: &[u8]) -> Option<(FMp4Codec, u32, u32, Vec<u8>, u8)> {
        for (bt, body) in BoxIter::new(stbl) {
            if &bt != b"stsd" || body.len() <= 8 {
                continue;
            }
            let entries = &body[8..];
            if entries.len() < 16 {
                continue;
            }
            let entry_size = read_u32_at(entries, 0) as usize;
            if entry_size < 16 || entry_size > entries.len() {
                continue;
            }
            let codec_type = box_type_at(entries, 4);
            let entry_body = &entries[..entry_size];

            match &codec_type {
                b"av01" => {
                    let (w, h) = parse_visual_dimensions(entry_body);
                    let mut config = Vec::new();
                    if entry_body.len() > 86 {
                        for (sbt, sbody) in BoxIter::new(&entry_body[86..]) {
                            if &sbt == b"av1C" {
                                config = sbody.to_vec();
                                break;
                            }
                        }
                    }
                    return Some((FMp4Codec::Av1, w, h, config, 0));
                }
                b"avc1" | b"avc3" => {
                    let (w, h) = parse_visual_dimensions(entry_body);
                    let mut config = Vec::new();
                    let mut nal_len_size = 4u8;
                    if entry_body.len() > 86 {
                        for (sbt, sbody) in BoxIter::new(&entry_body[86..]) {
                            if &sbt == b"avcC" {
                                config = sbody.to_vec();
                                if config.len() >= 5 {
                                    nal_len_size = (config[4] & 0x03) + 1;
                                }
                                break;
                            }
                        }
                    }
                    return Some((FMp4Codec::H264, w, h, config, nal_len_size));
                }
                _ => {}
            }
        }
        None
    }

    fn parse_audio_stsd(&self, stbl: &[u8]) -> Option<(FMp4Codec, u32, u16, Vec<u8>)> {
        for (bt, body) in BoxIter::new(stbl) {
            if &bt != b"stsd" || body.len() <= 8 {
                continue;
            }
            let entries = &body[8..];
            if entries.len() < 36 {
                continue;
            }
            let entry_size = read_u32_at(entries, 0) as usize;
            if entry_size < 36 || entry_size > entries.len() {
                continue;
            }
            let codec_type = box_type_at(entries, 4);
            if &codec_type != b"mp4a" {
                continue;
            }
            let entry_body = &entries[..entry_size];
            let mut o = 24;
            let channels = read_u16(entry_body, &mut o);
            let _sample_size = read_u16(entry_body, &mut o);
            o += 2 + 2;
            let sample_rate_fixed = read_u32(entry_body, &mut o);
            let sample_rate = sample_rate_fixed >> 16;
            let mut config = Vec::new();
            if entry_body.len() > 36 {
                for (sbt, sbody) in BoxIter::new(&entry_body[36..]) {
                    if &sbt == b"esds" {
                        config = parse_esds_audio_specific_config(sbody)?;
                        break;
                    }
                }
            }
            return Some((FMp4Codec::Aac, sample_rate, channels, config));
        }
        None
    }

    fn parse_moof_mdat(
        &self,
        moof: &[u8],
        mdat: &[u8],
        moof_box_size: usize,
        mdat_body_offset_from_moof: usize,
    ) -> Vec<FMp4Sample> {
        let mut samples = Vec::new();
        for (bt, body) in BoxIter::new(moof) {
            if &bt != b"traf" {
                continue;
            }
            self.parse_traf(body, mdat, moof_box_size, mdat_body_offset_from_moof, &mut samples);
        }
        samples
    }

    fn parse_traf(
        &self,
        traf: &[u8],
        mdat: &[u8],
        moof_box_size: usize,
        mdat_body_offset_from_moof: usize,
        samples: &mut Vec<FMp4Sample>,
    ) {
        let Some(init) = self.init.as_ref() else {
            return;
        };

        let mut base_decode_time = 0u64;
        let mut track_id = 0u32;
        let mut defaults = TrexDefaults::default();
        let mut trun_entries = Vec::new();
        let mut data_offset: Option<i32> = None;

        for (bt, body) in BoxIter::new(traf) {
            match &bt {
                b"tfhd" => {
                    if body.len() < 8 {
                        continue;
                    }
                    let mut o = 0;
                    let version_flags = read_u32(body, &mut o);
                    let flags = version_flags & 0x00FF_FFFF;
                    track_id = read_u32(body, &mut o);
                    defaults = self.trex_defaults.get(&track_id).copied().unwrap_or_default();

                    if flags & 0x000001 != 0 && o + 8 <= body.len() {
                        let _base_data_offset = read_u64(body, &mut o);
                    }
                    if flags & 0x000002 != 0 && o + 4 <= body.len() {
                        let _sample_desc_index = read_u32(body, &mut o);
                    }
                    if flags & 0x000008 != 0 && o + 4 <= body.len() {
                        defaults.duration = read_u32(body, &mut o);
                    }
                    if flags & 0x000010 != 0 && o + 4 <= body.len() {
                        defaults.size = read_u32(body, &mut o);
                    }
                    if flags & 0x000020 != 0 && o + 4 <= body.len() {
                        defaults.flags = read_u32(body, &mut o);
                    }
                }
                b"tfdt" => {
                    if body.len() >= 8 {
                        let version = body[0];
                        let mut o = 4;
                        base_decode_time = if version == 1 {
                            read_u64(body, &mut o)
                        } else {
                            read_u32(body, &mut o) as u64
                        };
                    }
                }
                b"trun" => {
                    if body.len() < 8 {
                        continue;
                    }
                    let mut o = 0;
                    let version_flags = read_u32(body, &mut o);
                    let flags = version_flags & 0x00FF_FFFF;
                    let version = (version_flags >> 24) as u8;
                    let sample_count = read_u32(body, &mut o);

                    if flags & 0x000001 != 0 && o + 4 <= body.len() {
                        data_offset = Some(read_i32(body, &mut o));
                    }
                    let first_sample_flags = if flags & 0x000004 != 0 && o + 4 <= body.len() {
                        Some(read_u32(body, &mut o))
                    } else {
                        None
                    };

                    for i in 0..sample_count as usize {
                        let duration = if flags & 0x000100 != 0 && o + 4 <= body.len() {
                            read_u32(body, &mut o)
                        } else {
                            defaults.duration
                        };
                        let size = if flags & 0x000200 != 0 && o + 4 <= body.len() {
                            read_u32(body, &mut o)
                        } else {
                            defaults.size
                        };
                        let sample_flags = if flags & 0x000400 != 0 && o + 4 <= body.len() {
                            read_u32(body, &mut o)
                        } else if i == 0 {
                            first_sample_flags.unwrap_or(defaults.flags)
                        } else {
                            defaults.flags
                        };
                        let cts_offset = if flags & 0x000800 != 0 && o + 4 <= body.len() {
                            if version == 0 {
                                read_u32(body, &mut o) as i32
                            } else {
                                read_i32(body, &mut o)
                            }
                        } else {
                            0
                        };
                        trun_entries.push(TrunEntry {
                            duration,
                            size,
                            is_sync: (sample_flags & 0x0001_0000) == 0,
                            cts_offset,
                        });
                    }
                }
                _ => {}
            }
        }

        if track_id == 0 || trun_entries.is_empty() {
            return;
        }
        let Some(codec) = init.track_codec(track_id) else {
            return;
        };

        let mut offset_in_mdat = data_offset
            .map(|offset| {
                offset
                    .saturating_sub(mdat_body_offset_from_moof as i32)
                    .max(0) as usize
            })
            .unwrap_or(0);
        if offset_in_mdat >= mdat.len() && moof_box_size + 8 == mdat_body_offset_from_moof {
            offset_in_mdat = 0;
        }

        let mut dts = base_decode_time;
        for entry in trun_entries {
            let pts = (dts as i64 + entry.cts_offset as i64).max(0) as u64;
            let end = offset_in_mdat.saturating_add(entry.size as usize);
            if end <= mdat.len() {
                samples.push(FMp4Sample {
                    track_id,
                    codec,
                    dts,
                    pts,
                    duration: entry.duration,
                    is_sync: entry.is_sync,
                    data: mdat[offset_in_mdat..end].to_vec(),
                });
            }
            offset_in_mdat = end;
            dts = dts.saturating_add(entry.duration as u64);
        }
    }
}

#[derive(Debug)]
struct TrunEntry {
    duration: u32,
    size: u32,
    is_sync: bool,
    cts_offset: i32,
}

fn parse_visual_dimensions(entry_body: &[u8]) -> (u32, u32) {
    if entry_body.len() < 40 {
        return (0, 0);
    }
    let mut o = 32;
    (
        read_u16(entry_body, &mut o) as u32,
        read_u16(entry_body, &mut o) as u32,
    )
}

fn parse_esds_audio_specific_config(esds: &[u8]) -> Option<Vec<u8>> {
    if esds.len() < 6 {
        return None;
    }
    for start in 4..esds.len().saturating_sub(2) {
        if esds[start] != 0x05 {
            continue;
        }
        let mut o = start + 1;
        let size = read_descriptor_len(esds, &mut o)?;
        if o + size <= esds.len() {
            return Some(esds[o..o + size].to_vec());
        }
    }
    None
}

fn find_init_end(data: &[u8]) -> Option<usize> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let (box_size, box_type, _header_size) = read_box_header_at(data, pos);
        if box_size == 0 || box_size as usize > 256 * 1024 * 1024 {
            return None;
        }
        let end = pos + box_size as usize;
        if end > data.len() {
            return None;
        }
        match &box_type {
            b"ftyp" | b"styp" | b"free" | b"skip" => pos = end,
            b"moov" => return Some(end),
            _ => pos = end,
        }
    }
    None
}

fn read_descriptor_len(data: &[u8], offset: &mut usize) -> Option<usize> {
    let mut size = 0usize;
    for _ in 0..4 {
        if *offset >= data.len() {
            return None;
        }
        let byte = data[*offset];
        *offset += 1;
        size = (size << 7) | (byte & 0x7f) as usize;
        if byte & 0x80 == 0 {
            return Some(size);
        }
    }
    None
}

fn read_box_header_at(data: &[u8], offset: usize) -> (u64, [u8; 4], usize) {
    let size = u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]) as u64;
    let bt = [
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ];
    if size == 1 && offset + 16 <= data.len() {
        let ext = u64::from_be_bytes([
            data[offset + 8],
            data[offset + 9],
            data[offset + 10],
            data[offset + 11],
            data[offset + 12],
            data[offset + 13],
            data[offset + 14],
            data[offset + 15],
        ]);
        return (ext, bt, 16);
    }
    (size, bt, 8)
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

fn read_u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_box(out: &mut Vec<u8>, box_type: &[u8; 4], body: &[u8]) {
        let size = (body.len() + 8) as u32;
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(box_type);
        out.extend_from_slice(body);
    }

    fn make_av1_init_segment() -> Vec<u8> {
        let mut buf = Vec::new();
        write_box(&mut buf, b"ftyp", b"isom\0\0\x02\0isomdash");

        let mut moov = Vec::new();
        let mut mvhd = vec![0u8; 104];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        write_box(&mut moov, b"mvhd", &mvhd);

        let mut trak = Vec::new();
        let mut tkhd = vec![0u8; 84];
        tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
        tkhd[76..80].copy_from_slice(&(320u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(240u32 << 16).to_be_bytes());
        write_box(&mut trak, b"tkhd", &tkhd);

        let mut mdia = Vec::new();
        let mut mdhd = vec![0u8; 24];
        mdhd[12..16].copy_from_slice(&30_000u32.to_be_bytes());
        write_box(&mut mdia, b"mdhd", &mdhd);
        let mut hdlr = vec![0u8; 24];
        hdlr[8..12].copy_from_slice(b"vide");
        write_box(&mut mdia, b"hdlr", &hdlr);

        let mut minf = Vec::new();
        let mut stbl = Vec::new();
        let mut stsd_body = vec![0u8; 8];
        stsd_body[7] = 1;
        let mut av01 = vec![0u8; 86];
        av01[4..8].copy_from_slice(b"av01");
        av01[32..34].copy_from_slice(&320u16.to_be_bytes());
        av01[34..36].copy_from_slice(&240u16.to_be_bytes());
        let av01_size = av01.len() as u32;
        av01[0..4].copy_from_slice(&av01_size.to_be_bytes());
        stsd_body.extend_from_slice(&av01);
        write_box(&mut stbl, b"stsd", &stsd_body);
        write_box(&mut minf, b"stbl", &stbl);
        write_box(&mut mdia, b"minf", &minf);
        write_box(&mut trak, b"mdia", &mdia);
        write_box(&mut moov, b"trak", &trak);

        let mut mvex = Vec::new();
        let mut trex = vec![0u8; 24];
        trex[7] = 1;
        trex[11] = 1;
        trex[12..16].copy_from_slice(&1000u32.to_be_bytes());
        write_box(&mut mvex, b"trex", &trex);
        write_box(&mut moov, b"mvex", &mvex);
        write_box(&mut buf, b"moov", &moov);
        buf
    }

    fn make_audio_init_segment() -> Vec<u8> {
        let mut buf = Vec::new();
        write_box(&mut buf, b"ftyp", b"isom\0\0\x02\0isomdash");

        let mut moov = Vec::new();
        let mut mvhd = vec![0u8; 104];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        write_box(&mut moov, b"mvhd", &mvhd);

        let mut trak = Vec::new();
        let mut tkhd = vec![0u8; 84];
        tkhd[12..16].copy_from_slice(&2u32.to_be_bytes());
        write_box(&mut trak, b"tkhd", &tkhd);

        let mut mdia = Vec::new();
        let mut mdhd = vec![0u8; 24];
        mdhd[12..16].copy_from_slice(&48_000u32.to_be_bytes());
        write_box(&mut mdia, b"mdhd", &mdhd);
        let mut hdlr = vec![0u8; 24];
        hdlr[8..12].copy_from_slice(b"soun");
        write_box(&mut mdia, b"hdlr", &hdlr);

        let mut minf = Vec::new();
        let mut stbl = Vec::new();
        let mut stsd_body = vec![0u8; 8];
        stsd_body[7] = 1;

        let mut mp4a = vec![0u8; 36];
        mp4a[4..8].copy_from_slice(b"mp4a");
        mp4a[24..26].copy_from_slice(&2u16.to_be_bytes());
        mp4a[26..28].copy_from_slice(&16u16.to_be_bytes());
        mp4a[32..36].copy_from_slice(&(48_000u32 << 16).to_be_bytes());

        let mut esds = vec![0, 0, 0, 0];
        esds.extend_from_slice(&[
            0x03, 0x19, 0x00, 0x02, 0x00, 0x04, 0x11, 0x40, 0x15, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x05, 0x02, 0x11, 0x90, 0x06, 0x01, 0x02,
        ]);
        write_box(&mut mp4a, b"esds", &esds);
        let mp4a_size = mp4a.len() as u32;
        mp4a[0..4].copy_from_slice(&mp4a_size.to_be_bytes());
        stsd_body.extend_from_slice(&mp4a);
        write_box(&mut stbl, b"stsd", &stsd_body);
        write_box(&mut minf, b"stbl", &stbl);
        write_box(&mut mdia, b"minf", &minf);
        write_box(&mut trak, b"mdia", &mdia);
        write_box(&mut moov, b"trak", &trak);

        let mut mvex = Vec::new();
        let mut trex = vec![0u8; 24];
        trex[7] = 2;
        trex[11] = 1;
        trex[12..16].copy_from_slice(&1024u32.to_be_bytes());
        write_box(&mut mvex, b"trex", &trex);
        write_box(&mut moov, b"mvex", &mvex);
        write_box(&mut buf, b"moov", &moov);
        buf
    }

    fn make_media_segment(track_id: u32, base_decode_time: u64, sample_data: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut moof = Vec::new();
        write_box(&mut moof, b"mfhd", &[0, 0, 0, 0, 0, 0, 0, 1]);
        let mut traf = Vec::new();
        let mut tfhd = vec![0, 0x02, 0, 0, 0, 0, 0, 0];
        tfhd[4..8].copy_from_slice(&track_id.to_be_bytes());
        write_box(&mut traf, b"tfhd", &tfhd);
        let mut tfdt = vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        tfdt[4..12].copy_from_slice(&base_decode_time.to_be_bytes());
        write_box(&mut traf, b"tfdt", &tfdt);

        let sample_count = sample_data.len() as u32;
        let flags: u32 = 0x000001 | 0x000100 | 0x000200 | 0x000400;
        let mut trun_body = Vec::new();
        trun_body.extend_from_slice(&flags.to_be_bytes());
        trun_body.extend_from_slice(&sample_count.to_be_bytes());
        trun_body.extend_from_slice(&[0, 0, 0, 0]);
        for (i, sample) in sample_data.iter().enumerate() {
            trun_body.extend_from_slice(&1000u32.to_be_bytes());
            trun_body.extend_from_slice(&(sample.len() as u32).to_be_bytes());
            let sample_flags: u32 = if i == 0 { 0 } else { 0x0001_0000 };
            trun_body.extend_from_slice(&sample_flags.to_be_bytes());
        }
        write_box(&mut traf, b"trun", &trun_body);
        write_box(&mut moof, b"traf", &traf);
        let moof_size = moof.len() + 8;
        write_box(&mut buf, b"moof", &moof);

        let mut mdat = Vec::new();
        for sample in sample_data {
            mdat.extend_from_slice(sample);
        }
        write_box(&mut buf, b"mdat", &mdat);

        let data_offset = (moof_size + 8) as i32;
        let do_bytes = data_offset.to_be_bytes();
        buf[84..88].copy_from_slice(&do_bytes);
        buf
    }

    #[test]
    fn parse_init_segment_video() {
        let init = make_av1_init_segment();
        let mut demuxer = IncrementalDemuxer::new();
        let events = demuxer.push_data(&init);
        match &events[0] {
            FMp4Event::InitSegment(init) => {
                assert_eq!(init.video_tracks.len(), 1);
                assert_eq!(init.video_tracks[0].codec, FMp4Codec::Av1);
                assert_eq!(init.video_tracks[0].width, 320);
                assert_eq!(init.video_tracks[0].height, 240);
                assert_eq!(init.video_tracks[0].timescale, 30_000);
            }
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn parse_init_segment_audio() {
        let init = make_audio_init_segment();
        let mut demuxer = IncrementalDemuxer::new();
        let events = demuxer.push_data(&init);
        match &events[0] {
            FMp4Event::InitSegment(init) => {
                assert_eq!(init.audio_tracks.len(), 1);
                assert_eq!(init.audio_tracks[0].codec, FMp4Codec::Aac);
                assert_eq!(init.audio_tracks[0].sample_rate, 48_000);
                assert_eq!(init.audio_tracks[0].channels, 2);
                assert_eq!(init.audio_tracks[0].codec_config, vec![0x11, 0x90]);
            }
            _ => panic!("expected init"),
        }
    }

    #[test]
    fn parse_media_segment() {
        let init = make_av1_init_segment();
        let sample1 = vec![0xAAu8; 100];
        let sample2 = vec![0xBBu8; 50];
        let segment = make_media_segment(1, 0, &[&sample1, &sample2]);
        let mut demuxer = IncrementalDemuxer::new();
        assert!(matches!(demuxer.push_data(&init)[0], FMp4Event::InitSegment(_)));

        let events = demuxer.push_data(&segment);
        match &events[0] {
            FMp4Event::MediaSamples(samples) => {
                assert_eq!(samples.len(), 2);
                assert_eq!(samples[0].track_id, 1);
                assert_eq!(samples[0].codec, FMp4Codec::Av1);
                assert_eq!(samples[0].dts, 0);
                assert!(samples[0].is_sync);
                assert_eq!(samples[1].dts, 1000);
                assert!(!samples[1].is_sync);
            }
            _ => panic!("expected samples"),
        }
    }

    #[test]
    fn incremental_push() {
        let init = make_av1_init_segment();
        let mut demuxer = IncrementalDemuxer::new();
        let mid = init.len() / 2;
        assert!(demuxer.push_data(&init[..mid]).is_empty());
        let events = demuxer.push_data(&init[mid..]);
        assert!(matches!(&events[0], FMp4Event::InitSegment(_)));
    }
}
