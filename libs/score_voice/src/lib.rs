//! Sung voices for a Score.
//!
//! - [`Singer`]: one realtime voice. Notes arrive with a syllable
//!   ([`PhonemeSeq`]); the voice is resynthesised from harmonics of its pitch
//!   under a per-phoneme spectral envelope (formants preserved at any pitch)
//!   plus noise bands for consonants and breath. No allocation or locks
//!   after construction; `render` adds into the host's buffer.
//! - [`VoiceProfile`]: one spectral shape per phoneme. Built in
//!   (`soprano` … `bass`), or analysed from speech (any Kokoro voice, with
//!   the `kokoro` feature) or a recorded vocal ([`analysis`]).
//! - [`Choir`]: several singers, detuned, time-jittered, with varied
//!   timbres, singing unison or harmony parts.
//! - [`lower_sing`]: notes + lyrics to sung events and a syllable table (what
//!   a Score's `sing(...)` lowers to).
//! - [`analysis`]: vocal analysis (f0, envelope, band noise) into a compact
//!   track, resynthesised re-pitched, retimed or harmonised.
//! - [`neural`] (feature `neural`): a lowered line through the neural voice
//!   (Cantor, `makepad-ai-sing`): offline renders, and the phrase renderer a
//!   live instrument plays ahead of time.

pub mod analysis;
pub mod bands;
pub mod choir;
pub mod fft;
pub mod harmony;
#[cfg(feature = "kokoro")]
pub mod kokoro;
pub mod lyrics;
#[cfg(feature = "neural")]
pub mod neural;
pub mod phoneme;
pub mod pitch;
pub mod profile;
pub mod render;
pub mod singer;
pub mod tuning;
pub mod voicer;

pub use choir::{Choir, ChoirStyle, ALL_PARTS};
pub use lyrics::{builtin_pronounce, lower_sing, ChordAt, Lowered, NoteIn, SingOpts, SungEvent, SyllableSpan};
pub use pitch::Style;
pub use profile::{Builtin, VoiceProfile};
pub use singer::{PhonemeSeq, Singer, SungNote};
