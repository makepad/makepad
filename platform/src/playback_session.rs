use crate::media_plugin::MediaPlaybackSession;
use crate::{AudioBuffer, AudioInfo};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

/// Opaque handle for a registered custom playback session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MediaPlaybackSessionId(pub u64);

type SendPlaybackSession = Box<dyn MediaPlaybackSession + Send>;
type SharedPlaybackSession = Arc<Mutex<SendPlaybackSession>>;

static MEDIA_PLAYBACK_SESSION_IDS: AtomicU64 = AtomicU64::new(1);
static MEDIA_PLAYBACK_SESSIONS: OnceLock<
    Mutex<HashMap<MediaPlaybackSessionId, SendPlaybackSession>>,
> = OnceLock::new();
static ACTIVE_MEDIA_AUDIO: OnceLock<Mutex<HashMap<MediaPlaybackSessionId, SharedPlaybackSession>>> =
    OnceLock::new();

fn media_playback_sessions() -> &'static Mutex<HashMap<MediaPlaybackSessionId, SendPlaybackSession>>
{
    MEDIA_PLAYBACK_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn active_media_audio() -> &'static Mutex<HashMap<MediaPlaybackSessionId, SharedPlaybackSession>> {
    ACTIVE_MEDIA_AUDIO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a custom playback session for later handoff into Makepad playback.
pub fn register_media_playback_session(session: SendPlaybackSession) -> MediaPlaybackSessionId {
    let id = MediaPlaybackSessionId(MEDIA_PLAYBACK_SESSION_IDS.fetch_add(1, Ordering::Relaxed));
    media_playback_sessions()
        .lock()
        .unwrap()
        .insert(id, session);
    id
}

/// Remove a registered playback session that has not yet been consumed.
pub fn unregister_media_playback_session(
    id: MediaPlaybackSessionId,
) -> Option<SendPlaybackSession> {
    media_playback_sessions().lock().unwrap().remove(&id)
}

/// Take ownership of a registered playback session.
#[doc(hidden)]
pub fn take_registered_media_playback_session(
    id: MediaPlaybackSessionId,
) -> Option<SendPlaybackSession> {
    unregister_media_playback_session(id)
}

/// Register an active custom session for Makepad audio output mixing.
pub fn register_active_media_audio(id: MediaPlaybackSessionId, session: SharedPlaybackSession) {
    active_media_audio().lock().unwrap().insert(id, session);
}

/// Remove an active custom session from Makepad audio output mixing.
pub fn unregister_active_media_audio(id: MediaPlaybackSessionId) {
    active_media_audio().lock().unwrap().remove(&id);
}

/// Mix active custom-session audio into one output buffer.
pub fn mix_active_media_audio(info: AudioInfo, output: &mut AudioBuffer) {
    output.zero();
    let sessions: Vec<SharedPlaybackSession> = active_media_audio()
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    for session in sessions {
        if let Ok(mut session) = session.lock() {
            session.fill_audio_output(info, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_plugin::PlaybackPrepared;
    use crate::video_decode::yuv::YuvPlaneData;

    struct StubSession {
        audio_calls: usize,
    }

    impl MediaPlaybackSession for StubSession {
        fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
            Some(Ok(PlaybackPrepared::new(1, 1, 0, false, vec![], vec![])))
        }

        fn poll_frame(&mut self) -> bool {
            false
        }
        fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
            None
        }
        fn check_eos(&mut self) -> bool {
            false
        }
        fn play(&mut self) {}
        fn pause(&mut self) {}
        fn resume(&mut self) {}
        fn is_playing(&self) -> bool {
            true
        }
        fn seek_to(&mut self, _position_ms: u64) {}
        fn set_volume(&self, _volume: f64) {}
        fn current_position_ms(&self) -> u128 {
            0
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
        fn fill_audio_output(&mut self, _info: AudioInfo, output: &mut AudioBuffer) {
            self.audio_calls += 1;
            if let Some(sample) = output.data.first_mut() {
                *sample += 0.5;
            }
        }
        fn is_active(&self) -> bool {
            true
        }
        fn cleanup(&mut self) {}
    }

    #[test]
    fn registry_roundtrip() {
        let id = register_media_playback_session(Box::new(StubSession { audio_calls: 0 }));
        let mut session = take_registered_media_playback_session(id).expect("session missing");
        assert!(session.check_prepared().is_some());
        assert!(take_registered_media_playback_session(id).is_none());
    }

    #[test]
    fn active_audio_mix_calls_registered_sessions() {
        let id = MediaPlaybackSessionId(999_001);
        let session: SharedPlaybackSession =
            Arc::new(Mutex::new(Box::new(StubSession { audio_calls: 0 })));
        register_active_media_audio(id, session.clone());

        let mut output = AudioBuffer::new_with_size(16, 2);
        mix_active_media_audio(
            AudioInfo {
                device_id: Default::default(),
                time: None,
                sample_rate: 48_000.0,
            },
            &mut output,
        );

        assert!(output.data[0] > 0.0);
        unregister_active_media_audio(id);
    }
}
