//! Web chat surface: the UI remains present, while provider execution is
//! reported unavailable because it requires native workers and credentials.

use crate::transcript::{ChatData, ChatRole};
use makepad_ai_hub::providers::provider::ChatProvider;
use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_client::dto::{ChatProviderKind, ChatToolOutcomeDto};
use makepad_asset_client::json::Value;
use makepad_asset_client::{ApiEndpoints, ChatAttachment};
use makepad_widgets::makepad_platform::thread::ThreadSpawner;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct FeedConfig {
    pub endpoints: ApiEndpoints,
    pub token: Option<String>,
    pub cache: PathBuf,
    pub namespace: String,
    pub client: String,
    pub provider: ChatProviderKind,
    pub provider_label: String,
    pub provider_factory:
        Option<Arc<dyn Fn() -> Box<dyn ChatProvider> + Send + Sync>>,
}

impl FeedConfig {
    pub fn new(
        endpoints: ApiEndpoints,
        token: Option<String>,
        cache: PathBuf,
        namespace: impl Into<String>,
        client: impl Into<String>,
    ) -> Self {
        Self {
            endpoints,
            token,
            cache,
            namespace: namespace.into(),
            client: client.into(),
            provider: ChatProviderKind::FleetQwen,
            provider_label: "Qwen".to_string(),
            provider_factory: None,
        }
    }
}

pub trait ClientTools: Send {
    fn execute(&mut self, name: &str, args: &Value) -> ToolOutcome;

    fn call_title(&mut self, name: &str, args: &Value) -> String {
        default_call_title(name, args)
    }

    fn outcome_summary(&mut self, name: &str, outcome: &ChatToolOutcomeDto) -> (String, String) {
        default_outcome_summary(name, outcome)
    }

    fn session_opened(&mut self) {}
}

pub struct NoClientTools;

impl ClientTools for NoClientTools {
    fn execute(&mut self, name: &str, _args: &Value) -> ToolOutcome {
        ToolOutcome::Unavailable {
            reason: format!("'{name}' is unavailable on web"),
        }
    }
}

pub struct ChatFeed {
    dirty: AtomicBool,
}

impl ChatFeed {
    pub fn start(
        _cfg: FeedConfig,
        _tools: Box<dyn ClientTools>,
        _spawner: ThreadSpawner,
    ) -> Self {
        ChatData::set_status("AI chat unavailable on web");
        Self { dirty: AtomicBool::new(true) }
    }

    pub fn send(&self, _text: String, _attachments: Vec<ChatAttachment>) {
        ChatData::push(ChatRole::System, "AI chat is unavailable on web");
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn cancel(&self) {}

    pub fn clear(&self) {
        ChatData::clear();
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn is_connected(&self) -> bool {
        false
    }

    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }
}

pub fn default_call_title(name: &str, args: &Value) -> String {
    match name {
        "assets.query" => {
            let sql = args.get("sql").and_then(Value::as_str).unwrap_or("");
            format!(
                "queried: {}",
                ellipsis(&sql.split_whitespace().collect::<Vec<_>>().join(" "), 72)
            )
        }
        "assets.schema" => "read the catalog schema".to_string(),
        "asset.search" => format!(
            "searched: {}",
            ellipsis(args.get("query").and_then(Value::as_str).unwrap_or(""), 60)
        ),
        "asset.inspect" => "inspected an asset".to_string(),
        other => other.to_string(),
    }
}

pub fn default_outcome_summary(name: &str, outcome: &ChatToolOutcomeDto) -> (String, String) {
    let base = match name {
        "assets.query" => "queried",
        "assets.schema" => "read the catalog schema",
        "asset.search" => "searched",
        "asset.inspect" => "inspected",
        other => other,
    };
    match outcome {
        ChatToolOutcomeDto::Ok { value } => {
            let note = value
                .get("rows")
                .and_then(Value::as_i64)
                .map(|count| format!(" → {count} rows"))
                .unwrap_or_default();
            let body = value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| value.to_json());
            (format!("{base}{note}"), format!("\n{body}"))
        }
        ChatToolOutcomeDto::Failed { message } => {
            (format!("{base} — failed"), format!("\nfailed: {message}"))
        }
        ChatToolOutcomeDto::Refused { what } => {
            (format!("{base} — refused"), format!("\nrefused: {what}"))
        }
        ChatToolOutcomeDto::Denied { what } => {
            (format!("{base} — denied"), format!("\ndenied: {what}"))
        }
        ChatToolOutcomeDto::Unavailable { reason } => (
            format!("{base} — unavailable"),
            format!("\nunavailable: {reason}"),
        ),
    }
}

pub fn ellipsis(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}
