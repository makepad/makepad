//! Tracker, chiptune and game music: the formats the demoscene and the
//! game consoles made their music in, read into the MIDI library's
//! [`Arrangement`](crate::arrangement::Arrangement)
//! so that its text form, its Score writer, its index and its tools serve
//! them as they serve MIDI.
//!
//! - [`tracker`]: MOD, S3M, XM and IT modules, and their replay into notes
//!   with every slide, vibrato, arpeggio and volume change.
//! - [`sid`]: C64 SID tunes run on our own 6502 in a small C64, the SID
//!   chip's register writes read back as notes on SID-style patches.
//! - [`archive`]: 7z (LZMA, LZMA2, PPMd, BCJ2), the collections' format.
//! - [`import`]: any of them as a Score (File > Open comes here), and the
//!   original as its own player plays it, for an A/B.

pub mod archive;
pub mod cli;
pub mod compare;
pub mod gbs;
pub mod hvsc;
pub mod import;
pub mod lr35902;
pub mod nsf;
pub mod shape;
pub mod sid;
pub mod spc;
pub mod spc700;
pub mod tracker;
pub mod vgm;
pub mod voices;
pub mod web;
pub mod zophar;
