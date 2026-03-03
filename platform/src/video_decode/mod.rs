//! Software AV1 video decoding via vendored dav1d.
//!
//! - `dav1d_ffi`: Minimal FFI bindings for dav1d C library
//! - `mp4_demux`: Pure Rust MP4 demuxer for AV1 sample extraction
//! - `yuv`: YUV→RGBA pixel format conversion
//! - `software_av1`: Complete software AV1 player implementation

pub mod dav1d_ffi;
pub mod mp4_demux;
pub mod software_av1;
pub mod yuv;
