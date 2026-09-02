//! Opt-in trace file for the live stream codecs. `MAKEPAD_H264_DEBUG=<path>`
//! appends one flushed line per event (packet in, MFT/VideoToolbox result,
//! negotiation, frame out) so a headless service can be diagnosed from the
//! file alone — a process launched by a service wrapper has no reliable
//! stderr, and the per-packet detail is too noisy for the service log.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

fn target() -> Option<&'static PathBuf> {
    static TARGET: OnceLock<Option<PathBuf>> = OnceLock::new();
    TARGET.get_or_init(|| std::env::var_os("MAKEPAD_H264_DEBUG").map(PathBuf::from)).as_ref()
}

fn started() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

pub fn enabled() -> bool {
    target().is_some()
}

/// Appends one line; the closure only runs when tracing is on.
pub fn log(line: impl FnOnce() -> String) {
    let Some(path) = target() else { return };
    let text = line();
    let ms = started().elapsed().as_millis();
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{ms:>8} {text}");
        let _ = file.flush();
    }
}

/// `hr` as the mferror.h-style hex a reader can look up.
pub fn hex_hr(hr: i32) -> String {
    format!("0x{:08X}", hr as u32)
}

/// First bytes of a payload, for telling Annex-B (`00 00 00 01 67`) from
/// AVCC (length-prefixed) at a glance.
pub fn head(bytes: &[u8]) -> String {
    bytes.iter().take(6).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}
