use makepad_platform::{
    MediaPlaybackSession, PlaybackPrepared, VideoFrameSession, VideoFrameSessionId,
    VideoSessionState, take_registered_video_frame_session, video_decode::yuv::YuvPlaneData,
};

pub struct VideoFrameSessionPlayer {
    session: Box<dyn VideoFrameSession>,
    playing: bool,
    active: bool,
    prepared_notified: bool,
    error_notified: bool,
    eos_notified: bool,
    dimensions: Option<(u32, u32)>,
    pending_yuv: Option<YuvPlaneData>,
    current_position_ms: u128,
}

impl VideoFrameSessionPlayer {
    pub fn from_registered(id: VideoFrameSessionId, autoplay: bool) -> Result<Self, String> {
        let session = take_registered_video_frame_session(id)
            .ok_or_else(|| format!("video session source {} is not registered", id.0))?;
        Ok(Self::new(session, autoplay))
    }

    pub fn new(session: Box<dyn VideoFrameSession>, autoplay: bool) -> Self {
        Self {
            session,
            playing: autoplay,
            active: true,
            prepared_notified: false,
            error_notified: false,
            eos_notified: false,
            dimensions: None,
            pending_yuv: None,
            current_position_ms: 0,
        }
    }

    fn current_dimensions(&mut self) -> Option<(u32, u32)> {
        if self.dimensions.is_none() {
            self.dimensions = self.session.dimensions();
        }
        self.dimensions
    }

    fn update_dimensions_from_frame(&mut self, frame: &makepad_platform::MseDecodedFrame) {
        if self.dimensions.is_none() && frame.yuv.width > 0 && frame.yuv.height > 0 {
            self.dimensions = Some((frame.yuv.width, frame.yuv.height));
        }
    }
}

impl MediaPlaybackSession for VideoFrameSessionPlayer {
    fn check_prepared(
        &mut self,
    ) -> Option<Result<PlaybackPrepared, String>> {
        if !self.prepared_notified {
            if let Some((width, height)) = self.current_dimensions() {
                self.prepared_notified = true;
                return Some(Ok(PlaybackPrepared::new(
                    width,
                    height,
                    0,
                    false,
                    vec!["video".into()],
                    vec![],
                )));
            }
        }

        match self.session.state() {
            VideoSessionState::Error(error) if !self.error_notified => {
                self.error_notified = true;
                Some(Err(error))
            }
            VideoSessionState::Ended if !self.prepared_notified && !self.error_notified => {
                self.error_notified = true;
                Some(Err("video session ended before it became ready".into()))
            }
            _ => None,
        }
    }

    fn poll_frame(&mut self) -> bool {
        let mut frames = self.session.take_frames();
        let Some(frame) = frames.pop() else {
            return false;
        };

        self.update_dimensions_from_frame(&frame);

        if !self.playing {
            return false;
        }

        self.current_position_ms = frame.pts_ms as u128;
        self.pending_yuv = Some(frame.yuv);
        true
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.pending_yuv.take()
    }

    fn check_eos(&mut self) -> bool {
        if self.eos_notified {
            return false;
        }
        if self.session.state() == VideoSessionState::Ended {
            self.eos_notified = true;
            return true;
        }
        false
    }

    fn play(&mut self) {
        self.playing = true;
    }

    fn pause(&mut self) {
        self.playing = false;
    }

    fn resume(&mut self) {
        self.playing = true;
    }

    fn is_playing(&self) -> bool {
        self.playing
    }

    fn seek_to(&mut self, _position_ms: u64) {}

    fn set_volume(&self, _volume: f64) {}

    fn current_position_ms(&self) -> u128 {
        self.current_position_ms
    }

    fn mute(&self) {}

    fn unmute(&self) {}

    fn set_playback_rate(&self, _rate: f64) {}

    fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        Vec::new()
    }

    fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        Vec::new()
    }

    fn is_active(&self) -> bool {
        self.active
    }

    fn cleanup(&mut self) {
        self.pending_yuv = None;
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_platform::{MseDecodedFrame, VideoSessionState, video_decode::yuv::{YuvColorMatrix, YuvLayout}};
    use std::{collections::VecDeque, sync::{Arc, Mutex}};

    struct StubState {
        dimensions: Option<(u32, u32)>,
        frames: VecDeque<MseDecodedFrame>,
        state: VideoSessionState,
    }

    impl Default for StubState {
        fn default() -> Self {
            Self {
                dimensions: None,
                frames: VecDeque::new(),
                state: VideoSessionState::Connecting,
            }
        }
    }

    struct StubSession {
        shared: Arc<Mutex<StubState>>,
    }

    impl VideoFrameSession for StubSession {
        fn take_frames(&mut self) -> Vec<MseDecodedFrame> {
            self.shared.lock().unwrap().frames.drain(..).collect()
        }

        fn dimensions(&self) -> Option<(u32, u32)> {
            self.shared.lock().unwrap().dimensions
        }

        fn state(&self) -> VideoSessionState {
            self.shared.lock().unwrap().state.clone()
        }
    }

    fn frame(pts_ms: u64) -> MseDecodedFrame {
        MseDecodedFrame {
            track_id: 0,
            pts_ms,
            yuv: YuvPlaneData {
                y: vec![16; 4],
                u: vec![128; 1],
                v: vec![128; 1],
                width: 2,
                height: 2,
                layout: YuvLayout::I420,
                matrix: YuvColorMatrix::BT709,
            },
        }
    }

    #[test]
    fn player_prepares_and_emits_latest_frame() {
        let shared = Arc::new(Mutex::new(StubState {
            dimensions: Some((2, 2)),
            frames: VecDeque::from([frame(10), frame(20)]),
            state: VideoSessionState::Active,
        }));
        let mut player = VideoFrameSessionPlayer::new(
            Box::new(StubSession { shared }),
            true,
        );

        let prepared = player.check_prepared().unwrap().unwrap();
        assert_eq!(prepared.width, 2);
        assert_eq!(prepared.height, 2);
        assert_eq!(prepared.duration_ms, 0);
        assert!(!prepared.is_seekable);
        assert_eq!(prepared.video_tracks, vec!["video".to_string()]);
        assert!(prepared.audio_tracks.is_empty());
        assert!(player.poll_frame());
        assert_eq!(player.current_position_ms(), 20);
        assert_eq!(player.take_yuv_frame().unwrap().width, 2);
        assert!(!player.poll_frame());
    }

    #[test]
    fn player_discards_frames_while_paused() {
        let shared = Arc::new(Mutex::new(StubState {
            dimensions: Some((2, 2)),
            frames: VecDeque::from([frame(10)]),
            state: VideoSessionState::Active,
        }));
        let mut player = VideoFrameSessionPlayer::new(
            Box::new(StubSession { shared }),
            false,
        );

        assert_eq!(player.check_prepared().unwrap().unwrap().width, 2);
        assert!(!player.poll_frame());
        assert!(player.take_yuv_frame().is_none());
    }

    #[test]
    fn player_reports_error_and_eos_once() {
        let shared = Arc::new(Mutex::new(StubState {
            dimensions: None,
            frames: VecDeque::new(),
            state: VideoSessionState::Error("boom".into()),
        }));
        let mut player = VideoFrameSessionPlayer::new(
            Box::new(StubSession { shared: shared.clone() }),
            true,
        );

        assert_eq!(player.check_prepared(), Some(Err("boom".into())));
        assert_eq!(player.check_prepared(), None);

        shared.lock().unwrap().state = VideoSessionState::Ended;
        assert!(player.check_eos());
        assert!(!player.check_eos());
    }
}
