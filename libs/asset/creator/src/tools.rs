//! The creator tool pack: the chat executor for a store that only stores
//! (aicore §9 / P8).
//!
//! Wraps the hardened catalog executor (`AssetServerTools`: search, inspect,
//! schema, assets.query — all reads over the app's own client) and takes
//! over what the store's queue used to own:
//!
//! - `content.generate` — the games' one closed generation entry — executes
//!   HERE through [`crate::runner`]: ETA-picked fleet node, the worker's own
//!   publish dressing, a catalog row the session can hand back by revision.
//! - the `operation.*` transform tools answer typed `Unavailable` naming
//!   where transforms live now. Honest and designed: an outcome, never an
//!   error string — the model reads it and says so.

use crate::runner::{self, PublishTarget};
use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::session::{CancelFlag, ExecCtx, ToolExecutor};
use makepad_asset_chat::tools::{ContentToolCall, ToolDef};
use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::ApiEndpoints;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct CreatorTools {
    inner: Result<AssetServerTools, String>,
    target: PublishTarget,
}

impl CreatorTools {
    pub fn connect(
        endpoints: ApiEndpoints,
        token: Option<String>,
        namespace: impl Into<String>,
    ) -> CreatorTools {
        let namespace = namespace.into();
        CreatorTools {
            inner: AssetServerTools::connect(endpoints.clone(), token.clone(), namespace.clone())
                .map_err(|e| e.to_string()),
            target: PublishTarget { endpoints, token, namespace },
        }
    }

    pub fn store_error(&self) -> Option<&str> {
        self.inner.as_ref().err().map(|s| s.as_str())
    }

    fn content_generate(
        &mut self,
        kind: makepad_asset_chat::tools::ContentGenerateKind,
        prompt: &str,
        dim_height: Option<f64>,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome {
        let kind_name = format!("{}.generate", kind.as_str());
        let mut body = vec![("prompt", json::s(prompt.to_string()))];
        if let Some(h) = dim_height {
            body.push(("height", Value::F64(h)));
        }
        let body = json::obj(body);
        let seed = prompt
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        let flag = Arc::new(AtomicBool::new(false));
        // Bridge the session's cancel flag: checked each progress beat.
        let mut beat = |note: &str, permille: u16| {
            if cancel.is_cancelled() {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            progress(permille, note);
        };
        match runner::generate_and_publish(&kind_name, &body, seed, &self.target, &flag, &mut beat)
        {
            Ok(generated) => ToolOutcome::Ok {
                value: json::obj(vec![
                    ("kind", json::s(kind_name)),
                    (
                        "asset_id",
                        generated.asset_id.map(json::s).unwrap_or(Value::Null),
                    ),
                    (
                        "revision",
                        generated.revision.map(json::s).unwrap_or(Value::Null),
                    ),
                    ("text", generated.text.map(json::s).unwrap_or(Value::Null)),
                ]),
            },
            Err(error) if error == "cancelled" => {
                ToolOutcome::Failed { message: "cancelled".to_string() }
            }
            Err(error) => ToolOutcome::Failed { message: error },
        }
    }
}

const TRANSFORMS_MOVED: &str = "asset transforms run in the creator apps now (the store only \
     stores); ask the person to run the enhancement from their app";

impl ToolExecutor for CreatorTools {
    fn capability_doc(&mut self) -> String {
        match &mut self.inner {
            Ok(tools) => tools.capability_doc(),
            Err(error) => format!("the asset server is unreachable: {error}"),
        }
    }

    fn tool_definitions(&mut self) -> Vec<ToolDef> {
        match &mut self.inner {
            Ok(tools) => tools.tool_definitions(),
            Err(_) => makepad_asset_chat::tools::definitions(),
        }
    }

    fn client_executes(&mut self, call: &ContentToolCall) -> bool {
        match &mut self.inner {
            Ok(tools) => tools.client_executes(call),
            Err(_) => false,
        }
    }

    fn execute(
        &mut self,
        call: &ContentToolCall,
        ctx: &ExecCtx,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome {
        match call {
            ContentToolCall::ContentGenerate { kind, prompt, dim_height } => {
                self.content_generate(*kind, prompt, *dim_height, progress, cancel)
            }
            ContentToolCall::OperationCreate { .. }
            | ContentToolCall::OperationGet { .. }
            | ContentToolCall::OperationWait { .. }
            | ContentToolCall::OperationCancel { .. }
            | ContentToolCall::OperationRetry { .. }
            | ContentToolCall::OperationCapabilities => ToolOutcome::Unavailable {
                reason: TRANSFORMS_MOVED.to_string(),
            },
            other => match &mut self.inner {
                Ok(tools) => tools.execute(other, ctx, progress, cancel),
                Err(error) => ToolOutcome::Unavailable {
                    reason: format!("the asset server is unreachable: {error}"),
                },
            },
        }
    }
}
