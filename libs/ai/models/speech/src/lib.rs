//! Speech family: Whisper (speech-to-text), Silero VAD, Kokoro and IndexTTS-2.5
//! (text-to-speech) — pure engines, audio in / text out and text in / PCM out.
//! No platform code and no device: the OS voices live in
//! makepad-system-speech and the choice between them is makepad-ai-hub's.
//! Re-exports the shared exec surface so existing `crate::backend` /
//! `crate::emit_progress` / `crate::error` paths inside moved modules
//! keep compiling unchanged.

pub use makepad_ai_common::backend;
pub use makepad_ai_common::error;
pub use makepad_ai_common::metal_accel;
pub use makepad_ai_common::torch_pth;
pub use makepad_ai_common::{
    band_progress, emit_byte_progress, emit_progress, f16_word_to_f32, hook_ref,
    BoxedProgressHook, DiffusionError, ProgressHook, Result, BYTE_PROGRESS_STEP,
};

pub mod convert;
#[cfg(feature = "kokoro")]
pub mod g2p;
#[cfg(feature = "indextts")]
pub mod indextts;
#[cfg(feature = "indextts")]
pub mod indextts_bigvgan;
#[cfg(feature = "indextts")]
pub mod indextts_campplus;
#[cfg(feature = "indextts")]
pub mod indextts_codec;
#[cfg(feature = "indextts")]
pub mod indextts_gpt;
#[cfg(feature = "indextts")]
pub mod indextts_mel;
#[cfg(feature = "indextts")]
pub mod indextts_pipeline;
#[cfg(feature = "indextts")]
pub mod indextts_s2mel;
#[cfg(feature = "indextts")]
pub mod indextts_tokenizer;
#[cfg(feature = "indextts")]
pub mod indextts_w2v;
#[cfg(feature = "kokoro")]
pub mod kokoro;
pub mod tts;
/// Silero VAD: the 16 kHz speech gate (pure Rust port).
#[cfg(feature = "vad")]
pub mod vad;
/// Whisper speech-to-text (CPU SIMD / Metal / CUDA), formerly `makepad-voice`.
#[cfg(feature = "whisper")]
pub mod whisper;

pub use tts::{SpeechAudio, TtsError};
