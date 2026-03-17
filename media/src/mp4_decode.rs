//! Shared MP4/fMP4 decode helpers.
//!
//! This module owns codec configuration, decoded-frame production, and buffered
//! range tracking for MP4 sample batches. It is reused by the direct
//! source-backed machine and the MSE append engine without coupling direct
//! playback to append semantics.

use crate::{
    aac::AacLcDecoder,
    fmp4_demux::{FMp4Codec, FMp4Init, FMp4Sample},
    MseAudioTrackInfo, MseDecodedAudioFrame, MseDecodedFrame, MseInitMetadata, MseVideoTrackInfo,
    PlaybackPrepared,
};
use makepad_platform::{FrameDecoderCodec, FrameDecoderConfig, VideoFrameDecoder, media_plugin};

pub struct Mp4DecodeSession {
    init: Option<FMp4Init>,
    video_decoder: VideoDecoderState,
    audio_decoder: Option<AudioDecoderState>,
    buffered: Vec<(f64, f64)>,
}

pub struct Mp4DecodeOutcome {
    pub audio_frames: Vec<MseDecodedAudioFrame>,
    pub video_frames: Vec<MseDecodedFrame>,
}

impl Mp4DecodeSession {
    pub fn new() -> Self {
        Self {
            init: None,
            video_decoder: VideoDecoderState::Pending,
            audio_decoder: None,
            buffered: Vec::new(),
        }
    }

    pub fn configure(&mut self, init: &FMp4Init) -> Result<PlaybackPrepared, String> {
        self.audio_decoder = init.primary_audio_track().and_then(|track| {
            if track.codec != FMp4Codec::Aac || track.codec_config.is_empty() {
                return None;
            }
            AacLcDecoder::new(&track.codec_config)
                .ok()
                .map(|decoder| AudioDecoderState {
                    track_id: track.track_id,
                    decoder,
                })
        });

        self.video_decoder = match init.primary_video_track() {
            Some(track) => match track.codec {
                #[cfg(has_dav1d)]
                FMp4Codec::Av1 => match crate::dav1d_ffi::Dav1dDecoder::new() {
                    Ok(decoder) => VideoDecoderState::Av1 {
                        track_id: track.track_id,
                        decoder,
                    },
                    Err(err) => VideoDecoderState::Unsupported(format!("dav1d init: {err}")),
                },
                #[cfg(not(has_dav1d))]
                FMp4Codec::Av1 => {
                    VideoDecoderState::Unsupported("AV1 decoding not available (no dav1d)".into())
                }
                FMp4Codec::H264 => match media_plugin()
                    .ok_or_else(|| "no media plugin".to_string())
                    .and_then(|plugin| {
                        plugin.create_video_frame_decoder(FrameDecoderConfig {
                            codec: FrameDecoderCodec::H264,
                            codec_config: track.codec_config.clone(),
                            width: track.width,
                            height: track.height,
                        })
                    }) {
                    Ok(decoder) => VideoDecoderState::H264(H264DecoderState {
                        track_id: track.track_id,
                        decoder,
                        nal_length_size: track.nal_length_size,
                    }),
                    Err(err) => VideoDecoderState::Unsupported(format!("H.264 decoder: {err}")),
                },
                FMp4Codec::Aac => VideoDecoderState::Pending,
            },
            None => VideoDecoderState::Pending,
        };

        self.init = Some(init.clone());
        self.buffered.clear();
        Ok(playback_prepared_from_init(init))
    }

    pub fn decode_samples(&mut self, samples: &[FMp4Sample]) -> Result<Mp4DecodeOutcome, String> {
        let Some(init) = self.init.as_ref() else {
            return Ok(Mp4DecodeOutcome {
                audio_frames: Vec::new(),
                video_frames: Vec::new(),
            });
        };

        update_buffered_ranges(&mut self.buffered, samples, init);

        let audio_frames = if let Some(audio) = &mut self.audio_decoder {
            decode_aac_samples(audio, samples, init)
        } else {
            Vec::new()
        };

        let video_frames = match &mut self.video_decoder {
            #[cfg(has_dav1d)]
            VideoDecoderState::Av1 { track_id, decoder } => {
                decode_av1_samples(*track_id, decoder, samples, init)
            }
            VideoDecoderState::H264(h264) => decode_h264_samples(h264, samples, init),
            VideoDecoderState::Unsupported(reason) => return Err(reason.clone()),
            VideoDecoderState::Pending => Vec::new(),
        };

        Ok(Mp4DecodeOutcome {
            audio_frames,
            video_frames,
        })
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.buffered.clone()
    }

    pub fn flush(&mut self) {
        match &mut self.video_decoder {
            #[cfg(has_dav1d)]
            VideoDecoderState::Av1 { decoder, .. } => decoder.flush(),
            VideoDecoderState::H264(h264) => h264.decoder.flush(),
            VideoDecoderState::Pending | VideoDecoderState::Unsupported(_) => {}
        }
        if let Some(audio) = &mut self.audio_decoder {
            audio.decoder.reset();
        }
    }

    pub fn reset_for_seek(&mut self) {
        self.flush();
        self.buffered.clear();
    }

    pub fn remove_buffered_range(&mut self, start: f64, end: f64) {
        self.buffered = self
            .buffered
            .iter()
            .filter_map(|&(s, e)| {
                if e <= start || s >= end {
                    Some((s, e))
                } else if s < start && e > end {
                    None
                } else if s < start {
                    Some((s, start))
                } else if e > end {
                    Some((end, e))
                } else {
                    None
                }
            })
            .collect();
    }

    pub fn cleanup(&mut self) {
        self.video_decoder = VideoDecoderState::Pending;
        self.audio_decoder = None;
        self.buffered.clear();
        self.init = None;
    }
}

pub fn mse_init_metadata_from_init(init: &FMp4Init) -> MseInitMetadata {
    MseInitMetadata {
        duration_ms: init.duration_ms,
        audio_tracks: init
            .audio_tracks
            .iter()
            .map(|track| MseAudioTrackInfo {
                track_id: track.track_id,
                codec: "mp4a.40.2".into(),
                sample_rate: track.sample_rate,
                channels: track.channels,
                config: track.codec_config.clone(),
            })
            .collect(),
        video_tracks: init
            .video_tracks
            .iter()
            .map(|track| MseVideoTrackInfo {
                track_id: track.track_id,
                codec: match track.codec {
                    FMp4Codec::Av1 => "av01".into(),
                    FMp4Codec::H264 => "avc1".into(),
                    FMp4Codec::Aac => "mp4a.40.2".into(),
                },
                width: track.width,
                height: track.height,
                config: track.codec_config.clone(),
            })
            .collect(),
    }
}

pub fn playback_prepared_from_init(init: &FMp4Init) -> PlaybackPrepared {
    PlaybackPrepared::new(
        init.video_tracks.first().map(|track| track.width).unwrap_or(0),
        init.video_tracks.first().map(|track| track.height).unwrap_or(0),
        init.duration_ms,
        init.duration_ms > 0,
        init.video_tracks
            .iter()
            .map(|track| match track.codec {
                FMp4Codec::Av1 => "av01".to_string(),
                FMp4Codec::H264 => "avc1".to_string(),
                FMp4Codec::Aac => "mp4a.40.2".to_string(),
            })
            .collect(),
        init.audio_tracks
            .iter()
            .map(|track| match track.codec {
                FMp4Codec::Aac => "mp4a.40.2".to_string(),
                FMp4Codec::Av1 => "av01".to_string(),
                FMp4Codec::H264 => "avc1".to_string(),
            })
            .collect(),
    )
}

enum VideoDecoderState {
    Pending,
    #[cfg(has_dav1d)]
    Av1 {
        track_id: u32,
        decoder: crate::dav1d_ffi::Dav1dDecoder,
    },
    H264(H264DecoderState),
    Unsupported(String),
}

struct H264DecoderState {
    track_id: u32,
    decoder: Box<dyn VideoFrameDecoder>,
    nal_length_size: u8,
}

struct AudioDecoderState {
    track_id: u32,
    decoder: AacLcDecoder,
}

fn update_buffered_ranges(buffered: &mut Vec<(f64, f64)>, samples: &[FMp4Sample], init: &FMp4Init) {
    if samples.is_empty() {
        return;
    }
    let start_ms = samples
        .iter()
        .map(|sample| init.track_ts_to_ms(sample.track_id, sample.pts))
        .min()
        .unwrap_or(0);
    let end_ms = samples
        .iter()
        .map(|sample| {
            init.track_ts_to_ms(sample.track_id, sample.pts.saturating_add(sample.duration as u64))
        })
        .max()
        .unwrap_or(start_ms);
    extend_buffered(buffered, start_ms as f64 / 1000.0, end_ms as f64 / 1000.0);
}

fn extend_buffered(buffered: &mut Vec<(f64, f64)>, start: f64, end: f64) {
    if start >= end {
        return;
    }
    let mut merged = false;
    for range in buffered.iter_mut() {
        if start <= range.1 + 0.05 && end >= range.0 - 0.05 {
            range.0 = range.0.min(start);
            range.1 = range.1.max(end);
            merged = true;
            break;
        }
    }
    if !merged {
        buffered.push((start, end));
    }
    buffered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut i = 0;
    while i + 1 < buffered.len() {
        if buffered[i].1 >= buffered[i + 1].0 - 0.05 {
            buffered[i].1 = buffered[i].1.max(buffered[i + 1].1);
            buffered.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

#[cfg(has_dav1d)]
fn decode_av1_samples(
    track_id: u32,
    decoder: &mut crate::dav1d_ffi::Dav1dDecoder,
    samples: &[FMp4Sample],
    init: &FMp4Init,
) -> Vec<MseDecodedFrame> {
    let mut frames = Vec::new();
    for sample in samples.iter().filter(|sample| sample.track_id == track_id) {
        let pts_ms = init.track_ts_to_ms(sample.track_id, sample.pts);
        match decoder.send_data(&sample.data, pts_ms as i64) {
            Ok(true) => {}
            Ok(false) => {
                if let Ok(Some(pic)) = decoder.get_picture() {
                    frames.push(decoded_picture_to_frame(track_id, &pic));
                }
                let _ = decoder.send_data(&sample.data, pts_ms as i64);
            }
            Err(_) => continue,
        }
        while let Ok(Some(pic)) = decoder.get_picture() {
            frames.push(decoded_picture_to_frame(track_id, &pic));
        }
    }
    frames
}

#[cfg(has_dav1d)]
fn decoded_picture_to_frame(
    track_id: u32,
    pic: &crate::dav1d_ffi::DecodedPicture,
) -> MseDecodedFrame {
    let planes = crate::yuv::extract_yuv_planes(pic);
    MseDecodedFrame {
        track_id,
        pts_ms: pic.timestamp() as u64,
        yuv: crate::YuvPlaneData {
            y: planes.y,
            u: planes.u,
            v: planes.v,
            width: planes.width,
            height: planes.height,
            layout: match planes.layout {
                crate::yuv::YuvLayout::I420 => crate::YuvLayout::I420,
                crate::yuv::YuvLayout::I422 => crate::YuvLayout::I422,
                crate::yuv::YuvLayout::I444 => crate::YuvLayout::I444,
                crate::yuv::YuvLayout::I400 => crate::YuvLayout::I400,
            },
            matrix: match planes.matrix {
                crate::yuv::YuvColorMatrix::BT709 => crate::YuvColorMatrix::BT709,
                crate::yuv::YuvColorMatrix::BT601 => crate::YuvColorMatrix::BT601,
                crate::yuv::YuvColorMatrix::BT2020 => crate::YuvColorMatrix::BT2020,
            },
        },
    }
}

fn decode_h264_samples(
    h264: &mut H264DecoderState,
    samples: &[FMp4Sample],
    init: &FMp4Init,
) -> Vec<MseDecodedFrame> {
    let mut frames = Vec::new();
    for sample in samples.iter().filter(|sample| sample.track_id == h264.track_id) {
        let pts_ms = init.track_ts_to_ms(sample.track_id, sample.pts);
        let annexb_data = if h264.nal_length_size > 0 {
            crate::h264_packets::avcc_sample_to_annexb(&sample.data, h264.nal_length_size as usize)
        } else {
            Some(sample.data.clone())
        };
        let Some(data) = annexb_data else {
            continue;
        };
        if h264.decoder.push_data(&data, pts_ms).is_err() {
            continue;
        }
        while let Ok(Some(frame)) = h264.decoder.pull_frame() {
            frames.push(MseDecodedFrame {
                track_id: h264.track_id,
                pts_ms: frame.pts_ms,
                yuv: frame.yuv,
            });
        }
    }
    frames
}

fn decode_aac_samples(
    audio: &mut AudioDecoderState,
    samples: &[FMp4Sample],
    init: &FMp4Init,
) -> Vec<MseDecodedAudioFrame> {
    let mut frames = Vec::new();
    for sample in samples.iter().filter(|sample| sample.track_id == audio.track_id) {
        let Ok(pcm) = audio.decoder.decode_access_unit(&sample.data, sample.pts) else {
            continue;
        };
        frames.push(MseDecodedAudioFrame {
            track_id: audio.track_id,
            pts_ms: init.track_ts_to_ms(sample.track_id, sample.pts),
            sample_rate: pcm.sample_rate,
            channels: pcm.channels as u16,
            samples: pcm.samples,
        });
    }
    frames
}
