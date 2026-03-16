use crate::media_plugin::MseDecodedFrame;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

/// State of an app-owned video frame session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoSessionState {
    /// Session is initializing and has not produced displayable output yet.
    Connecting,
    /// Session is active and may produce frames.
    Active,
    /// Session ended cleanly.
    Ended,
    /// Session failed.
    Error(String),
}

/// App-owned decoded-frame session for the generic Makepad video path.
///
/// Implementations own transport, decode, buffering, and lifetime. The
/// Makepad `Video` widget drains decoded frames through this trait and reuses
/// the normal YUV upload path.
pub trait VideoFrameSession: Send {
    /// Drain newly available decoded frames.
    fn take_frames(&mut self) -> Vec<MseDecodedFrame>;

    /// Current video dimensions, if known.
    fn dimensions(&self) -> Option<(u32, u32)>;

    /// Current session lifecycle state.
    fn state(&self) -> VideoSessionState;
}

/// Opaque handle for a registered video frame session source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VideoFrameSessionId(pub u64);

static VIDEO_FRAME_SESSION_IDS: AtomicU64 = AtomicU64::new(1);
static VIDEO_FRAME_SESSIONS: OnceLock<
    Mutex<HashMap<VideoFrameSessionId, Box<dyn VideoFrameSession>>>,
> = OnceLock::new();

fn video_frame_sessions() -> &'static Mutex<HashMap<VideoFrameSessionId, Box<dyn VideoFrameSession>>>
{
    VIDEO_FRAME_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register an app-owned video frame session for later playback binding.
pub fn register_video_frame_session(session: Box<dyn VideoFrameSession>) -> VideoFrameSessionId {
    let id = VideoFrameSessionId(VIDEO_FRAME_SESSION_IDS.fetch_add(1, Ordering::Relaxed));
    video_frame_sessions().lock().unwrap().insert(id, session);
    id
}

/// Remove a registered video frame session that has not yet been consumed.
pub fn unregister_video_frame_session(
    id: VideoFrameSessionId,
) -> Option<Box<dyn VideoFrameSession>> {
    video_frame_sessions().lock().unwrap().remove(&id)
}

/// Take ownership of a registered video frame session.
///
/// This is the handoff from widget/platform plumbing into the software player.
#[doc(hidden)]
pub fn take_registered_video_frame_session(
    id: VideoFrameSessionId,
) -> Option<Box<dyn VideoFrameSession>> {
    unregister_video_frame_session(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_decode::yuv::{YuvColorMatrix, YuvLayout, YuvPlaneData};

    struct StubSession;

    impl VideoFrameSession for StubSession {
        fn take_frames(&mut self) -> Vec<MseDecodedFrame> {
            vec![MseDecodedFrame {
                track_id: 0,
                pts_ms: 123,
                yuv: YuvPlaneData {
                    y: vec![16; 4],
                    u: vec![128; 1],
                    v: vec![128; 1],
                    width: 2,
                    height: 2,
                    layout: YuvLayout::I420,
                    matrix: YuvColorMatrix::BT709,
                },
            }]
        }

        fn dimensions(&self) -> Option<(u32, u32)> {
            Some((2, 2))
        }

        fn state(&self) -> VideoSessionState {
            VideoSessionState::Active
        }
    }

    #[test]
    fn registry_roundtrip() {
        let id = register_video_frame_session(Box::new(StubSession));
        let mut session = take_registered_video_frame_session(id).expect("session missing");
        assert_eq!(session.dimensions(), Some((2, 2)));
        assert_eq!(session.state(), VideoSessionState::Active);
        assert_eq!(session.take_frames().len(), 1);
        assert!(take_registered_video_frame_session(id).is_none());
    }

    #[test]
    fn unregister_drops_pending_session() {
        let id = register_video_frame_session(Box::new(StubSession));
        assert!(unregister_video_frame_session(id).is_some());
        assert!(take_registered_video_frame_session(id).is_none());
    }
}
