//! Conversational voice pipeline for Makepad apps.
//!
//! The full loop is: microphone -> VAD gate + Whisper (the Window voice
//! plumbing in `makepad-widgets`) -> [`TranscriptFilter`] (local judge/rewrite,
//! pluggable) -> agent backend (`makepad-ai`: Claude Code, ACP, or any
//! OpenAI-compatible endpoint, including a local server) -> streamed reply ->
//! [`SpeechOutput`] speaking each sentence as it lands.
//!
//! Apps embed [`ConversePipeline`] for the whole loop, or pick parts:
//! [`SpeechOutput`] alone gives an app a voice; the filter alone gives an
//! open-mic app a "was that meant for me?" gate.

pub mod filter;
pub mod pipeline;
#[cfg(feature = "local-llm")]
pub mod qwen_filter;
pub mod speech;

pub use filter::{FilterDecision, PassthroughFilter, TranscriptFilter};
#[cfg(feature = "local-llm")]
pub use qwen_filter::QwenFilter;
pub use pipeline::{ConverseAction, ConversePipeline};
pub use speech::{spoken_text, Playback, SpeechOutput};
