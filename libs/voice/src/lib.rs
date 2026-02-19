mod decode_loop;
mod decoder;
mod encoder;
mod mel;
mod metal_backend;
mod model;
mod quant;
mod tensor;
mod transcriber;

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(force_whisper)))]
pub mod apple_speech;

pub use decode_loop::{Segment, WhisperParams, WhisperState};
pub use model::WhisperModel;
pub use transcriber::{
    NativeAppleTranscriber, VoiceBackendKind, VoiceTranscribeError, VoiceTranscribeParams,
    VoiceTranscriber, WhisperTranscriber,
};

use std::sync::atomic::{AtomicU64, Ordering};

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
