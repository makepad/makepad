//! Re-export the generic Makepad video frame session seam.
//!
//! The trait now lives in `makepad-platform` so widgets, platform playback,
//! media backends, and application code all share one generic session type.

pub use makepad_platform::{VideoFrameSession, VideoFrameSessionId, VideoSessionState};
