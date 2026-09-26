//! Capture one application's output and play it back through a processor.
//!
//! The host picks a process by pid or bundle id, and this crate taps that
//! process, silences its direct path to the speakers while the tap is read,
//! and writes the processor's samples to an output device. A mixer opens one
//! route per application. The processor is the equalizer: it runs on the
//! audio thread and must not allocate, lock, or block.
//!
//! macOS 14.2 and later implement this with a Core Audio process tap
//! (`macos.rs`), Windows 10 2004 (build 19041) and later with WASAPI process
//! loopback (`windows.rs`), Linux through the PulseAudio API (PulseAudio or
//! PipeWire's pipewire-pulse) by moving the application's streams onto a
//! sink of the route's own (`linux.rs`). Other platforms expose the same types and return
//! [`Error::Unsupported`] until they grow a backend. On macOS the embedding
//! application needs `NSAudioCaptureUsageDescription` in its Info.plist; the
//! first route open asks the user for system audio recording permission.
//! A [`Source::BundleId`] is the application's macOS bundle id everywhere;
//! each backend maps it to its platform's name for the application.

mod analysis;
mod dsp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

pub use analysis::{Analyzer, AnalyzerHandle};
pub use dsp::{Equalizer, EqualizerHandle, FnProcessor, Gain, GainHandle, Limiter, Passthrough};

use std::fmt;

/// An application `coreaudiod` is willing to tap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioProcess {
    pub pid: u32,
    pub bundle_id: String,
    pub name: String,
    pub output_running: bool,
    pub input_running: bool,
}

/// A device a route can play to. `uid` is what [`RouteConfig::output`] takes.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputDevice {
    pub uid: String,
    pub name: String,
    pub channels: u16,
    pub sample_rate: f64,
}

/// Which process to tap. A bundle id follows the application across relaunch
/// where the platform supports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Pid(u32),
    BundleId(String),
}

/// What happens to the tapped application's own path to the hardware
/// while the route is processing ([`RouteConfig::processing`]); while it
/// monitors, the application always plays by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mute {
    /// The application keeps playing. The route is a copy.
    HearOriginal,
    /// The hardware never hears the application. Only this route's output does.
    Replace,
    /// The hardware hears the application until this route is reading it.
    ReplaceWhileRouted,
}

/// One tap, mixed by the system from every listed source, played to one device.
#[derive(Clone, Debug)]
pub struct RouteConfig {
    pub sources: Vec<Source>,
    pub mute: Mute,
    /// `None` is the default output device.
    pub output: Option<String>,
    /// The route's mode, changed later with `Route::set_processing`, the
    /// same on every platform:
    ///
    /// * `false`, **monitoring**: the application plays through its own
    ///   output untouched; the processor runs on a copy (for pictures and
    ///   meters) and nothing it leaves is played.
    /// * `true`, **processing**: the application's own path is silenced
    ///   (per [`Mute`]) and the processor's output is played instead.
    ///
    /// Switching crossfades where the platform allows it.
    pub processing: bool,
    /// Where a backend that has to change the application's own settings
    /// to silence it (Windows ducks its audio session) keeps what it
    /// changed, so [`restore_after_crash`] can put it back after the host
    /// died mid-route. `None` keeps nothing.
    pub state_dir: Option<std::path::PathBuf>,
}

/// What the audio thread tells the processor about the block it was handed.
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub sample_rate: f64,
    pub channels: u16,
    pub frames: usize,
    pub host_time: u64,
}

/// In-place processing of one block. `frames` is interleaved, `info.channels`
/// wide, and already holds the tapped audio. Whatever is left in `frames` is
/// what gets played.
pub trait Processor: Send {
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo);
}

#[derive(Debug)]
pub enum Error {
    /// No backend on this platform yet.
    Unsupported,
    /// None of the sources is an audio client, and none can be followed.
    NotFound,
    /// The audio system rejected a step. `status` is the OS code.
    System { status: i32, step: &'static str },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported => write!(f, "audio routing is not implemented on this platform"),
            Error::NotFound => write!(f, "no audio client matches the requested source"),
            Error::System { status, step } => write!(f, "{step} failed ({status})"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(target_os = "macos")]
pub use macos::{default_output, outputs, permission, processes, Route};
#[cfg(target_os = "windows")]
pub use windows::{default_output, outputs, permission, processes, restore_after_crash, Route};

/// Put back what a route that did not close (the host crashed or was
/// killed while processing) changed on the tapped application, as kept in
/// `state_dir` ([`RouteConfig::state_dir`]). Call it at startup. Only
/// Windows and Linux (both duck the application's volume) change anything
/// that outlives the route.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn restore_after_crash(_state_dir: &std::path::Path) {}
#[cfg(target_os = "linux")]
pub use linux::{default_output, outputs, permission, processes, restore_after_crash, Route};

/// Whether the system lets this app hear other apps ([`permission`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    /// Not asked yet.
    Unknown,
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn permission() -> Option<Permission> {
    None
}

/// A route that is not open. Other platforms keep the type so a host compiles.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub struct Route {
    _private: (),
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
impl Route {
    pub fn open(_config: RouteConfig, _processor: Box<dyn Processor>) -> Result<Self> {
        Err(Error::Unsupported)
    }

    pub fn set_processor(&self, _processor: Box<dyn Processor>) {}

    pub fn set_processing(&self, _processing: bool) -> Result<()> {
        Ok(())
    }

    pub fn processing(&self) -> bool {
        false
    }

    pub fn reclaim(&self) {}

    pub fn sample_rate(&self) -> f64 {
        0.0
    }

    pub fn channels(&self) -> u16 {
        0
    }

    pub fn describe(&self) -> String {
        String::new()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn processes() -> Result<Vec<AudioProcess>> {
    Err(Error::Unsupported)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn outputs() -> Result<Vec<OutputDevice>> {
    Err(Error::Unsupported)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn default_output() -> Result<OutputDevice> {
    Err(Error::Unsupported)
}
