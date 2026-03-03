//! Software AV1 player using a pure-Rust rav1d decoder.
//!
//! Provides a fallback when native platform video players are unavailable
//! or don't support AV1. Decodes AV1 samples from MP4 containers using
//! rav1d, converts YUV to RGBA, and presents frames for texture upload.

use {
    super::mp4_demux::{self, Mp4Track},
    super::rav1d::Rav1dDecoder,
    super::yuv,
    crate::event::video_playback::VideoSource,
    crate::makepad_live_id::LiveId,
    crate::texture::TextureId,
    std::io::Cursor,
    std::time::Instant,
};

pub struct SoftwareAv1Player {
    pub video_id: LiveId,
    pub texture_id: TextureId,
    state: PlayerState,
}

enum PlayerState {
    /// Waiting for data to be loaded and parsed.
    Loading {
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    },
    /// Ready to decode frames.
    Active(ActivePlayer),
    /// Failed to initialize.
    Failed(String),
}

struct ActivePlayer {
    decoder: Rav1dDecoder,
    track: Mp4Track,
    file_data: Vec<u8>,
    autoplay: bool,
    is_looping: bool,
    // Playback state
    playing: bool,
    start_time: Option<Instant>,
    pause_offset_ms: u64,
    current_sample: usize,
    // Frame buffer
    pub rgba_buf: Vec<u8>,
    pub frame_width: u32,
    pub frame_height: u32,
    pub has_new_frame: bool,
    // Metadata
    prepared: bool,
    prepare_notified: bool,
}

impl SoftwareAv1Player {
    pub fn new(
        video_id: LiveId,
        texture_id: TextureId,
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Self {
        SoftwareAv1Player {
            video_id,
            texture_id,
            state: PlayerState::Loading {
                source,
                autoplay,
                is_looping,
            },
        }
    }

    /// Try to prepare the player. Called each frame until it returns Some.
    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        // Handle loading state — load the file and parse MP4
        if let PlayerState::Loading { .. } = &self.state {
            let (source, autoplay, is_looping) = match std::mem::replace(
                &mut self.state,
                PlayerState::Failed("transitioning".into()),
            ) {
                PlayerState::Loading {
                    source,
                    autoplay,
                    is_looping,
                } => (source, autoplay, is_looping),
                _ => unreachable!(),
            };

            match Self::init_active(&source, autoplay, is_looping) {
                Ok(active) => {
                    self.state = PlayerState::Active(active);
                }
                Err(e) => {
                    crate::error!("SOFTWARE_AV1: init failed: {}", e);
                    self.state = PlayerState::Failed(e.clone());
                    return Some(Err(e));
                }
            }
        }

        match &mut self.state {
            PlayerState::Active(active) => {
                if active.prepare_notified {
                    return None;
                }
                active.prepare_notified = true;
                active.prepared = true;

                let w = active.track.width;
                let h = active.track.height;
                let dur = active.track.duration_ms();
                crate::log!(
                    "SOFTWARE_AV1: prepared id={} {}x{} duration={}ms samples={}",
                    self.video_id.0,
                    w,
                    h,
                    dur,
                    active.track.samples.len()
                );

                if active.autoplay {
                    active.playing = true;
                    active.start_time = Some(Instant::now());
                }

                Some(Ok((w, h, dur, true, vec!["video".into()], vec![])))
            }
            PlayerState::Failed(e) => Some(Err(e.clone())),
            PlayerState::Loading { .. } => None,
        }
    }

    fn init_active(
        source: &VideoSource,
        autoplay: bool,
        is_looping: bool,
    ) -> Result<ActivePlayer, String> {
        let file_data = match source {
            VideoSource::Filesystem(path) => {
                std::fs::read(path).map_err(|e| format!("read file: {}", e))?
            }
            VideoSource::InMemory(data) => data.as_ref().clone(),
            VideoSource::Network(url) => {
                return Err(format!(
                    "network sources not supported in software player: {}",
                    url
                ));
            }
        };

        let mut cursor = Cursor::new(&file_data);
        let track = mp4_demux::parse_mp4(&mut cursor).map_err(|e| format!("MP4 parse: {}", e))?;

        let decoder = Rav1dDecoder::new()?;

        Ok(ActivePlayer {
            decoder,
            track,
            file_data,
            autoplay,
            is_looping,
            playing: false,
            start_time: None,
            pause_offset_ms: 0,
            current_sample: 0,
            rgba_buf: Vec::new(),
            frame_width: 0,
            frame_height: 0,
            has_new_frame: false,
            prepared: false,
            prepare_notified: false,
        })
    }

    /// Decode and produce the next frame if it's time. Returns true if a new
    /// RGBA frame is available in `rgba_buf`.
    pub fn poll_frame(&mut self) -> bool {
        let active = match &mut self.state {
            PlayerState::Active(a) => a,
            _ => return false,
        };

        if !active.playing || !active.prepared {
            return false;
        }

        let elapsed_ms = match active.start_time {
            Some(t) => t.elapsed().as_millis() as u64 + active.pause_offset_ms,
            None => active.pause_offset_ms,
        };

        // Find the sample whose PTS is closest to current time
        let num_samples = active.track.samples.len();
        if num_samples == 0 {
            return false;
        }

        // Advance through samples until we find one past current time
        let mut decoded_new = false;
        while active.current_sample < num_samples {
            let pts_ms = active.track.sample_pts_ms(active.current_sample);
            if pts_ms > elapsed_ms {
                break;
            }

            // Decode this sample
            let sample_data = {
                let s = &active.track.samples[active.current_sample];
                let start = s.offset as usize;
                let end = start + s.size as usize;
                if end > active.file_data.len() {
                    active.current_sample += 1;
                    continue;
                }
                &active.file_data[start..end]
            };

            // Send to rav1d
            match active.decoder.send_data(sample_data, pts_ms as i64) {
                Ok(true) => {}
                Ok(false) => {
                    // EAGAIN — drain a picture first
                    if let Ok(Some(pic)) = active.decoder.get_picture() {
                        yuv::picture_to_rgba(&pic, &mut active.rgba_buf);
                        active.frame_width = pic.width();
                        active.frame_height = pic.height();
                        decoded_new = true;
                    }
                    // Retry send
                    let _ = active.decoder.send_data(sample_data, pts_ms as i64);
                }
                Err(_e) => {}
            }

            // Try to get decoded picture
            if let Ok(Some(pic)) = active.decoder.get_picture() {
                yuv::picture_to_rgba(&pic, &mut active.rgba_buf);
                active.frame_width = pic.width();
                active.frame_height = pic.height();
                decoded_new = true;
            }

            active.current_sample += 1;
        }

        // Handle looping
        if active.current_sample >= num_samples && active.is_looping {
            active.current_sample = 0;
            active.start_time = Some(Instant::now());
            active.pause_offset_ms = 0;
            active.decoder.flush();
        }

        if decoded_new {
            active.has_new_frame = true;
        }

        decoded_new
    }

    /// Get the current RGBA frame data and dimensions, if a new frame is available.
    pub fn take_frame(&mut self) -> Option<(&[u8], u32, u32)> {
        let active = match &mut self.state {
            PlayerState::Active(a) => a,
            _ => return None,
        };

        if active.has_new_frame && !active.rgba_buf.is_empty() {
            active.has_new_frame = false;
            Some((&active.rgba_buf, active.frame_width, active.frame_height))
        } else {
            None
        }
    }

    pub fn check_eos(&mut self) -> bool {
        let active = match &mut self.state {
            PlayerState::Active(a) => a,
            _ => return false,
        };
        if active.is_looping {
            return false;
        }
        active.current_sample >= active.track.samples.len()
    }

    pub fn play(&mut self) {
        if let PlayerState::Active(a) = &mut self.state {
            if !a.playing {
                a.playing = true;
                a.start_time = Some(Instant::now());
            }
        }
    }

    pub fn pause(&mut self) {
        if let PlayerState::Active(a) = &mut self.state {
            if a.playing {
                if let Some(t) = a.start_time.take() {
                    a.pause_offset_ms += t.elapsed().as_millis() as u64;
                }
                a.playing = false;
            }
        }
    }

    pub fn resume(&mut self) {
        self.play();
    }

    pub fn is_playing(&self) -> bool {
        if let PlayerState::Active(a) = &self.state {
            return a.playing;
        }
        false
    }

    pub fn seek_to(&mut self, position_ms: u64) {
        if let PlayerState::Active(a) = &mut self.state {
            a.pause_offset_ms = position_ms;
            a.start_time = if a.playing {
                Some(Instant::now())
            } else {
                None
            };

            // Find the nearest sync sample at or before position_ms
            let mut target = 0;
            for (i, s) in a.track.samples.iter().enumerate() {
                let pts = a.track.sample_pts_ms(i);
                if pts <= position_ms {
                    if s.is_sync {
                        target = i;
                    }
                } else {
                    break;
                }
            }
            a.current_sample = target;
            a.decoder.flush();
        }
    }

    pub fn set_volume(&self, _volume: f64) {
        // Audio not implemented in software player
    }

    pub fn current_position_ms(&self) -> u128 {
        if let PlayerState::Active(a) = &self.state {
            let elapsed = match a.start_time {
                Some(t) if a.playing => t.elapsed().as_millis() as u64 + a.pause_offset_ms,
                _ => a.pause_offset_ms,
            };
            return elapsed as u128;
        }
        0
    }

    pub fn mute(&self) {}
    pub fn unmute(&self) {}

    pub fn set_playback_rate(&self, _rate: f64) {
        // Not implemented
    }

    pub fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        if let PlayerState::Active(a) = &self.state {
            let dur_s = a.track.duration_ms() as f64 / 1000.0;
            if dur_s > 0.0 {
                return vec![(0.0, dur_s)];
            }
        }
        vec![]
    }

    pub fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.seekable_ranges()
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, PlayerState::Active(_))
    }

    pub fn cleanup(&mut self) {
        self.state = PlayerState::Failed("cleaned up".into());
    }
}

impl Drop for SoftwareAv1Player {
    fn drop(&mut self) {
        self.cleanup();
    }
}
