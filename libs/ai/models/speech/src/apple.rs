//! `AVSpeechSynthesizer` rendered to a buffer, via `swift/tts_bridge.swift`.

use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int};

use crate::SpeechAudio;

extern "C" {
    fn apple_tts_synthesize(
        text: *const c_char,
        voice: *const c_char,
        rate: c_float,
        out_len: *mut c_int,
        out_rate: *mut c_float,
    ) -> *mut c_float;

    fn apple_tts_free(ptr: *mut c_float);
}

/// Render `text` to mono PCM. `rate` follows `AVSpeechUtterance.rate`
/// (0.5 is the system default); pass `0.0` to leave it alone.
pub fn synthesize(text: &str, voice: Option<&str>, rate: f32) -> Option<SpeechAudio> {
    let text = CString::new(text).ok()?;
    let voice = voice.and_then(|id| CString::new(id).ok());
    let voice_ptr = voice
        .as_ref()
        .map_or(std::ptr::null(), |id| id.as_ptr());

    let mut len: c_int = 0;
    let mut sample_rate: c_float = 0.0;

    // Safety: the bridge either returns null or a buffer of `len` floats that it
    // allocated and we free below. `text`/`voice` outlive the call.
    let samples = unsafe {
        let ptr = apple_tts_synthesize(text.as_ptr(), voice_ptr, rate, &mut len, &mut sample_rate);
        if ptr.is_null() || len <= 0 {
            return None;
        }
        let samples = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        apple_tts_free(ptr);
        samples
    };

    Some(SpeechAudio {
        samples,
        sample_rate: sample_rate as u32,
    })
}
