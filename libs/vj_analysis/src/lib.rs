//! Headless entry point for the VJ's native track analysis.
//!
//! The algorithm modules below live in this crate, compiled against a small
//! value-type seam so command-line bakers run the track analysis without
//! linking or starting any UI.

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

pub mod preprocess {
    use std::path::PathBuf;

    pub const WAVE_SUBDIR: &str = "wave-cache";

    /// A headless baker has no operator-chosen cache root, so the sidecars
    /// go where `VJ_WAVE_CACHE` or the data root says.
    pub fn cache_subdir(_subdir: &str) -> Option<PathBuf> {
        None
    }
}

pub mod service {
    use std::path::PathBuf;

    /// `VJ_ASSET_CACHE` when it names a directory, otherwise `local/vj` at
    /// the root of the checkout. An empty variable is not a choice.
    pub fn data_root() -> PathBuf {
        match std::env::var("VJ_ASSET_CACHE") {
            Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir.trim()),
            _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/vj"),
        }
    }
}

pub mod media {
    use crate::mixer::TrackPcm;
    use makepad_asset_data::MediaType;
    use std::path::{Path, PathBuf};

    /// The container a local path is decoded as, by its name alone; a name
    /// no in-process decoder knows is handed on as `Mp4`.
    pub fn local_media_type(path: &Path) -> MediaType {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        match extension.as_deref() {
            Some("wav" | "wave") => MediaType::Wav,
            Some("ogg" | "oga") => MediaType::Ogg,
            Some("mp3") => MediaType::Mp3,
            _ => MediaType::Mp4,
        }
    }

    /// Whether the sound of a container is cut to the edit its index
    /// declares. Off unless `VJ_CONTAINER_EDIT_TRIM=1`.
    pub fn container_trim_enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| {
            std::env::var("VJ_CONTAINER_EDIT_TRIM").map(|value| value.trim() == "1").unwrap_or(false)
        })
    }

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

pub mod dsp_math;
pub mod loudness;
pub mod track_key;
pub mod beat_sync;
pub mod wave_analysis;
pub mod loop_splat;
