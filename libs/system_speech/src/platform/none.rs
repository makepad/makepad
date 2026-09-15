//! No OS speech services here (web, Apple without the Swift bridge, OpenHarmony).
//! Everything reports unavailable; nothing panics.

use crate::{
    ListenHandle, SpeechAudio, SpeechError, SttCapabilities, SttEvent, SttOptions, Transcript,
    TtsOptions, Voice,
};
use std::sync::mpsc::Sender;

pub(crate) const STT_ENGINE: &str = "none";
pub(crate) const TTS_ENGINE: &str = "none";

fn unavailable() -> SpeechError {
    SpeechError::Unavailable("no system speech engine on this platform".to_string())
}

pub(crate) fn stt_available() -> bool {
    false
}

pub(crate) fn stt_capabilities() -> SttCapabilities {
    SttCapabilities::default()
}

pub(crate) fn stt_prepare(_language: &str) -> Result<(), SpeechError> {
    Err(unavailable())
}

pub(crate) fn stt_transcribe(_samples_16k: &[f32], _options: &SttOptions) -> Result<Transcript, SpeechError> {
    Err(unavailable())
}

pub(crate) fn stt_listen(_options: &SttOptions, _sink: Sender<SttEvent>) -> Result<ListenHandle, SpeechError> {
    Err(unavailable())
}

pub(crate) fn tts_available() -> bool {
    false
}

pub(crate) fn tts_voices() -> Vec<Voice> {
    Vec::new()
}

pub(crate) fn tts_synthesize(_text: &str, _options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
    Err(unavailable())
}
