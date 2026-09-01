//! Android: `android.speech.SpeechRecognizer` for STT and
//! `android.speech.tts.TextToSpeech` for TTS, both reached through
//! `MakepadSpeech.java`, which hangs off `MakepadActivity`.
//!
//! Minimum API level is 26, so the recognizer is the mic-owning
//! `SpeechRecognizer` + `EXTRA_PREFER_OFFLINE` (API 23) rather than
//! `createOnDeviceSpeechRecognizer` (API 31), and TTS renders through
//! `synthesizeToFile(CharSequence, Bundle, File, String)` (API 21) rather than
//! the `ParcelFileDescriptor` overload (API 30).
//!
//! Every function here blocks and belongs on a worker thread: the Java side
//! does its work on the main looper (both engines require it) and parks the
//! caller on a latch, so calling from the main thread would deadlock.

use crate::{
    bcp47, ListenHandle, SpeechAudio, SpeechError, SttCapabilities, SttEvent, SttOptions,
    Transcript, TtsOptions, Voice, VoiceGender,
};
use makepad_android_state::{get_activity, get_java_vm};
use makepad_jni_sys as jni;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

pub(crate) const STT_ENGINE: &str = "android-speechrecognizer";
pub(crate) const TTS_ENGINE: &str = "android-texttospeech";

// ------------------------------------------------------------------ JNI glue

/// Attach this (worker) thread to the VM. Threads created by Rust are unknown
/// to the JVM until attached, and stay attached for the process' life.
unsafe fn attach_env() -> Option<*mut jni::JNIEnv> {
    let vm = get_java_vm();
    if vm.is_null() {
        return None;
    }
    let mut env: *mut jni::JNIEnv = std::ptr::null_mut();
    let attach = (**vm).AttachCurrentThread?;
    if attach(vm, &mut env, std::ptr::null_mut()) != 0 || env.is_null() {
        return None;
    }
    Some(env)
}

/// A pending Java exception poisons every later JNI call on this thread, so it
/// is logged and cleared at each boundary rather than carried across.
unsafe fn clear_exception(env: *mut jni::JNIEnv) {
    let (Some(check), Some(describe), Some(clear)) =
        ((**env).ExceptionCheck, (**env).ExceptionDescribe, (**env).ExceptionClear)
    else {
        return;
    };
    if check(env) != 0 {
        describe(env);
        clear(env);
    }
}

/// Resolve a method on the activity's *own* class. A natively attached thread
/// carries only the system class loader, so `FindClass("dev/makepad/...")`
/// cannot see app classes from here; `GetObjectClass(activity)` always can.
unsafe fn activity_method(
    env: *mut jni::JNIEnv,
    name: &str,
    sig: &str,
) -> Option<(jni::jobject, jni::jmethodID)> {
    let activity = get_activity();
    if activity.is_null() {
        return None;
    }
    let class = ((**env).GetObjectClass?)(env, activity);
    if class.is_null() {
        clear_exception(env);
        return None;
    }
    let name = CString::new(name).ok()?;
    let sig = CString::new(sig).ok()?;
    let method = ((**env).GetMethodID?)(env, class, name.as_ptr(), sig.as_ptr());
    if let Some(delete) = (**env).DeleteLocalRef {
        delete(env, class);
    }
    if method.is_null() {
        // GetMethodID throws NoSuchMethodError when the Java half is older.
        clear_exception(env);
        return None;
    }
    Some((activity, method))
}

// The `A` (jvalue-array) call forms are used throughout: the variadic forms
// take C promotion rules that Rust does not apply, which silently mangles
// `float` and `boolean` arguments on aarch64.

unsafe fn call_activity_void(
    env: *mut jni::JNIEnv,
    name: &str,
    sig: &str,
    args: &[jni::jvalue],
) -> bool {
    let Some((activity, method)) = activity_method(env, name, sig) else {
        return false;
    };
    let Some(call) = (**env).CallVoidMethodA else {
        return false;
    };
    call(env, activity, method, args.as_ptr());
    clear_exception(env);
    true
}

unsafe fn call_activity_bool(env: *mut jni::JNIEnv, name: &str, sig: &str) -> bool {
    let Some((activity, method)) = activity_method(env, name, sig) else {
        return false;
    };
    let Some(call) = (**env).CallBooleanMethodA else {
        return false;
    };
    let result = call(env, activity, method, std::ptr::null());
    clear_exception(env);
    result != 0
}

unsafe fn call_activity_object(
    env: *mut jni::JNIEnv,
    name: &str,
    sig: &str,
    args: &[jni::jvalue],
) -> jni::jobject {
    let Some((activity, method)) = activity_method(env, name, sig) else {
        return std::ptr::null_mut();
    };
    let Some(call) = (**env).CallObjectMethodA else {
        return std::ptr::null_mut();
    };
    let result = call(env, activity, method, args.as_ptr());
    clear_exception(env);
    result
}

unsafe fn delete_local_ref(env: *mut jni::JNIEnv, object: jni::jobject) {
    if object.is_null() {
        return;
    }
    if let Some(delete) = (**env).DeleteLocalRef {
        delete(env, object);
    }
}

unsafe fn new_jstring(env: *mut jni::JNIEnv, text: &str) -> jni::jstring {
    let text = CString::new(text)
        .unwrap_or_else(|_| CString::new(text.replace('\0', " ")).unwrap_or_default());
    match (**env).NewStringUTF {
        Some(new) => new(env, text.as_ptr()),
        None => std::ptr::null_mut(),
    }
}

unsafe fn jstring_to_string(env: *mut jni::JNIEnv, text: jni::jstring) -> String {
    if text.is_null() {
        return String::new();
    }
    let Some(get) = (**env).GetStringUTFChars else {
        return String::new();
    };
    let chars = get(env, text, std::ptr::null_mut());
    if chars.is_null() {
        clear_exception(env);
        return String::new();
    }
    let out = CStr::from_ptr(chars).to_string_lossy().into_owned();
    if let Some(release) = (**env).ReleaseStringUTFChars {
        release(env, text, chars);
    }
    out
}

unsafe fn jbyte_array_to_vec(env: *mut jni::JNIEnv, array: jni::jbyteArray) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    let (Some(length_of), Some(region)) = ((**env).GetArrayLength, (**env).GetByteArrayRegion)
    else {
        return Vec::new();
    };
    let length = length_of(env, array);
    if length <= 0 {
        return Vec::new();
    }
    let mut out = vec![0u8; length as usize];
    region(env, array, 0, length, out.as_mut_ptr() as *mut jni::jbyte);
    clear_exception(env);
    out
}

// ------------------------------------------------------------------- session

/// Live `listen` sessions. The Java half calls back on the main looper while
/// the Rust caller is off elsewhere, so the sinks live in a global map keyed by
/// a session id rather than travelling through JNI as a pointer.
fn sinks() -> &'static Mutex<HashMap<u64, Sender<SttEvent>>> {
    static SINKS: OnceLock<Mutex<HashMap<u64, Sender<SttEvent>>>> = OnceLock::new();
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn recognizer_error(word: &str) -> SpeechError {
    match word {
        "permission" => SpeechError::PermissionDenied,
        "" => SpeechError::Backend("android recognizer failed".to_string()),
        other => SpeechError::Backend(other.to_string()),
    }
}

/// `MakepadSpeech.onSttEvent`. `kind`: 0 level, 1 partial, 2 final, 3 error,
/// 4 ended — the Java half sends exactly one `ended` per session, last.
#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadSpeech_onSttEvent(
    env: *mut jni::JNIEnv,
    _class: jni::jclass,
    session: jni::jlong,
    kind: jni::jint,
    text: jni::jstring,
    level: jni::jfloat,
) {
    let session = session as u64;
    let text = jstring_to_string(env, text);
    let Ok(mut sinks) = sinks().lock() else {
        return;
    };
    if kind == 4 {
        // Ended retires the session: the sink is dropped here, which is what
        // tells a receiver blocked on the channel that the utterance is over.
        if let Some(sink) = sinks.remove(&session) {
            let _ = sink.send(SttEvent::Ended);
        }
        return;
    }
    let Some(sink) = sinks.get(&session) else {
        return;
    };
    let event = match kind {
        0 => SttEvent::Level(level.clamp(0.0, 1.0)),
        1 => SttEvent::Partial(text),
        2 => SttEvent::Final(Transcript::from_text(text)),
        3 => SttEvent::Error(recognizer_error(text.trim())),
        _ => return,
    };
    let _ = sink.send(event);
}

// ----------------------------------------------------------------------- STT

pub(crate) fn stt_available() -> bool {
    unsafe {
        let Some(env) = attach_env() else {
            return false;
        };
        call_activity_bool(env, "speechSttAvailable", "()Z")
    }
}

pub(crate) fn stt_capabilities() -> SttCapabilities {
    SttCapabilities {
        // `SpeechRecognizer` owns the microphone itself; there is no PCM input.
        pcm_input: false,
        engine_mic: true,
        partial_results: true,
        // EXTRA_PREFER_OFFLINE is honoured where an on-device model exists;
        // an engine without one still recognizes over the network.
        offline: true,
    }
}

pub(crate) fn stt_prepare(_language: &str) -> Result<(), SpeechError> {
    if stt_available() {
        Ok(())
    } else {
        Err(SpeechError::Unavailable("no android recognition service installed".to_string()))
    }
}

pub(crate) fn stt_transcribe(
    _samples_16k: &[f32],
    _options: &SttOptions,
) -> Result<Transcript, SpeechError> {
    Err(SpeechError::Unsupported(
        "the Android recognizer only listens on the microphone; use listen",
    ))
}

pub(crate) fn stt_listen(
    options: &SttOptions,
    sink: Sender<SttEvent>,
) -> Result<ListenHandle, SpeechError> {
    if !stt_available() {
        return Err(SpeechError::Unavailable(
            "no android recognition service installed".to_string(),
        ));
    }
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    let language = bcp47(&options.language);
    let partial = options.partial_results;
    let prefer_offline = options.prefer_offline;

    match sinks().lock() {
        Ok(mut sinks) => {
            sinks.insert(session, sink);
        }
        Err(_) => return Err(SpeechError::Backend("speech session registry poisoned".to_string())),
    }

    let started = unsafe {
        match attach_env() {
            Some(env) => {
                let language = new_jstring(env, &language);
                let args = [
                    jni::jvalue { j: session as jni::jlong },
                    jni::jvalue { l: language },
                    jni::jvalue { z: partial as jni::jboolean },
                    jni::jvalue { z: prefer_offline as jni::jboolean },
                ];
                let ok = call_activity_void(
                    env,
                    "speechSttStart",
                    "(JLjava/lang/String;ZZ)V",
                    &args,
                );
                delete_local_ref(env, language);
                ok
            }
            None => false,
        }
    };
    if !started {
        if let Ok(mut sinks) = sinks().lock() {
            sinks.remove(&session);
        }
        return Err(SpeechError::Unavailable("android speech bridge missing".to_string()));
    }

    Ok(ListenHandle::new(move || unsafe {
        if let Some(env) = attach_env() {
            let args = [jni::jvalue { j: session as jni::jlong }];
            call_activity_void(env, "speechSttStop", "(J)V", &args);
        }
    }))
}

// ----------------------------------------------------------------------- TTS

pub(crate) fn tts_available() -> bool {
    unsafe {
        let Some(env) = attach_env() else {
            return false;
        };
        call_activity_bool(env, "speechTtsAvailable", "()Z")
    }
}

pub(crate) fn tts_voices() -> Vec<Voice> {
    unsafe {
        let Some(env) = attach_env() else {
            return Vec::new();
        };
        let array =
            call_activity_object(env, "speechTtsVoices", "()[Ljava/lang/String;", &[]);
        if array.is_null() {
            return Vec::new();
        }
        let (Some(length_of), Some(element_of)) =
            ((**env).GetArrayLength, (**env).GetObjectArrayElement)
        else {
            delete_local_ref(env, array);
            return Vec::new();
        };
        let length = length_of(env, array);
        let mut voices = Vec::new();
        for index in 0..length {
            let item = element_of(env, array, index);
            let line = jstring_to_string(env, item);
            delete_local_ref(env, item);
            // "name\tlanguageTag\tqualityInt\tnetworkRequiredBool"
            let mut fields = line.split('\t');
            let (Some(name), Some(language), Some(_quality), Some(network)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            voices.push(Voice {
                id: name.to_string(),
                name: name.to_string(),
                language: language.to_string(),
                // Android's Voice carries no gender; the name often hints at
                // one, but guessing from it would be a lie.
                gender: VoiceGender::Unknown,
                offline: !network.eq_ignore_ascii_case("true"),
            });
        }
        delete_local_ref(env, array);
        voices
    }
}

pub(crate) fn tts_synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
    unsafe {
        let Some(env) = attach_env() else {
            return Err(SpeechError::Unavailable("no java vm on this thread".to_string()));
        };
        let text = new_jstring(env, text);
        let voice = match options.voice.as_deref().filter(|v| !v.is_empty()) {
            Some(voice) => new_jstring(env, voice),
            None => std::ptr::null_mut(),
        };
        let language = new_jstring(env, &bcp47(&options.language));
        let args = [
            jni::jvalue { l: text },
            jni::jvalue { l: voice },
            jni::jvalue { l: language },
            jni::jvalue { f: options.rate },
            jni::jvalue { f: options.pitch },
        ];
        let wav = call_activity_object(
            env,
            "speechTtsSynthesize",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;FF)[B",
            &args,
        );
        delete_local_ref(env, text);
        delete_local_ref(env, voice);
        delete_local_ref(env, language);

        if wav.is_null() {
            let last_error =
                call_activity_object(env, "speechTtsLastError", "()Ljava/lang/String;", &[]);
            let reason = jstring_to_string(env, last_error);
            delete_local_ref(env, last_error);
            let reason = if reason.trim().is_empty() {
                "android tts produced no audio".to_string()
            } else {
                reason
            };
            return Err(SpeechError::Backend(reason));
        }
        let bytes = jbyte_array_to_vec(env, wav);
        delete_local_ref(env, wav);
        if bytes.is_empty() {
            return Err(SpeechError::Empty);
        }
        crate::wav::decode(&bytes).map_err(SpeechError::Backend)
    }
}
