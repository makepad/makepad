//! Headless entry point for the VJ's native track analysis.
//!
//! The algorithm modules below are the app's source files, compiled here
//! against their small value-type seam so command-line bakers run precisely
//! the code a native deck runs without linking or starting the VJ UI.

pub mod decks {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum DeckId {
        A,
        B,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct LoopSpan {
        pub start_secs: f64,
        pub end_secs: f64,
    }
}

pub mod mixer {
    /// A fully decoded, immutable PCM clip (interleaved stereo i16).
    pub struct TrackPcm {
        pub frames: Vec<[i16; 2]>,
        pub sample_rate: u32,
    }

    impl TrackPcm {
        pub fn seconds(&self) -> f64 {
            if self.sample_rate == 0 {
                return 0.0;
            }
            self.frames.len() as f64 / self.sample_rate as f64
        }
    }
}

pub mod music_dsp {
    pub const STEM_COUNT: usize = 4;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(usize)]
    pub enum StemKind {
        Vocals = 0,
        Drums = 1,
        Bass = 2,
        Other = 3,
    }

    impl StemKind {
        pub const ALL: [StemKind; STEM_COUNT] =
            [StemKind::Vocals, StemKind::Drums, StemKind::Bass, StemKind::Other];

        pub fn index(self) -> usize {
            self as usize
        }
    }
}

pub mod clock {
    use std::time::Duration;

    #[derive(Clone, Copy, Debug)]
    pub struct Instant(std::time::Instant);

    impl Instant {
        pub fn now() -> Self {
            Self(std::time::Instant::now())
        }

        pub fn elapsed(self) -> Duration {
            self.0.elapsed()
        }
    }
}

pub mod media {
    use crate::mixer::TrackPcm;
    use makepad_asset_data::MediaType;
    use std::path::PathBuf;

    /// The headless baker already owns decoded PCM and never takes the VJ's
    /// platform-media fallback path.
    pub fn decode_audio_clip(
        _path: &PathBuf,
        _media: MediaType,
        _max_frames: usize,
    ) -> Result<TrackPcm, String> {
        Err("platform media decode is unavailable in the headless analysis crate".into())
    }
}

#[path = "../../../apps/vj/src/beat_sync.rs"]
pub mod beat_sync;
#[path = "../../../apps/vj/src/wave_analysis.rs"]
pub mod wave_analysis;
#[path = "../../../apps/vj/src/loop_splat.rs"]
pub mod loop_splat;
