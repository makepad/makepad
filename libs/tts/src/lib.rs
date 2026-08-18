//! Facade over `makepad-ai-speech` so converse/route keep the `makepad_tts`
//! API after kokoro moved into the speech family (aiarch.md §1 / §9).

pub use makepad_ai_speech::convert;
pub use makepad_ai_speech::g2p;
pub use makepad_ai_speech::kokoro;
pub use makepad_ai_speech::tts::{Speaker, SpeechAudio, TtsBackend, TtsError};
