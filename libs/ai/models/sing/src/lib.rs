//! Cantor: a neural singing voice.
//!
//! Score notes + lyrics -> [`score`] (phonemes aligned to notes, the f0 curve,
//! frame features) -> [`acoustic`] (phoneme encoder, conformer decoder, flow
//! refiner: a 128-bin log-mel) -> [`vocoder`] (a harmonic-plus-noise source
//! filtered in the STFT domain, so the pitch is exactly the f0 curve) -> 48 kHz
//! audio. [`nn`] is the one model definition's tensor library: inference runs
//! the same forward code the trainer differentiates.
//!
//! Design: local/agent_state/edits/design/NEURAL-VOICE.md.

pub mod data;
#[cfg(feature = "dit")]
pub mod dit;
pub mod disc;
pub mod dsp;
pub mod fft;
pub mod nn;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod nn_gpu;
pub mod acoustic;
pub mod cantor;
pub mod layers;
pub mod phonemes;
pub mod score;
pub mod train;
pub mod vocoder;
pub mod weights;

pub use cantor::{Cantor, F0Mode, RenderOpts};

/// Attribution for the data the voice is trained on: shown on the host's
/// about screen and stored in a shipped voice asset's metadata. The full
/// record (citations, checksums) is CREDITS.md.
pub const CREDITS: &str = "Cantor singing voice, trained on: \
VocalSet (J. Wilkins, P. Seetharaman, A. Wahl, B. Pardo, ISMIR 2018; https://zenodo.org/records/1442513; CC BY 4.0) and \
LibriTTS (H. Zen, V. Dang, R. Clark, Y. Zhang, R. J. Weiss, Y. Jia, Z. Chen, Y. Wu, Interspeech 2019; https://www.openslr.org/60/; CC BY 4.0). \
Modified: resampled, segmented and annotated to train the voice.";
