//! Ogg Vorbis encoder, written from scratch beside this repo's own decoder
//! (`makepad-audio-decode`), which is also its round-trip oracle. No
//! dependencies beyond that crate, no `unsafe`.
//!
//! Built for one job done fast: turning separated stems and other baked
//! audio into near-transparent ~256 kbit/s side-channel files. The shape is
//! deliberately simple — a single 1024-sample block size, floor 1 fitted to
//! a masking estimate, residue type 1 with amplitude-graded classes, and
//! per-file Huffman codebooks built from the track's own symbol counts in a
//! two-pass encode. Both passes parallelise over blocks; only pagination is
//! serial. Output is deterministic: the same PCM and options give the same
//! bytes, so content-addressed stores dedupe re-encodes for free.
//!
//! ```no_run
//! let pcm = vec![0f32; 44_100 * 2]; // one second of interleaved stereo
//! let opts = makepad_audio_encode::EncodeOptions::default();
//! let ogg = makepad_audio_encode::encode_vorbis(44_100, 2, &pcm, &opts).unwrap();
//! ```

pub mod bits;
pub mod encode;
pub mod floor_enc;
pub mod huffman;
pub mod mdct;
pub mod ogg;
pub mod psy;
pub mod setup;

pub use encode::{encode_vorbis, EncodeError, EncodeOptions};
