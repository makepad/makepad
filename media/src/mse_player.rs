//! MSE engine implementation: incremental fMP4 demux + shared MP4 decode.
//!
//! This layer owns append semantics and incremental demux integration. Codec
//! configuration and sample decoding are shared with the direct source-backed
//! machine through `mp4_decode`.

use crate::{
    fmp4_demux::{FMp4Event, IncrementalDemuxer},
    mp4_decode::{Mp4DecodeSession, mse_init_metadata_from_init},
};
use makepad_platform::{MseEngineOutput, MsePlaybackEngine};

pub struct SoftwareMsePlaybackEngine {
    demuxer: IncrementalDemuxer,
    decoder: Mp4DecodeSession,
}

impl SoftwareMsePlaybackEngine {
    pub fn new(mime: &str) -> Result<Self, String> {
        let base = mime.split(';').next().unwrap_or("").trim();
        if base != "video/mp4" && base != "video/x-m4v" && base != "audio/mp4" {
            return Err(format!("unsupported container: {base}"));
        }
        Ok(Self {
            demuxer: IncrementalDemuxer::new(),
            decoder: Mp4DecodeSession::new(),
        })
    }
}

impl MsePlaybackEngine for SoftwareMsePlaybackEngine {
    fn append_data(&mut self, data: &[u8]) -> Result<MseEngineOutput, String> {
        let events = self.demuxer.push_data(data);
        let mut output = MseEngineOutput::default();

        for event in events {
            match event {
                FMp4Event::InitSegment(init) => {
                    self.decoder.configure(&init)?;
                    output.init = Some(mse_init_metadata_from_init(&init));
                }
                FMp4Event::MediaSamples(samples) => {
                    let decoded = self.decoder.decode_samples(&samples)?;
                    output.audio_frames.extend(decoded.audio_frames);
                    output.video_frames.extend(decoded.video_frames);
                }
            }
        }

        output.buffered_ranges = self.decoder.buffered_ranges();
        Ok(output)
    }

    fn end_of_stream(&mut self) -> Result<MseEngineOutput, String> {
        Ok(MseEngineOutput {
            init: None,
            audio_frames: Vec::new(),
            video_frames: Vec::new(),
            buffered_ranges: self.decoder.buffered_ranges(),
            reached_eos: true,
        })
    }

    fn remove(&mut self, start: f64, end: f64) {
        self.decoder.remove_buffered_range(start, end);
    }

    fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.decoder.buffered_ranges()
    }

    fn flush(&mut self) {
        self.decoder.flush();
    }

    fn cleanup(&mut self) {
        self.decoder.cleanup();
    }
}
