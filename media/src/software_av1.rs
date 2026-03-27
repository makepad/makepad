use {
    crate::{
        dav1d_ffi::{Dav1dDecoder, Dav1dPicAllocator, DecodedPicture},
        demux::{self, Mp4Track},
        yuv::{self, YuvPlaneData},
    },
    makepad_platform::PlaybackPrepared,
    std::{io::Cursor, rc::Rc, time::Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub enum VideoSource {
    InMemory(Rc<Vec<u8>>),
    Network(String),
    Filesystem(String),
    Camera,
}

pub struct SoftwareAv1Player {
    allocator: Option<Dav1dPicAllocator>,
    state: PlayerState,
}

enum PlayerState {
    Loading {
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
    },
    Active(ActivePlayer),
    Failed(String),
}

struct ActivePlayer {
    decoder: Dav1dDecoder,
    track: Mp4Track,
    file_data: Vec<u8>,
    autoplay: bool,
    is_looping: bool,
    playing: bool,
    start_time: Option<Instant>,
    pause_offset_ms: u64,
    current_sample: usize,
    pub yuv_frame: Option<YuvPlaneData>,
    pub decoded_pic: Option<DecodedPicture>,
    pub has_new_frame: bool,
    pub has_custom_allocator: bool,
    prepared: bool,
    prepare_notified: bool,
}

impl SoftwareAv1Player {
    pub fn new(source: VideoSource, autoplay: bool, is_looping: bool) -> Self {
        Self {
            allocator: None,
            state: PlayerState::Loading {
                source,
                autoplay,
                is_looping,
            },
        }
    }

    pub fn new_with_allocator(
        source: VideoSource,
        autoplay: bool,
        is_looping: bool,
        allocator: Dav1dPicAllocator,
    ) -> Self {
        Self {
            allocator: Some(allocator),
            state: PlayerState::Loading {
                source,
                autoplay,
                is_looping,
            },
        }
    }

    pub fn check_prepared(
        &mut self,
    ) -> Option<Result<PlaybackPrepared, String>> {
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

            match Self::init_active(&source, autoplay, is_looping, self.allocator.take()) {
                Ok(active) => {
                    self.state = PlayerState::Active(active);
                }
                Err(e) => {
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

                if active.autoplay {
                    active.playing = true;
                    active.start_time = Some(Instant::now());
                }

                Some(Ok(PlaybackPrepared::new(
                    w,
                    h,
                    dur,
                    true,
                    vec!["video".into()],
                    vec![],
                )))
            }
            PlayerState::Failed(e) => Some(Err(e.clone())),
            PlayerState::Loading { .. } => None,
        }
    }

    fn init_active(
        source: &VideoSource,
        autoplay: bool,
        is_looping: bool,
        allocator: Option<Dav1dPicAllocator>,
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
            VideoSource::Camera => {
                return Err("camera sources not supported in software player".to_string());
            }
        };

        let mut cursor = Cursor::new(&file_data);
        let track = demux::parse_mp4(&mut cursor).map_err(|e| format!("MP4 parse: {}", e))?;

        let has_custom_allocator = allocator.is_some();
        let decoder = if let Some(alloc) = allocator {
            Dav1dDecoder::new_with_allocator(alloc)?
        } else {
            Dav1dDecoder::new()?
        };
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
            yuv_frame: None,
            decoded_pic: None,
            has_new_frame: false,
            has_custom_allocator,
            prepared: false,
            prepare_notified: false,
        })
    }

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

        let num_samples = active.track.samples.len();
        if num_samples == 0 {
            return false;
        }

        let mut decoded_new = false;
        while active.current_sample < num_samples {
            let pts_ms = active.track.sample_pts_ms(active.current_sample);
            if pts_ms > elapsed_ms {
                break;
            }

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

            match active.decoder.send_data(sample_data, pts_ms as i64) {
                Ok(true) => {}
                Ok(false) => {
                    if let Ok(Some(pic)) = active.decoder.get_picture() {
                        let planes = yuv::extract_yuv_planes(&pic);
                        active.yuv_frame = Some(planes);
                        if active.has_custom_allocator {
                            active.decoded_pic = Some(pic);
                        }
                        decoded_new = true;
                    }
                    let _ = active.decoder.send_data(sample_data, pts_ms as i64);
                }
                Err(_e) => {}
            }

            if let Ok(Some(pic)) = active.decoder.get_picture() {
                let planes = yuv::extract_yuv_planes(&pic);
                active.yuv_frame = Some(planes);
                if active.has_custom_allocator {
                    active.decoded_pic = Some(pic);
                }
                decoded_new = true;
            }

            active.current_sample += 1;
        }

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

    pub fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        let active = match &mut self.state {
            PlayerState::Active(a) => a,
            _ => return None,
        };

        if active.has_new_frame && active.yuv_frame.is_some() {
            active.has_new_frame = false;
            active.yuv_frame.take()
        } else {
            None
        }
    }

    pub fn take_decoded_picture(&mut self) -> Option<DecodedPicture> {
        let active = match &mut self.state {
            PlayerState::Active(a) => a,
            _ => return None,
        };
        active.decoded_pic.take()
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
            a.start_time = if a.playing { Some(Instant::now()) } else { None };

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

    pub fn set_volume(&self, _volume: f64) {}

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
    pub fn set_playback_rate(&self, _rate: f64) {}

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
