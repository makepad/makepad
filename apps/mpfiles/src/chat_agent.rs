//! The file browser's thin adapter to the shared in-process local chat engine.

use makepad_ai_hub::{
    hub::{AiHub, ChatConfig},
    hub_chat::HubChatSession,
    local_llm::{LocalLlmConfig, ToolSpec},
};
use makepad_widgets::makepad_platform::thread::SignalToUI;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub use makepad_ai_hub::local_llm::ChatEvent;

/// Where the weights live, relative to the checkout this was built from.
pub const MODEL_FILE: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";
/// The environment variable that overrides it.
pub const MODEL_ENV: &str = "MPFILES_CHAT_MODEL";

pub struct ChatAgent {
    session: HubChatSession,
}

impl ChatAgent {
    /// Start loading the model. Nothing blocks: the load happens on the hub's
    /// worker and reports itself through [`ChatAgent::poll`].
    pub fn start(model: PathBuf, system_prompt: String, tools: Vec<ToolSpec>) -> Self {
        let config = ChatConfig {
            llm: LocalLlmConfig::new(model),
            system_prompt,
            tools,
            wake: Some(Arc::new(SignalToUI::set_ui_signal)),
        };
        Self {
            session: AiHub::in_process().start_local_chat(config),
        }
    }

    pub fn send_user_turn(&self, text: String) {
        self.session.send_user_turn(text);
    }

    pub fn send_tool_results(&self, results: Vec<(String, bool)>) {
        self.session.send_tool_results(results);
    }

    pub fn cancel(&self) {
        self.session.cancel();
    }

    pub fn poll(&self) -> Vec<ChatEvent> {
        self.session.poll()
    }
}

/// Where the weights are, or `None` when this machine has none.
///
/// `MPFILES_CHAT_MODEL` wins; otherwise the file is looked for relative to the
/// working directory, then up from the binary (which finds `target/release`
/// runs from anywhere), then in the checkout this binary was compiled in.
pub fn model_path() -> Option<PathBuf> {
    if let Some(from_env) = std::env::var_os(MODEL_ENV) {
        let path = PathBuf::from(from_env);
        return path.is_file().then_some(path);
    }
    let relative = Path::new(MODEL_FILE);
    if relative.is_file() {
        return Some(relative.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe() {
        for base in exe.ancestors() {
            let candidate = base.join(relative);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join(relative))?;
    checkout.is_file().then_some(checkout)
}
