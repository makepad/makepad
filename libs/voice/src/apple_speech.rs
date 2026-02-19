use crate::decode_loop::{Segment, WhisperParams};
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;

/// C-compatible segment layout matching Swift CSegment struct.
/// Must stay in sync with speech_bridge.swift.
#[repr(C)]
struct CSegment {
    text: *mut c_char,
    start_ms: i64,
    end_ms: i64,
}

extern "C" {
    /// Swift @_cdecl uses OpaquePointer for the segments pointer,
    /// which maps to void* in C. We cast on the Rust side.
    fn apple_speech_transcribe(
        samples: *const f32,
        sample_count: i64,
        lang: *const c_char,
        out_count: *mut i32,
        out_segments: *mut *mut c_void,
    ) -> i32;

    fn apple_speech_free_segments(ptr: *mut c_void, count: i32);

    fn apple_speech_ensure_model(lang: *const c_char) -> i32;
}

/// Transcribe PCM audio (f32, 16kHz, mono) using Apple SpeechAnalyzer.
/// Returns segments with timestamps, matching the same `Segment` type as the CPU Whisper backend.
pub fn transcribe(samples: &[f32], params: &WhisperParams) -> Vec<Segment> {
    let lang =
        CString::new(params.language.as_str()).unwrap_or_else(|_| CString::new("en").unwrap());
    let mut count: i32 = 0;
    let mut raw_ptr: *mut c_void = std::ptr::null_mut();

    let ret = unsafe {
        apple_speech_transcribe(
            samples.as_ptr(),
            samples.len() as i64,
            lang.as_ptr(),
            &mut count,
            &mut raw_ptr,
        )
    };

    if ret != 0 || count <= 0 || raw_ptr.is_null() {
        return Vec::new();
    }

    let ptr = raw_ptr as *mut CSegment;

    let segments = unsafe {
        (0..count as usize)
            .map(|i| {
                let cs = &*ptr.add(i);
                let text = if cs.text.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(cs.text).to_string_lossy().into_owned()
                };
                Segment {
                    start_ms: cs.start_ms,
                    end_ms: cs.end_ms,
                    text,
                }
            })
            .collect()
    };

    unsafe {
        apple_speech_free_segments(raw_ptr, count);
    }

    segments
}

/// Ensure the speech model for a language is downloaded.
/// Call this before the first transcription for a given language.
/// Returns Ok(()) if the model is ready, Err(()) on failure.
pub fn ensure_model(language: &str) -> Result<(), ()> {
    let lang = CString::new(language).unwrap_or_else(|_| CString::new("en").unwrap());
    let ret = unsafe { apple_speech_ensure_model(lang.as_ptr()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(())
    }
}
