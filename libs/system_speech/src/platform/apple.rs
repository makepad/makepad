//! macOS / iOS: `SFSpeechRecognizer` (on-device, PCM in) for STT and
//! `AVSpeechSynthesizer` rendered to a buffer for TTS, both called straight
//! through the Objective-C runtime (`makepad_objc_sys`); no Swift, no build
//! script. SpeechAnalyzer/SpeechTranscriber are Swift-only APIs, so the
//! Objective-C-visible recognizer is the engine here.

use crate::{
    bcp47, ListenHandle, Segment, SpeechAudio, SpeechError, SttCapabilities, SttEvent, SttOptions,
    Transcript, TtsOptions, Voice, VoiceGender,
};
use makepad_objc_sys::{
    class, msg_send, objc_block, runtime::Object, runtime::BOOL, runtime::NO, runtime::YES, sel,
    sel_impl,
};
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub(crate) const STT_ENGINE: &str = "apple-sfspeech";
pub(crate) const TTS_ENGINE: &str = "apple-avspeech";

type ObjcId = *mut Object;
const NIL: ObjcId = std::ptr::null_mut();

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}
#[link(name = "Speech", kind = "framework")]
extern "C" {}
#[link(name = "Foundation", kind = "framework")]
extern "C" {}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
}
#[link(name = "System")]
extern "C" {
    // Referenced by name from `objc_block!`.
    static _NSConcreteStackBlock: [*const c_void; 32];
}

// ------------------------------------------------------------------ helpers

/// Drains the thread's autoreleased objects on drop; worker threads have no pool.
struct Pool(ObjcId);

impl Pool {
    fn new() -> Pool {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), alloc];
            let pool: ObjcId = msg_send![pool, init];
            Pool(pool)
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.0, drain];
        }
    }
}

fn cstring(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new(s.replace('\0', " ")).unwrap())
}

/// Autoreleased NSString.
unsafe fn ns_string(s: &str) -> ObjcId {
    let c = cstring(s);
    msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
}

unsafe fn rust_string(ns: ObjcId) -> String {
    if ns.is_null() {
        return String::new();
    }
    let ptr: *const c_char = msg_send![ns, UTF8String];
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

unsafe fn error_text(error: ObjcId) -> String {
    if error.is_null() {
        return "unknown error".to_string();
    }
    let desc: ObjcId = msg_send![error, localizedDescription];
    rust_string(desc)
}

unsafe fn responds(obj: ObjcId, sel: makepad_objc_sys::runtime::Sel) -> bool {
    let r: BOOL = msg_send![obj, respondsToSelector: sel];
    r != NO
}

fn on_main_thread() -> bool {
    unsafe {
        let r: BOOL = msg_send![class!(NSThread), isMainThread];
        r != NO
    }
}

/// Wait until `done` is set, up to `timeout`. A caller on the main thread must
/// pump its run loop (the callbacks may be queued there) instead of blocking.
fn wait_for(state: &(Mutex<bool>, Condvar), timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    if on_main_thread() {
        loop {
            if *state.0.lock().unwrap() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, 0);
            }
        }
    }
    let mut done = state.0.lock().unwrap();
    while !*done {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        done = state.1.wait_timeout(done, deadline - now).unwrap().0;
    }
    true
}

fn signal(state: &(Mutex<bool>, Condvar)) {
    *state.0.lock().unwrap() = true;
    state.1.notify_all();
}

// --------------------------------------------------------------------- STT

pub(crate) fn stt_available() -> bool {
    true
}

pub(crate) fn stt_capabilities() -> SttCapabilities {
    SttCapabilities { pcm_input: true, engine_mic: false, partial_results: false, offline: true }
}

/// SFSpeechRecognizerAuthorizationStatus: 0 notDetermined, 1 denied, 2 restricted, 3 authorized.
const AUTH_AUTHORIZED: isize = 3;
const AUTH_NOT_DETERMINED: isize = 0;

/// Ask for speech-recognition permission when it was never decided, then
/// report whether it is granted.
unsafe fn authorized() -> bool {
    let mut status: isize = msg_send![class!(SFSpeechRecognizer), authorizationStatus];
    if status == AUTH_NOT_DETERMINED {
        let state = Arc::new((Mutex::new(false), Condvar::new()));
        let s = state.clone();
        let block = objc_block!(move |_status: isize| {
            signal(&s);
        });
        let _: () = msg_send![class!(SFSpeechRecognizer), requestAuthorization: &block];
        wait_for(&state, Duration::from_secs(60));
        status = msg_send![class!(SFSpeechRecognizer), authorizationStatus];
    }
    status == AUTH_AUTHORIZED
}

/// A retained recognizer for `language` (BCP-47), or nil when the locale is unsupported.
unsafe fn make_recognizer(language: &str) -> ObjcId {
    let locale: ObjcId = msg_send![class!(NSLocale), localeWithLocaleIdentifier: ns_string(language)];
    let recognizer: ObjcId = msg_send![class!(SFSpeechRecognizer), alloc];
    msg_send![recognizer, initWithLocale: locale]
}

unsafe fn release(obj: ObjcId) {
    if !obj.is_null() {
        let _: () = msg_send![obj, release];
    }
}

pub(crate) fn stt_prepare(language: &str) -> Result<(), SpeechError> {
    let _pool = Pool::new();
    unsafe {
        let recognizer = make_recognizer(&bcp47(language));
        if recognizer.is_null() {
            return Err(SpeechError::Unsupported("language not supported by the Apple recognizer"));
        }
        let ok = authorized();
        release(recognizer);
        if !ok {
            return Err(SpeechError::PermissionDenied);
        }
    }
    Ok(())
}

/// What the result handler hands back: the final transcription, or a failure.
struct SttOutcome {
    segments: Vec<Segment>,
    error: Option<String>,
}

pub(crate) fn stt_transcribe(samples_16k: &[f32], options: &SttOptions) -> Result<Transcript, SpeechError> {
    let _pool = Pool::new();
    let language = bcp47(&options.language);
    let want_timestamps = options.timestamps;
    unsafe {
        let recognizer = make_recognizer(&language);
        if recognizer.is_null() {
            return Err(SpeechError::Backend("apple stt transcribe failed (-2)".to_string()));
        }
        if !authorized() {
            release(recognizer);
            return Err(SpeechError::PermissionDenied);
        }
        let available: BOOL = msg_send![recognizer, isAvailable];
        if available == NO {
            release(recognizer);
            return Err(SpeechError::Backend("apple stt transcribe failed (-1): recognizer unavailable".to_string()));
        }

        // Deliver callbacks on a private queue, not the main one, so a worker
        // thread can block without a pumping main thread.
        let queue: ObjcId = msg_send![class!(NSOperationQueue), alloc];
        let queue: ObjcId = msg_send![queue, init];
        let _: () = msg_send![recognizer, setQueue: queue];

        let request: ObjcId = msg_send![class!(SFSpeechAudioBufferRecognitionRequest), alloc];
        let request: ObjcId = msg_send![request, init];
        let on_device: BOOL = msg_send![recognizer, supportsOnDeviceRecognition];
        if on_device != NO {
            let _: () = msg_send![request, setRequiresOnDeviceRecognition: YES];
        }
        let _: () = msg_send![request, setShouldReportPartialResults: NO];
        if responds(request, sel!(setAddsPunctuation:)) {
            let _: () = msg_send![request, setAddsPunctuation: YES];
        }

        // 16 kHz mono float PCM as one AVAudioPCMBuffer.
        let count = samples_16k.len();
        let format: ObjcId = msg_send![class!(AVAudioFormat), alloc];
        let format: ObjcId = msg_send![
            format,
            initWithCommonFormat: 1usize // AVAudioPCMFormatFloat32
            sampleRate: crate::STT_SAMPLE_RATE as f64
            channels: 1u32
            interleaved: NO
        ];
        let buffer: ObjcId = msg_send![class!(AVAudioPCMBuffer), alloc];
        let buffer: ObjcId = msg_send![buffer, initWithPCMFormat: format frameCapacity: count as u32];
        if buffer.is_null() {
            release(format);
            release(request);
            release(queue);
            release(recognizer);
            return Err(SpeechError::Backend("apple stt transcribe failed (-1): audio buffer".to_string()));
        }
        let _: () = msg_send![buffer, setFrameLength: count as u32];
        let channels: *const *mut f32 = msg_send![buffer, floatChannelData];
        std::ptr::copy_nonoverlapping(samples_16k.as_ptr(), *channels, count);
        let _: () = msg_send![request, appendAudioPCMBuffer: buffer];
        let _: () = msg_send![request, endAudio];

        let state = Arc::new((Mutex::new(false), Condvar::new()));
        let outcome = Arc::new(Mutex::new(SttOutcome { segments: Vec::new(), error: None }));
        let (s, o) = (state.clone(), outcome.clone());
        let handler = objc_block!(move |result: ObjcId, error: ObjcId| {
            unsafe {
                let mut out = o.lock().unwrap();
                if !error.is_null() {
                    out.error = Some(error_text(error));
                    drop(out);
                    signal(&s);
                    return;
                }
                if result.is_null() {
                    return;
                }
                let is_final: BOOL = msg_send![result, isFinal];
                if is_final == NO {
                    return;
                }
                let best: ObjcId = msg_send![result, bestTranscription];
                if want_timestamps {
                    let segments: ObjcId = msg_send![best, segments];
                    let n: usize = msg_send![segments, count];
                    for i in 0..n {
                        let seg: ObjcId = msg_send![segments, objectAtIndex: i];
                        let text = rust_string(msg_send![seg, substring]);
                        let start: f64 = msg_send![seg, timestamp];
                        let duration: f64 = msg_send![seg, duration];
                        if !text.is_empty() {
                            out.segments.push(Segment {
                                start_ms: (start * 1000.0) as i64,
                                end_ms: ((start + duration) * 1000.0) as i64,
                                text,
                            });
                        }
                    }
                } else {
                    let text = rust_string(msg_send![best, formattedString]);
                    if !text.is_empty() {
                        out.segments.push(Segment { start_ms: 0, end_ms: 0, text });
                    }
                }
                drop(out);
                signal(&s);
            }
        });
        let task: ObjcId = msg_send![recognizer, recognitionTaskWithRequest: request resultHandler: &handler];

        let audio_secs = count as f64 / crate::STT_SAMPLE_RATE as f64;
        let finished = wait_for(&state, Duration::from_secs_f64(30.0 + audio_secs * 4.0));
        if !finished {
            let _: () = msg_send![task, cancel];
        }
        release(buffer);
        release(format);
        release(request);
        release(queue);
        release(recognizer);

        let out = outcome.lock().unwrap();
        if let Some(err) = &out.error {
            // A silent or wordless buffer ends in an error with nothing
            // collected; report what the recognizer said.
            if out.segments.is_empty() {
                return Err(SpeechError::Backend(format!("apple stt transcribe failed (-1): {err}")));
            }
        } else if !finished {
            return Err(SpeechError::Timeout);
        }
        Ok(Transcript { segments: out.segments.clone(), language: None })
    }
}

pub(crate) fn stt_listen(_options: &SttOptions, _sink: Sender<SttEvent>) -> Result<ListenHandle, SpeechError> {
    Err(SpeechError::Unsupported("the Apple bridge takes PCM; record and call transcribe"))
}

// --------------------------------------------------------------------- TTS

pub(crate) fn tts_available() -> bool {
    true
}

pub(crate) fn tts_voices() -> Vec<Voice> {
    let _pool = Pool::new();
    unsafe {
        let voices: ObjcId = msg_send![class!(AVSpeechSynthesisVoice), speechVoices];
        if voices.is_null() {
            return Vec::new();
        }
        let n: usize = msg_send![voices, count];
        (0..n)
            .map(|i| {
                let v: ObjcId = msg_send![voices, objectAtIndex: i];
                let gender: isize = msg_send![v, gender];
                Voice {
                    id: rust_string(msg_send![v, identifier]),
                    name: rust_string(msg_send![v, name]),
                    language: rust_string(msg_send![v, language]),
                    gender: match gender {
                        // AVSpeechSynthesisVoiceGender: unspecified 0, male 1, female 2.
                        1 => VoiceGender::Male,
                        2 => VoiceGender::Female,
                        _ => VoiceGender::Unknown,
                    },
                    offline: true,
                }
            })
            .collect()
    }
}

#[derive(Default)]
struct Rendered {
    samples: Vec<f32>,
    sample_rate: f64,
}

pub(crate) fn tts_synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
    let _pool = Pool::new();
    unsafe {
        let utterance: ObjcId = msg_send![class!(AVSpeechUtterance), speechUtteranceWithString: ns_string(text)];

        let mut voice: ObjcId = NIL;
        if let Some(id) = options.voice.as_deref().filter(|v| !v.is_empty()) {
            voice = msg_send![class!(AVSpeechSynthesisVoice), voiceWithIdentifier: ns_string(id)];
        }
        if voice.is_null() {
            voice = msg_send![class!(AVSpeechSynthesisVoice), voiceWithLanguage: ns_string(&bcp47(&options.language))];
        }
        if voice.is_null() {
            voice = msg_send![class!(AVSpeechSynthesisVoice), voiceWithLanguage: ns_string("en-US")];
        }
        if !voice.is_null() {
            let _: () = msg_send![utterance, setVoice: voice];
        }
        // AVSpeechUtterance.rate is 0...1 with the default at 0.5; our 1.0 is that default.
        if options.rate > 0.0 && options.rate.is_finite() {
            let rate = (0.5 * options.rate).clamp(0.0, 1.0);
            let _: () = msg_send![utterance, setRate: rate];
        }
        if options.pitch > 0.0 && options.pitch.is_finite() {
            let pitch = options.pitch.clamp(0.5, 2.0);
            let _: () = msg_send![utterance, setPitchMultiplier: pitch];
        }

        let synthesizer: ObjcId = msg_send![class!(AVSpeechSynthesizer), alloc];
        let synthesizer: ObjcId = msg_send![synthesizer, init];
        let rendered = Arc::new(Mutex::new(Rendered::default()));
        let state = Arc::new((Mutex::new(false), Condvar::new()));
        let (r, s) = (rendered.clone(), state.clone());
        // Buffers arrive on an internal queue; a zero-length buffer terminates the run.
        let callback = objc_block!(move |buffer: ObjcId| {
            unsafe {
                let is_pcm: BOOL = msg_send![buffer, isKindOfClass: class!(AVAudioPCMBuffer)];
                if is_pcm == NO {
                    return;
                }
                let frames: u32 = msg_send![buffer, frameLength];
                if frames == 0 {
                    signal(&s);
                    return;
                }
                let frames = frames as usize;
                let format: ObjcId = msg_send![buffer, format];
                let rate: f64 = msg_send![format, sampleRate];
                let mut out = r.lock().unwrap();
                out.sample_rate = rate;
                let floats: *const *const f32 = msg_send![buffer, floatChannelData];
                if !floats.is_null() {
                    out.samples.extend_from_slice(std::slice::from_raw_parts(*floats, frames));
                } else {
                    let ints: *const *const i16 = msg_send![buffer, int16ChannelData];
                    if !ints.is_null() {
                        out.samples.extend(
                            std::slice::from_raw_parts(*ints, frames).iter().map(|&v| v as f32 / 32768.0),
                        );
                    }
                }
            }
        });
        let _: () = msg_send![synthesizer, writeUtterance: utterance toBufferCallback: &callback];

        // `write` delivers its buffers through the MAIN run loop, whichever
        // thread called it. Every UI app pumps main, so a worker just waits;
        // a caller ON main pumps instead of blocking (wait_for). A headless
        // tool calling from a worker must keep its main thread in CFRunLoopRun.
        wait_for(&state, Duration::from_secs(30));
        release(synthesizer);

        let out = rendered.lock().unwrap();
        if out.samples.is_empty() || out.sample_rate <= 0.0 {
            return Err(SpeechError::Empty);
        }
        Ok(SpeechAudio { samples: out.samples.clone(), sample_rate: out.sample_rate as f32 as u32 })
    }
}
