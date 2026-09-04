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

use crate::runner::{CreateError, FleetTransport, GenerationTransport, PublishTarget};
use crate::composite::{self, Publisher, Recipe};
use makepad_asset_chat::dispatch::AssetServerTools;
use makepad_asset_chat::session::{CancelFlag, ExecCtx, ToolExecutor};
use makepad_asset_chat::tools::{ContentToolCall, ToolDef};
use makepad_asset_chat::wire::ToolOutcome;
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::ApiEndpoints;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct CreatorTools {
    inner: Result<AssetServerTools, String>,
    transport: Box<dyn GenerationTransport>,
    publisher: Box<dyn Publisher>,
    cancel_signal: Option<Arc<AtomicBool>>,
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
            transport: Box::new(FleetTransport),
            publisher: Box::new(PublishTarget { endpoints, token, namespace }),
            cancel_signal: None,
        }
    }

    pub fn store_error(&self) -> Option<&str> {
        self.inner.as_ref().err().map(|s| s.as_str())
    }

    /// Inject only I/O; routing recipes, job ownership and result validation
    /// remain the exact executor used in shipping sessions.
    pub fn with_runtime(transport: Box<dyn GenerationTransport>, publisher: Box<dyn Publisher>) -> Self {
        Self { inner: Err("no catalog attached to injected runtime".into()), transport, publisher, cancel_signal: None }
    }

    /// UI cancellation must reach a blocking job without waiting in its worker queue.
    pub fn with_cancel_signal(mut self, signal: Arc<AtomicBool>) -> Self {
        self.cancel_signal = Some(signal);
        self
    }

    fn generate(&self, recipe: Result<Recipe, CreateError>, progress: &mut dyn FnMut(u16, &str), cancel: &CancelFlag) -> ToolOutcome {
        static NEXT_RUN: AtomicU64 = AtomicU64::new(0);
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
            ^ NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        let cancel = ToolCancel { session: cancel, signal: self.cancel_signal.as_deref() };
        let result = recipe.and_then(|recipe| composite::run(&recipe, seed, self.transport.as_ref(),
            self.publisher.as_ref(), &cancel, &mut |note, n| progress(n, note), Duration::from_millis(500)));
        match result {
            Ok(result) => {
                let last = &result.stages.last().expect("nonempty recipe").output;
                let optional = |value: &Option<String>| value.clone().map(json::s).unwrap_or(Value::Null);
                let stages = result.stages.iter().map(|stage| json::obj(vec![
                    ("stage", json::s(stage.domain.clone())), ("job_id", json::s(stage.job_id.clone())),
                    ("model", json::s(stage.model.clone())), ("asset_id", optional(&stage.output.asset_id)),
                    ("revision", optional(&stage.output.revision)), ("alias", optional(&stage.output.alias)),
                    ("text", optional(&stage.output.text)),
                ])).collect();
                let character = result.character.map(|c| json::obj(vec![
                    ("skinned", Value::Bool(c.skinned)), ("rigged", Value::Bool(c.skinned)),
                    ("animated", Value::Bool(c.animated)), ("embedded_atlas", Value::Bool(c.embedded_atlas)),
                    ("playable", Value::Bool(c.playable)),
                    ("clips", Value::Arr(c.clips.into_iter().map(json::s).collect())),
                ])).unwrap_or(Value::Null);
                ToolOutcome::Ok { value: json::obj(vec![
                    ("asset_id", optional(&last.asset_id)), ("revision", optional(&last.revision)),
                    ("alias", optional(&last.alias)), ("stages", Value::Arr(stages)),
                    ("character", character),
                    ("dim_height", result.dim_height.map(Value::F64).unwrap_or(Value::Null)),
                    ("placement", json::s("not placed; use the alias with world.spawn or world.set_player_model; dim_height is an intended size, not an applied mesh transform")),
                ]) }
            }
            Err(CreateError::Unavailable(reason)) => ToolOutcome::Unavailable { reason },
            Err(error) => ToolOutcome::Failed { message: makepad_asset_chat::wire::sanitize_public_error(&error.to_string()) },
        }
    }
}

struct ToolCancel<'a> {
    session: &'a CancelFlag,
    signal: Option<&'a AtomicBool>,
}
impl crate::runner::Cancellation for ToolCancel<'_> {
    fn cancelled(&self) -> bool {
        self.session.is_cancelled() || self.signal.is_some_and(|s| s.load(Ordering::Relaxed))
    }
}

const TRANSFORMS_MOVED: &str = "asset transforms run in the creator apps now (the store only \
     stores); ask the person to run the enhancement from their app";

impl ToolExecutor for CreatorTools {
    fn capability_doc(&mut self) -> String {
        let catalog = match &mut self.inner {
            Ok(tools) => tools.capability_doc(),
            Err(error) => format!("the asset server is unreachable: {error}"),
        };
        format!("{catalog} Creator generation owns and waits for real pipeline jobs. Availability is checked against live local fleet capabilities at each stage; missing stages return Unavailable. Character: expanded brief → image → matte → mesh → rig → motion. Prop: image → mesh. Sound: audio. Each output is published and returned by alias/revision; nothing is placed automatically.")
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
        if let Some(recipe) = Recipe::for_call(call) {
            return self.generate(recipe, progress, cancel);
        }
        match call {
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
