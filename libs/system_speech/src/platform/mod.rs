//! One module per OS, all implementing the same nine items. `none` is the
//! reference: every function here must be safe to call on any platform.
//!
//! Contract (implemented by each platform file with these exact signatures):
//! ```ignore
//! pub(crate) const STT_ENGINE: &str;
//! pub(crate) const TTS_ENGINE: &str;
//! pub(crate) fn stt_available() -> bool;
//! pub(crate) fn stt_capabilities() -> SttCapabilities;
//! pub(crate) fn stt_prepare(language: &str) -> Result<(), SpeechError>;
//! pub(crate) fn stt_transcribe(samples_16k: &[f32], options: &SttOptions) -> Result<Transcript, SpeechError>;
//! pub(crate) fn stt_listen(options: &SttOptions, sink: Sender<SttEvent>) -> Result<ListenHandle, SpeechError>;
//! pub(crate) fn tts_available() -> bool;
//! pub(crate) fn tts_voices() -> Vec<Voice>;
//! pub(crate) fn tts_synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError>;
//! ```

#[cfg(all(any(target_os = "macos", target_os = "ios"), apple_speech))]
mod apple;
#[cfg(all(any(target_os = "macos", target_os = "ios"), apple_speech))]
pub(crate) use apple::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub(crate) use android::*;

#[cfg(all(target_os = "linux", not(target_os = "android")))]
mod linux;
#[cfg(all(target_os = "linux", not(target_os = "android")))]
pub(crate) use linux::*;

#[cfg(not(any(
    all(any(target_os = "macos", target_os = "ios"), apple_speech),
    windows,
    target_os = "android",
    all(target_os = "linux", not(target_os = "android")),
)))]
mod none;
#[cfg(not(any(
    all(any(target_os = "macos", target_os = "ios"), apple_speech),
    windows,
    target_os = "android",
    all(target_os = "linux", not(target_os = "android")),
)))]
pub(crate) use none::*;
