mod accel;
#[path = "cuda/backend.rs"]
mod cuda_backend;
#[path = "cpu/decode_loop.rs"]
mod decode_loop;
#[path = "cpu/decoder.rs"]
mod decoder;
#[path = "cpu/encoder.rs"]
mod encoder;
#[path = "cpu/mel.rs"]
mod mel;
#[path = "metal/backend.rs"]
mod metal_backend;
#[path = "cpu/model.rs"]
mod model;
#[path = "cpu/quant.rs"]
mod quant;
mod settings;
#[path = "cpu/tensor.rs"]
mod tensor;
mod transcriber;
mod vad;

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(force_whisper)))]
#[path = "apple/speech.rs"]
pub mod apple_speech;

pub use accel::{backend_name as accel_backend_name, set_enabled as set_accel_enabled};
pub use decode_loop::{Segment, WhisperParams, WhisperState};
pub use model::WhisperModel;
pub use transcriber::{
    NativeAppleTranscriber, VoiceBackendKind, VoiceTranscribeError, VoiceTranscribeParams,
    VoiceTranscriber, WhisperTranscriber,
};
pub use vad::{
    vad_model_path_if_present, SileroVad, VadError, VadStream, VAD_CHUNK_SAMPLES,
    VAD_SAMPLE_RATE,
};

use std::sync::atomic::{AtomicU64, Ordering};

// --- token trace ----------------------------------------------------------
//
// Off unless `MAKEPAD_VOICE_TOKEN_TRACE` is set. When on, every greedily
// sampled token id is appended to a process-wide buffer so a parity harness
// can compare backends at the token level instead of only on rendered text.
// Zero cost and zero behaviour change when off.

static TOKEN_TRACE: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

fn token_trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_VOICE_TOKEN_TRACE").is_some())
}

pub(crate) fn record_token(id: i32) {
    if !token_trace_enabled() {
        return;
    }
    if let Ok(mut buf) = TOKEN_TRACE.lock() {
        buf.push(id);
    }
}

/// Drain the sampled-token trace collected since the last call. Empty unless
/// `MAKEPAD_VOICE_TOKEN_TRACE` is set in the environment.
pub fn take_token_trace() -> Vec<i32> {
    match TOKEN_TRACE.lock() {
        Ok(mut buf) => std::mem::take(&mut *buf),
        Err(_) => Vec::new(),
    }
}

pub(crate) static PROF_MATMUL_RAW: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MATMUL_RAW_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MATMUL_T: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MATMUL_T_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENCODER: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENC_ATTN: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENC_CONV: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_ENC_ELEM: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_DECODER: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_DECODER_CALLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_CROSS_KV: AtomicU64 = AtomicU64::new(0);
pub(crate) static PROF_MEL: AtomicU64 = AtomicU64::new(0);

pub fn reset_profiling() {
    PROF_MATMUL_RAW.store(0, Ordering::Relaxed);
    PROF_MATMUL_RAW_CALLS.store(0, Ordering::Relaxed);
    PROF_MATMUL_T.store(0, Ordering::Relaxed);
    PROF_MATMUL_T_CALLS.store(0, Ordering::Relaxed);
    PROF_ENCODER.store(0, Ordering::Relaxed);
    PROF_ENC_ATTN.store(0, Ordering::Relaxed);
    PROF_ENC_CONV.store(0, Ordering::Relaxed);
    PROF_ENC_ELEM.store(0, Ordering::Relaxed);
    PROF_DECODER.store(0, Ordering::Relaxed);
    PROF_DECODER_CALLS.store(0, Ordering::Relaxed);
    PROF_CROSS_KV.store(0, Ordering::Relaxed);
    PROF_MEL.store(0, Ordering::Relaxed);
}

pub fn print_profiling() {
    let to_ms = |v: &AtomicU64| v.load(Ordering::Relaxed) as f64 / 1_000_000.0;
    eprintln!("--- profiling ---");
    eprintln!("  mel:            {:.1}ms", to_ms(&PROF_MEL));
    eprintln!("  encoder:        {:.1}ms", to_ms(&PROF_ENCODER));
    eprintln!("    enc_conv:     {:.1}ms", to_ms(&PROF_ENC_CONV));
    eprintln!("    enc_attn:     {:.1}ms", to_ms(&PROF_ENC_ATTN));
    eprintln!("    enc_elem:     {:.1}ms", to_ms(&PROF_ENC_ELEM));
    eprintln!("  cross_kv:       {:.1}ms", to_ms(&PROF_CROSS_KV));
    eprintln!(
        "  decoder:        {:.1}ms ({} calls)",
        to_ms(&PROF_DECODER),
        PROF_DECODER_CALLS.load(Ordering::Relaxed)
    );
    eprintln!(
        "  matmul_raw:     {:.1}ms ({} calls)",
        to_ms(&PROF_MATMUL_RAW),
        PROF_MATMUL_RAW_CALLS.load(Ordering::Relaxed)
    );
    eprintln!(
        "  matmul_t:       {:.1}ms ({} calls)",
        to_ms(&PROF_MATMUL_T),
        PROF_MATMUL_T_CALLS.load(Ordering::Relaxed)
    );
}
