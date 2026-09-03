//! The hub object every app constructs: ephemeral AI compute and workload
//! management, in-process (aicore.md §0).
//!
//! Laws, enforced by construction:
//! - **Loopback-only, no announce**: an in-process hub opens NO listener and
//!   emits NO beacon — there is simply no socket in this module. The LAN face
//!   belongs exclusively to dedicated nodes (the `apps/ai-hub` binary /
//!   machine node) and arrives with the P3 fabric work.
//! - **No asset-server linkage**: the hub is compute; publishing generated
//!   content is the app's job through `makepad-asset-client`. Nothing here
//!   may import asset types.
//! - **The transcript lives with the app**: a session handle streams events;
//!   the hub side holds only engine state (KV warmth). Killing the hub costs
//!   a re-prefill, never a conversation (aicore.md §7).

#[cfg(feature = "llm")]
use crate::hub_chat::{HubChatConfig, HubChatSession};
#[cfg(feature = "llm")]
use crate::local_llm::{LocalLlmConfig, ToolSpec, WakeHook};
use crate::pipe::PipeId;

/// Configuration for a local-model chat session.
#[cfg(feature = "llm")]
pub struct ChatConfig {
    /// Engine limits + the GGUF to load.
    pub llm: LocalLlmConfig,
    /// The caller's system instructions.
    pub system_prompt: String,
    /// The caller's tool pack, declared to the model in its own template.
    pub tools: Vec<ToolSpec>,
    /// Wake-up for sleeping consumers (UI apps pass their platform signal).
    pub wake: Option<WakeHook>,
}

/// One in-process hub. Cheap to construct; owns no threads until a session
/// or pipe actually starts.
pub struct AiHub {
    _private: (),
}

impl AiHub {
    /// The default for every app: fully in-process, loopback-only by
    /// construction (no listener exists to bind anything else).
    pub fn in_process() -> Self {
        Self { _private: () }
    }

    /// Open the shared local registry/downloader/backend manager. Like the
    /// hub itself this binds no listener; workers start only for installs or
    /// runs.
    #[cfg(feature = "local")]
    pub fn local_models(&self) -> Result<crate::local::LocalModels, crate::AssetAiError> {
        crate::local::LocalModels::open()
    }

    /// Start a chat on this machine. The session's worker runs the machine
    /// residency election first (aicore §3): route to a serving co-located
    /// holder, wait on a loading one, else claim and load in-process.
    /// Loading reports through the returned session's `poll()`; nothing
    /// blocks.
    #[cfg(feature = "llm")]
    pub fn start_local_chat(&self, config: ChatConfig) -> HubChatSession {
        HubChatSession::start(HubChatConfig {
            llm: config.llm,
            preferred_model: None,
            system_prompt: config.system_prompt,
            tools: config.tools,
            wake: config.wake,
        })
    }

    /// Start speech-to-text. The worker picks the engine — Whisper here, on
    /// the machine node, or on a LAN node, else the OS recognizer — and
    /// reports it in `Ready`; the app feeds PCM or asks the engine to listen.
    #[cfg(feature = "speech")]
    pub fn start_stt(&self, config: crate::speech::SttConfig) -> crate::speech::SttSession {
        crate::speech::SttSession::start(config)
    }

    /// Start text-to-speech: Kokoro wherever it is, else the OS voice. Text
    /// in, PCM out; the app owns playback.
    #[cfg(feature = "speech")]
    pub fn start_tts(&self, config: crate::speech::TtsConfig) -> crate::speech::TtsSession {
        crate::speech::TtsSession::start(config)
    }

    /// The pipe id the in-process local model publishes (machine-local only).
    pub fn local_llm_pipe() -> PipeId {
        PipeId::new("llm.local")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_constructs_without_any_io() {
        let _hub = AiHub::in_process();
        assert_eq!(AiHub::local_llm_pipe().domain(), "llm");
    }
}
