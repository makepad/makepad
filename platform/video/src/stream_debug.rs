//! Opt-in trace file for the live stream codecs. `MAKEPAD_H264_DEBUG=<path>`
//! appends one flushed line per event (packet in, MFT/VideoToolbox result,
//! negotiation, frame out) so a headless service can be diagnosed from the
//! file alone — a process launched by a service wrapper has no reliable
//! stderr, and the per-packet detail is too noisy for the service log.

#[cfg(not(target_arch = "wasm32"))]
mod native {
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

    /// Keeps the first access units beside the trace (`<trace>.au0007.h264`)
    /// so a failing stream can be replayed through the decoder offline.
    pub fn dump_packet(index: u64, bytes: &[u8]) {
        let Some(path) = target() else { return };
        if index >= 64 {
            return;
        }
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".au{index:04}.h264"));
        let _ = std::fs::write(name, bytes);
    }

    /// `hr` as the mferror.h-style hex a reader can look up.
}
#[cfg(not(target_arch = "wasm32"))]
pub use native::{dump_packet, enabled, log};

/// The trace file needs a filesystem and a clock; the web has neither, so tracing is off there.
#[cfg(target_arch = "wasm32")]
pub fn enabled() -> bool {
    false
}
#[cfg(target_arch = "wasm32")]
pub fn log(_line: impl FnOnce() -> String) {}
#[cfg(target_arch = "wasm32")]
pub fn dump_packet(_index: u64, _bytes: &[u8]) {}

pub fn hex_hr(hr: i32) -> String {
    format!("0x{:08X}", hr as u32)
}

/// First bytes of a payload, for telling Annex-B (`00 00 00 01 67`) from
/// AVCC (length-prefixed) at a glance.
pub fn head(bytes: &[u8]) -> String {
    bytes.iter().take(6).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}
