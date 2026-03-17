//! Direct source-backed playback machine.
//!
//! This path owns random-access source reads, byte-region caching, seek mapping,
//! and demux/decode progression for resolved media assets. It is distinct from
//! the append-shaped MSE path.

use crate::{
    fmp4_demux::{FMp4Event, FragmentSeekIndex, index_fragmented_mp4},
    mp4_decode::Mp4DecodeSession,
    MseDecodedAudioFrame, MseDecodedFrame, PlaybackPrepared,
};
use std::{collections::BTreeMap, sync::Arc};

pub type DirectByteSourceReader = Arc<dyn Fn(u64, usize) -> Result<Vec<u8>, String> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct DirectMediaPlaybackConfig {
    pub content_length: u64,
    pub read_chunk_size: usize,
    pub startup_buffer_target_secs: f64,
    pub steady_buffer_target_secs: f64,
    pub max_reads_per_pump: usize,
    pub cache_max_bytes: usize,
}

impl DirectMediaPlaybackConfig {
    pub fn new(content_length: u64) -> Self {
        Self {
            content_length,
            read_chunk_size: 256 * 1024,
            startup_buffer_target_secs: 2.0,
            steady_buffer_target_secs: 8.0,
            max_reads_per_pump: 4,
            cache_max_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DirectMediaCursor {
    pub current_time_ms: u64,
    pub current_byte_offset: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DirectPumpOutput {
    pub prepared: Option<PlaybackPrepared>,
    pub audio_frames: Vec<MseDecodedAudioFrame>,
    pub video_frames: Vec<MseDecodedFrame>,
    pub buffered_ranges: Vec<(f64, f64)>,
    pub reached_eos: bool,
}

#[derive(Clone)]
struct CachedRegion {
    data: Vec<u8>,
    last_used_tick: u64,
}

pub struct ByteRegionCache {
    max_bytes: usize,
    total_bytes: usize,
    regions: BTreeMap<u64, CachedRegion>,
    lru_tick: u64,
}

impl ByteRegionCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            total_bytes: 0,
            regions: BTreeMap::new(),
            lru_tick: 0,
        }
    }

    pub fn get_range(&mut self, start: u64, len: usize) -> Option<Vec<u8>> {
        if len == 0 {
            return Some(Vec::new());
        }
        let end = start.checked_add(len as u64)?;
        let (&region_start, region) = self.regions.range(..=start).next_back()?;
        let region_end = region_start.saturating_add(region.data.len() as u64);
        if start < region_start || end > region_end {
            return None;
        }
        self.lru_tick = self.lru_tick.saturating_add(1);
        let tick = self.lru_tick;
        if let Some(region) = self.regions.get_mut(&region_start) {
            region.last_used_tick = tick;
            let from = (start - region_start) as usize;
            let to = from + len;
            return Some(region.data[from..to].to_vec());
        }
        None
    }

    pub fn insert(&mut self, start: u64, data: Vec<u8>, protected_span: Option<(u64, u64)>) {
        if data.is_empty() {
            return;
        }
        self.lru_tick = self.lru_tick.saturating_add(1);
        let tick = self.lru_tick;
        let mut merged_start = start;
        let mut merged_end = start.saturating_add(data.len() as u64);
        let mut merged_data = data;
        let mut merged_tick = tick;

        let overlaps: Vec<u64> = self
            .regions
            .range(..=merged_end)
            .filter_map(|(&existing_start, existing)| {
                let existing_end = existing_start.saturating_add(existing.data.len() as u64);
                if existing_end < merged_start || existing_start > merged_end {
                    None
                } else {
                    Some(existing_start)
                }
            })
            .collect();

        for existing_start in overlaps {
            let existing = self.regions.remove(&existing_start).expect("cache region missing");
            self.total_bytes = self.total_bytes.saturating_sub(existing.data.len());
            let existing_end = existing_start.saturating_add(existing.data.len() as u64);
            let combined_start = merged_start.min(existing_start);
            let combined_end = merged_end.max(existing_end);
            let mut combined = vec![0u8; (combined_end - combined_start) as usize];

            let existing_offset = (existing_start - combined_start) as usize;
            combined[existing_offset..existing_offset + existing.data.len()]
                .copy_from_slice(&existing.data);

            let merged_offset = (merged_start - combined_start) as usize;
            combined[merged_offset..merged_offset + merged_data.len()].copy_from_slice(&merged_data);

            merged_start = combined_start;
            merged_end = combined_end;
            merged_data = combined;
            merged_tick = merged_tick.max(existing.last_used_tick);
        }

        self.total_bytes = self.total_bytes.saturating_add(merged_data.len());
        self.regions.insert(
            merged_start,
            CachedRegion {
                data: merged_data,
                last_used_tick: merged_tick,
            },
        );
        self.evict_cold_regions(protected_span);
    }

    fn evict_cold_regions(&mut self, protected_span: Option<(u64, u64)>) {
        while self.total_bytes > self.max_bytes {
            let candidate = self
                .regions
                .iter()
                .filter(|(start, region)| {
                    let region_start = **start;
                    let region_end = region_start.saturating_add(region.data.len() as u64);
                    !overlaps(protected_span, (region_start, region_end))
                })
                .min_by_key(|(_, region)| region.last_used_tick)
                .map(|(start, _)| *start);
            let Some(candidate) = candidate else {
                break;
            };
            if let Some(region) = self.regions.remove(&candidate) {
                self.total_bytes = self.total_bytes.saturating_sub(region.data.len());
            }
        }
    }
}

fn overlaps(a: Option<(u64, u64)>, b: (u64, u64)) -> bool {
    let Some((a_start, a_end)) = a else {
        return false;
    };
    a_start < b.1 && b.0 < a_end
}

#[derive(Clone, Debug)]
pub enum DirectMediaIndex {
    FragmentedMp4(FragmentedMp4Index),
}

#[derive(Clone, Debug)]
pub struct FragmentedMp4Index {
    pub init_prefix: Vec<u8>,
    pub seek_index: FragmentSeekIndex,
}

impl DirectMediaIndex {
    fn initial_offset(&self) -> u64 {
        match self {
            Self::FragmentedMp4(index) => index.seek_index.init_end,
        }
    }

    fn seek_offset_for_time_ms(&self, target_ms: u64) -> u64 {
        match self {
            Self::FragmentedMp4(index) => index
                .seek_index
                .moof_offset_for_time_ms(target_ms)
                .unwrap_or(index.seek_index.init_end),
        }
    }

    fn init_prefix(&self) -> &[u8] {
        match self {
            Self::FragmentedMp4(index) => &index.init_prefix,
        }
    }
}

pub struct DirectMediaMachine {
    reader: DirectByteSourceReader,
    config: DirectMediaPlaybackConfig,
    cache: ByteRegionCache,
    index: DirectMediaIndex,
    cursor: DirectMediaCursor,
    demuxer: crate::fmp4_demux::IncrementalDemuxer,
    decoder: Mp4DecodeSession,
    prepared: Option<PlaybackPrepared>,
    audio_output_info: Option<(u32, u8)>,
    reached_eos: bool,
}

impl DirectMediaMachine {
    pub fn open(
        reader: DirectByteSourceReader,
        config: DirectMediaPlaybackConfig,
        mime: &str,
    ) -> Result<Self, String> {
        let base = mime.split(';').next().unwrap_or("").trim();
        if !matches!(base, "video/mp4" | "video/x-m4v" | "audio/mp4" | "audio/x-m4a") {
            return Err(format!("unsupported direct container: {base}"));
        }

        let probe_len = usize::try_from(config.content_length)
            .map_err(|_| "content length too large for direct playback".to_string())?;
        let probe_bytes = reader(0, probe_len)?;
        if probe_bytes.len() as u64 != config.content_length {
            return Err("direct playback probe returned truncated data".to_string());
        }

        let seek_index = index_fragmented_mp4(&probe_bytes)
            .ok_or_else(|| "direct playback currently requires fragmented MP4 indexing".to_string())?;
        let init_prefix = probe_bytes[..seek_index.init_end as usize].to_vec();
        let index = DirectMediaIndex::FragmentedMp4(FragmentedMp4Index {
            init_prefix,
            seek_index,
        });

        let mut cache = ByteRegionCache::new(config.cache_max_bytes);
        cache.insert(0, index.init_prefix().to_vec(), None);

        let mut demuxer = crate::fmp4_demux::IncrementalDemuxer::new();
        let mut decoder = Mp4DecodeSession::new();
        let mut prepared = None;
        let mut audio_output_info = None;
        for event in demuxer.push_data(index.init_prefix()) {
            if let FMp4Event::InitSegment(init) = event {
                prepared = Some(decoder.configure(&init)?);
                audio_output_info = init
                    .primary_audio_track()
                    .map(|track| (track.sample_rate, track.channels as u8));
            }
        }

        Ok(Self {
            reader,
            config,
            cache,
            cursor: DirectMediaCursor {
                current_time_ms: 0,
                current_byte_offset: index.initial_offset(),
            },
            index,
            demuxer,
            decoder,
            prepared,
            audio_output_info,
            reached_eos: false,
        })
    }

    pub fn pump(&mut self, current_position_ms: u64) -> Result<DirectPumpOutput, String> {
        let mut output = DirectPumpOutput {
            prepared: self.prepared.clone(),
            ..Default::default()
        };
        if self.reached_eos {
            output.buffered_ranges = self.decoder.buffered_ranges();
            output.reached_eos = true;
            return Ok(output);
        }

        for _ in 0..self.config.max_reads_per_pump {
            if !self.should_read_more(current_position_ms) {
                break;
            }
            if self.cursor.current_byte_offset >= self.config.content_length {
                self.reached_eos = true;
                output.reached_eos = true;
                break;
            }

            let start = self.cursor.current_byte_offset;
            let bytes = self.read_window(start, self.config.read_chunk_size)?;
            if bytes.is_empty() {
                self.reached_eos = true;
                output.reached_eos = true;
                break;
            }
            self.cursor.current_byte_offset = self.cursor.current_byte_offset.saturating_add(bytes.len() as u64);

            for event in self.demuxer.push_data(&bytes) {
                match event {
                    FMp4Event::InitSegment(init) => {
                        self.prepared = Some(self.decoder.configure(&init)?);
                        output.prepared = self.prepared.clone();
                    }
                    FMp4Event::MediaSamples(samples) => {
                        let decoded = self.decoder.decode_samples(&samples)?;
                        output.audio_frames.extend(decoded.audio_frames);
                        output.video_frames.extend(decoded.video_frames);
                    }
                }
            }
        }

        output.buffered_ranges = self.decoder.buffered_ranges();
        Ok(output)
    }

    pub fn seek_to(&mut self, target_ms: u64) -> Result<(), String> {
        self.cursor.current_time_ms = target_ms;
        self.cursor.current_byte_offset = self.index.seek_offset_for_time_ms(target_ms);
        self.reached_eos = false;
        self.demuxer = crate::fmp4_demux::IncrementalDemuxer::new();
        self.decoder.reset_for_seek();
        for event in self.demuxer.push_data(self.index.init_prefix()) {
            if let FMp4Event::InitSegment(init) = event {
                self.prepared = Some(self.decoder.configure(&init)?);
                self.audio_output_info = init
                    .primary_audio_track()
                    .map(|track| (track.sample_rate, track.channels as u8));
            }
        }
        Ok(())
    }

    pub fn audio_output_info(&self) -> Option<(u32, u8)> {
        self.audio_output_info
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.decoder.buffered_ranges()
    }

    fn should_read_more(&self, current_position_ms: u64) -> bool {
        self.prepared.is_none() || self.buffered_seconds_ahead(current_position_ms) < self.target_buffer_seconds()
    }

    fn target_buffer_seconds(&self) -> f64 {
        if self.prepared.is_some() {
            self.config.steady_buffer_target_secs
        } else {
            self.config.startup_buffer_target_secs
        }
    }

    fn buffered_seconds_ahead(&self, current_position_ms: u64) -> f64 {
        let current_seconds = current_position_ms as f64 / 1000.0;
        self.decoder
            .buffered_ranges()
            .into_iter()
            .find_map(|(start, end)| {
                if current_seconds + 0.05 < start || current_seconds > end + 0.05 {
                    return None;
                }
                Some((end - current_seconds).max(0.0))
            })
            .unwrap_or(0.0)
    }

    fn read_window(&mut self, start: u64, len: usize) -> Result<Vec<u8>, String> {
        let clamped_len = ((self.config.content_length.saturating_sub(start)) as usize).min(len);
        if clamped_len == 0 {
            return Ok(Vec::new());
        }
        if let Some(bytes) = self.cache.get_range(start, clamped_len) {
            return Ok(bytes);
        }
        let bytes = (self.reader)(start, clamped_len)?;
        self.cache.insert(
            start,
            bytes.clone(),
            Some((self.cursor.current_byte_offset, self.cursor.current_byte_offset.saturating_add(clamped_len as u64))),
        );
        Ok(bytes)
    }
}
