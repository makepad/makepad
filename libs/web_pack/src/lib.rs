//! Packing a Makepad wasm app for static hosting.
//!
//! The pieces a packer needs that are not the app's own: the platform's
//! web runtime stripped to the sections the app uses ([`js`]), the output
//! folder ([`output`]), font coverage classification ([`coverage`]),
//! the crunch knobs ([`pack`]), the collect manifest ([`manifest`]),
//! brotli, and a small static server ([`serve`]). Stage's web export and
//! `cargo makepad wasm --pack` use them.

pub mod coverage;
pub mod js;
pub mod output;
pub mod pack;
pub mod serve;

/// The collect manifest a packer reads (written by a collect run).
pub use makepad_web_manifest as manifest;

use std::io::Write;

/// Brotli at quality 11 with a 16 MiB window (what a static host would
/// serve).
pub fn brotli(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 1 << 16, 11, 24);
        w.write_all(bytes).expect("writing to memory");
    }
    out
}

/// Brotli at quality 9 (the sizes a report shows per part: within a few
/// percent of quality 11, many times faster on large inputs).
pub fn brotli_estimate(bytes: &[u8]) -> usize {
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 1 << 16, 9, 22);
        w.write_all(bytes).expect("writing to memory");
    }
    out.len()
}

/// Bytes as megabytes, for reports.
pub fn mb(bytes: usize) -> String {
    format!("{:.2} MB", bytes as f64 / 1e6)
}
