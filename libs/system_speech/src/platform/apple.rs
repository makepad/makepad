//! macOS / iOS: `SpeechAnalyzer` for STT (PCM in) and `AVSpeechSynthesizer`
//! rendered to a buffer for TTS, both through `swift/*.swift` (symbols `mss_*`).

use crate::{
    bcp47, ListenHandle, Segment, SpeechAudio, SpeechError, SttCapabilities, SttEvent, SttOptions,
    Transcript, TtsOptions, Voice, VoiceGender,
};
use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_float, c_int};
use std::sync::mpsc::Sender;

pub(crate) const STT_ENGINE: &str = "apple-speechanalyzer";
pub(crate) const TTS_ENGINE: &str = "apple-avspeech";

/// Mirrors `MssSegment` in stt_bridge.swift.
#[repr(C)]
struct MssSegment {
    text: *mut c_char,
    start_ms: i64,
    end_ms: i64,
}

/// Mirrors `MssVoice` in tts_bridge.swift.
#[repr(C)]
struct MssVoice {
    id: *mut c_char,
    name: *mut c_char,
    language: *mut c_char,
    gender: i32,
}

extern "C" {
    fn mss_stt_transcribe(
        samples: *const f32,
        sample_count: i64,
        lang: *const c_char,
        want_timestamps: i32,
        out_count: *mut i32,
        out_segments: *mut *mut c_void,
    ) -> i32;
    fn mss_stt_free_segments(ptr: *mut c_void, count: i32);
    fn mss_stt_prepare(lang: *const c_char) -> i32;

    fn mss_tts_synthesize(
        text: *const c_char,
        voice: *const c_char,
        language: *const c_char,
        rate: c_float,
        pitch: c_float,
        out_len: *mut c_int,
        out_rate: *mut c_float,
    ) -> *mut c_float;
    fn mss_tts_free(ptr: *mut c_float);
    fn mss_tts_voices(out_count: *mut i32) -> *mut c_void;
    fn mss_tts_free_voices(ptr: *mut c_void, count: i32);
}

fn cstring(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new(s.replace('\0', " ")).unwrap())
}

unsafe fn owned_str(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

pub(crate) fn stt_available() -> bool {
    true
}

pub(crate) fn stt_capabilities() -> SttCapabilities {
    SttCapabilities { pcm_input: true, engine_mic: false, partial_results: false, offline: true }
}

pub(crate) fn stt_prepare(language: &str) -> Result<(), SpeechError> {
    let lang = cstring(&bcp47(language));
    match unsafe { mss_stt_prepare(lang.as_ptr()) } {
        0 => Ok(()),
        -2 => Err(SpeechError::Unsupported("language not supported by the Apple recognizer")),
        code => Err(SpeechError::Backend(format!("apple stt prepare failed ({code})"))),
    }
}

pub(crate) fn stt_transcribe(samples_16k: &[f32], options: &SttOptions) -> Result<Transcript, SpeechError> {
    let lang = cstring(&bcp47(&options.language));
    let mut count: i32 = 0;
    let mut raw: *mut c_void = std::ptr::null_mut();
    let ret = unsafe {
        mss_stt_transcribe(
            samples_16k.as_ptr(),
            samples_16k.len() as i64,
            lang.as_ptr(),
            options.timestamps as i32,
            &mut count,
            &mut raw,
        )
    };
    if ret != 0 {
        return Err(SpeechError::Backend(format!("apple stt transcribe failed ({ret})")));
    }
    if count <= 0 || raw.is_null() {
        return Ok(Transcript::default());
    }
    let segments = unsafe {
        let ptr = raw as *const MssSegment;
        let out = (0..count as usize)
            .map(|i| {
                let cs = &*ptr.add(i);
                Segment { start_ms: cs.start_ms, end_ms: cs.end_ms, text: owned_str(cs.text) }
            })
            .collect();
        mss_stt_free_segments(raw, count);
        out
    };
    Ok(Transcript { segments })
}

pub(crate) fn stt_listen(_options: &SttOptions, _sink: Sender<SttEvent>) -> Result<ListenHandle, SpeechError> {
    Err(SpeechError::Unsupported("the Apple bridge takes PCM; record and call transcribe"))
}

pub(crate) fn tts_available() -> bool {
    true
}

pub(crate) fn tts_voices() -> Vec<Voice> {
    let mut count: i32 = 0;
    let raw = unsafe { mss_tts_voices(&mut count) };
    if raw.is_null() || count <= 0 {
        return Vec::new();
    }
    unsafe {
        let ptr = raw as *const MssVoice;
        let voices = (0..count as usize)
            .map(|i| {
                let v = &*ptr.add(i);
                Voice {
                    id: owned_str(v.id),
                    name: owned_str(v.name),
                    language: owned_str(v.language),
                    gender: match v.gender {
                        // AVSpeechSynthesisVoiceGender: unspecified 0, male 1, female 2.
                        1 => VoiceGender::Male,
                        2 => VoiceGender::Female,
                        _ => VoiceGender::Unknown,
                    },
                    offline: true,
                }
            })
            .collect();
        mss_tts_free_voices(raw, count);
        voices
    }
}

pub(crate) fn tts_synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
    let text = cstring(text);
    let voice = options.voice.as_deref().filter(|v| !v.is_empty()).map(cstring);
    let language = cstring(&bcp47(&options.language));
    let mut len: c_int = 0;
    let mut sample_rate: c_float = 0.0;
    // Safety: the bridge returns null or a buffer of `len` floats it allocated
    // and we free right after copying; every CString outlives the call.
    let samples = unsafe {
        let ptr = mss_tts_synthesize(
            text.as_ptr(),
            voice.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            language.as_ptr(),
            options.rate,
            options.pitch,
            &mut len,
            &mut sample_rate,
        );
        if ptr.is_null() || len <= 0 {
            return Err(SpeechError::Empty);
        }
        let samples = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        mss_tts_free(ptr);
        samples
    };
    Ok(SpeechAudio { samples, sample_rate: sample_rate as u32 })
}
