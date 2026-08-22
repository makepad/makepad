//! Pictures of audio: a log-frequency spectrogram, an antialiased waveform
//! strip, and the composite that carries both.
//!
//! ONE definition of the recipe, shared by everything that draws sound. The
//! importer bakes these into catalog thumbnails; the preview widgets render
//! the same pictures live from decoded samples. Before this crate the ramp,
//! the window and the band lived inside the importer, so a widget could only
//! show a spectrogram somebody else had already baked.
//!
//! Zero dependencies: an iterative radix-2 FFT and a coverage rasteriser.
//! That is what lets a widget crate and a headless importer share it without
//! either dragging the other's world along.

pub mod composite;
pub mod spectrogram;
pub mod wave;

pub use composite::{composite_rgba, CompositeRegions, WAVE_STRIP_FRACTION};
pub use spectrogram::{ramp, spectrogram_rgba, HD_H, HD_W};
pub use wave::{wave_rgba, WavePalette};

/// Straight mono downmix. Every picture here is of one signal; a stereo
/// track's two channels are averaged rather than drawn twice, because what a
/// thumbnail answers is "what is this piece", not "how is it panned".
pub fn mono(frames: &[(f32, f32)]) -> Vec<f32> {
    frames.iter().map(|(l, r)| (l + r) * 0.5).collect()
}
