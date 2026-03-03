//! Software AV1 video decoding via pure-Rust `rav1d`.
//!
//! - `rav1d`: Minimal decoder wrapper over the rav1d crate
//! - `mp4_demux`: Pure Rust MP4 demuxer for AV1 sample extraction
//! - `yuv`: YUV→RGBA pixel format conversion
//! - `software_av1`: Complete software AV1 player implementation

pub mod mp4_demux;
pub mod rav1d;
pub mod software_av1;
pub mod yuv;
