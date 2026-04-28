use crate::dispatch::HubEvent;
use makepad_live_id::LiveId;
use makepad_micro_serde::*;
use makepad_network::{HttpMethod, HttpRequest, NetworkConfig, NetworkResponse, NetworkRuntime};
use makepad_studio_protocol::hub_protocol::{
    AiAgentId, AiAgentState, AiAgentSummary, AiBackendInfo, AiMessage, AiMessageRole, AiMountState,
};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const LOCAL_BACKEND_ID: &str = "openai_localhost";
const CLOUD_BACKEND_ID: &str = "openai";
const DEFAULT_LOCAL_BASE_URL: &str = "http://10.0.0.217:8080/v1/chat/completions";
const DEFAULT_LOCAL_MODEL: &str = "";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";
const DEFAULT_MAX_TOKENS: u32 = 2048;
const MAX_TOOL_ROUNDS: u32 = 8;
const DEFAULT_READ_LIMIT: usize = 200;
const DEFAULT_LIST_LIMIT: usize = 200;
const DEFAULT_SEARCH_LIMIT: usize = 100;
const DEFAULT_OBSERVE_FILESYSTEM_LIMIT: usize = 50;
const MAX_OBSERVE_FILESYSTEM_LIMIT: usize = 500;
const DEFAULT_OBSERVE_FILESYSTEM_WINDOW_SECS: u64 = 300;
const MAX_OBSERVE_FILESYSTEM_WINDOW_SECS: u64 = 3600;
const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_RESULT_CHARS: usize = 16_000;
const DEFAULT_BASH_TIMEOUT_SECS: u64 = 20;
const MAX_BASH_TIMEOUT_SECS: u64 = 120;
const SYSTEM_PROMPT_TEMPLATE: &str = include_str!("../ai_mgr.md");
const AI_CHAT_PERSIST_FS_SUPPRESS: Duration = Duration::from_millis(1_500);
const AI_TASK_EVENT_PREFIX: &str = "TASK EVENT:";
const AI_TERMINAL_EXCERPT_MAX_CHARS: usize = 480;
const AI_TERMINAL_EXCERPT_MAX_LINES: usize = 10;

pub struct AiTerminalObservation {
    pub path: String,
    pub terminal_title: String,
    pub cols: u16,
    pub rows: u16,
    pub top_row: usize,
    pub total_lines: usize,
    pub is_tui: bool,
    pub text: String,
}

#[derive(Clone)]
struct AiBackendConfig {
    id: String,
    label: String,
    detail: String,
    url: String,
    model: String,
    api_key: Option<String>,
    disable_thinking_via_chat_template: bool,
}

#[derive(Clone, Debug, SerJson, DeJson)]
struct ToolCallRecord {
    id: String,
    name: String,
    arguments_json: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
enum ConversationItem {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallRecord>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Clone, Debug)]
pub struct AiToolExecutionResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub is_error: bool,
}

struct RunningAgent {
    title: String,
    backend_id: String,
    status: String,
    pending_request_id: Option<LiveId>,
    pending_tool_batch: bool,
    cancel_requested: bool,
    run_token: u64,
    tool_rounds: u32,
    messages: Vec<AiMessage>,
    history: Vec<ConversationItem>,
    updated_at: f64,
}

struct MountAgents {
    root_path: String,
    active_backend_id: String,
    active_agent_id: Option<AiAgentId>,
    next_chat_ordinal: u64,
    next_task_id: u64,
    loaded_from_disk: bool,
    order: Vec<AiAgentId>,
    agents: HashMap<AiAgentId, RunningAgent>,
    tasks: Vec<AiTrackedTask>,
    queued_followups: VecDeque<AiQueuedFollowup>,
    terminal_snapshots: HashMap<String, AiTerminalSnapshot>,
}

#[derive(Clone, Debug)]
struct AiTrackedTask {
    id: u64,
    agent_id: AiAgentId,
    goal: String,
    terminal_path: Option<String>,
    expected_paths: Vec<String>,
    touched_paths: Vec<String>,
    status: String,
    last_terminal_mode: String,
    last_terminal_summary: String,
    last_terminal_excerpt: String,
    last_codex_status: Option<String>,
}

#[derive(Clone, Debug)]
struct AiQueuedFollowup {
    agent_id: AiAgentId,
    task_id: u64,
    signature: String,
    text: String,
}

#[derive(Clone, Debug, Default)]
struct AiTerminalSnapshot {
    path: String,
    mode: &'static str,
    summary: String,
    visible_text: String,
    is_codex: bool,
    codex_status: Option<String>,
}

struct InFlightRequest {
    mount: String,
    agent_id: AiAgentId,
    run_token: u64,
    stream: StreamingTurnState,
}

#[derive(Clone, Debug, Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments_json: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct StreamVisibleState {
    thinking_message_index: Option<usize>,
    assistant_message_index: Option<usize>,
}

#[derive(Default)]
struct StreamingTurnState {
    buffer: String,
    thinking_text: String,
    assistant_text: String,
    tool_calls: Vec<ToolCallAccumulator>,
    finish_reason: Option<String>,
    done_received: bool,
    visible: StreamVisibleState,
}

#[derive(Default)]
struct StreamUpdate {
    changed: bool,
    done: bool,
}

#[derive(Clone, Debug, SerJson, DeJson)]
struct PersistedAiChat {
    version: u32,
    agent_id: AiAgentId,
    title: String,
    backend_id: String,
    active: Option<bool>,
    status: String,
    pending: bool,
    updated_at: f64,
    messages: Vec<AiMessage>,
    history: Vec<ConversationItem>,
}

#[derive(DeJson)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    error: Option<OpenAiErrorEnvelope>,
}

#[derive(DeJson)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(DeJson)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
    error: Option<OpenAiErrorEnvelope>,
}

#[derive(DeJson)]
struct OpenAiStreamChoice {
    delta: Option<OpenAiStreamDelta>,
    finish_reason: Option<String>,
}

#[derive(DeJson)]
struct OpenAiStreamDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    reasoning_text: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCallDelta>>,
}

#[derive(DeJson)]
struct OpenAiStreamToolCallDelta {
    index: Option<u32>,
    id: Option<String>,
    #[rename(type)]
    kind: Option<String>,
    function: Option<OpenAiStreamFunctionDelta>,
}

#[derive(DeJson)]
struct OpenAiStreamFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(DeJson)]
struct OpenAiResponseMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    reasoning_text: Option<String>,
    tool_calls: Option<Vec<OpenAiResponseToolCall>>,
}

#[derive(DeJson)]
struct OpenAiResponseToolCall {
    id: String,
    #[rename(type)]
    kind: Option<String>,
    function: OpenAiResponseFunctionCall,
}

#[derive(DeJson)]
struct OpenAiResponseFunctionCall {
    name: String,
    arguments: String,
}

#[derive(DeJson)]
struct OpenAiErrorEnvelope {
    message: Option<String>,
}

#[derive(DeJson)]
struct ReadFileArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(DeJson)]
struct ListFilesArgs {
    path: Option<String>,
    limit: Option<usize>,
}

#[derive(DeJson)]
struct SearchTextArgs {
    pattern: String,
    path: Option<String>,
    limit: Option<usize>,
}

#[derive(DeJson)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(DeJson)]
struct ReplaceInFileArgs {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: Option<bool>,
}

#[derive(DeJson)]
struct BashArgs {
    command: String,
    timeout_secs: Option<u64>,
}

#[derive(DeJson)]
struct ObserveFilesystemArgs {
    path: Option<String>,
    limit: Option<usize>,
    since_secs: Option<u64>,
}

#[derive(DeJson)]
struct ReadTerminalArgs {
    path: String,
    rows: Option<u16>,
    top_row: Option<usize>,
}

#[derive(DeJson)]
struct OpenTerminalArgs {
    name: Option<String>,
    command: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(DeJson)]
struct OpenEditorArgs {
    path: String,
    line: Option<usize>,
    column: Option<usize>,
}

#[derive(DeJson)]
struct SendTerminalTextArgs {
    path: String,
    text: String,
    submit: Option<bool>,
    bracketed_paste: Option<bool>,
}

#[derive(DeJson)]
struct SendTerminalKeyArgs {
    path: String,
    key: String,
    shift: Option<bool>,
    control: Option<bool>,
    alt: Option<bool>,
}

struct AssistantTurn {
    text: String,
    thinking_text: String,
    tool_calls: Vec<ToolCallRecord>,
}

pub struct AiManager {
    event_tx: Sender<HubEvent>,
    runtime: Arc<NetworkRuntime>,
    backends: Vec<AiBackendConfig>,
    mounts: HashMap<String, MountAgents>,
    inflight: HashMap<LiveId, InFlightRequest>,
    next_agent_id: u64,
    next_run_token: u64,
}

impl AiManager {
    pub fn new(event_tx: Sender<HubEvent>) -> Self {
        let runtime = Arc::new(NetworkRuntime::new(NetworkConfig::default()));
        let runtime_rx = Arc::clone(&runtime);
        let event_tx_runtime = event_tx.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        thread::spawn(move || {
            forward_runtime_events(
                runtime_rx,
                event_tx_runtime,
                Duration::from_secs(60),
                shutdown,
            );
        });

        Self {
            event_tx,
            runtime,
            backends: Self::detect_backends(),
            mounts: HashMap::new(),
            inflight: HashMap::new(),
            next_agent_id: 1,
            next_run_token: 1,
        }
    }

    pub fn register_mount(&mut self, mount: &str, root: &Path) {
        let default_backend_id = self.default_backend_id();
        let mut should_load = false;
        {
            let entry = self
                .mounts
                .entry(mount.to_string())
                .or_insert_with(|| MountAgents {
                    root_path: String::new(),
                    active_backend_id: default_backend_id.clone(),
                    active_agent_id: None,
                    next_chat_ordinal: 1,
                    next_task_id: 1,
                    loaded_from_disk: false,
                    order: Vec::new(),
                    agents: HashMap::new(),
                    tasks: Vec::new(),
                    queued_followups: VecDeque::new(),
                    terminal_snapshots: HashMap::new(),
                });
            entry.root_path = root.to_string_lossy().to_string();
            if entry.active_backend_id.is_empty() {
                entry.active_backend_id = default_backend_id.clone();
            }
            if !entry.loaded_from_disk {
                entry.loaded_from_disk = true;
                should_load = true;
            }
        }
        if should_load {
            self.load_mount_from_disk(mount);
        }
        self.ensure_default_agent(mount);
        self.persist_mount_state_best_effort(mount);
    }

    pub fn remove_mount(&mut self, mount: &str) -> AiMountState {
        if let Some(state) = self.mounts.remove(mount) {
            for agent in state.agents.into_values() {
                if let Some(request_id) = agent.pending_request_id {
                    let _ = self.runtime.http_cancel(request_id);
                    self.inflight.remove(&request_id);
                }
            }
        }
        AiMountState::default()
    }

    pub fn get_state(&mut self, mount: &str) -> AiMountState {
        self.ensure_default_agent(mount);
        self.persist_mount_state_best_effort(mount);
        self.snapshot(mount)
    }

    pub fn create_agent(&mut self, mount: &str, title: Option<String>) -> AiMountState {
        self.ensure_mount_entry(mount);
        let agent_id = self.alloc_agent_id();
        let mount_state = self.mounts.get_mut(mount).unwrap();
        let title = title
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| {
                let title = format!("Chat {}", mount_state.next_chat_ordinal);
                mount_state.next_chat_ordinal += 1;
                title
            });
        let backend_id = mount_state.active_backend_id.clone();
        mount_state.order.push(agent_id);
        mount_state.active_agent_id = Some(agent_id);
        mount_state.agents.insert(
            agent_id,
            RunningAgent {
                title,
                backend_id,
                status: "ready".to_string(),
                pending_request_id: None,
                pending_tool_batch: false,
                cancel_requested: false,
                run_token: 0,
                tool_rounds: 0,
                messages: Vec::new(),
                history: Vec::new(),
                updated_at: now_seconds(),
            },
        );
        self.persist_mount_state_best_effort(mount);
        self.snapshot(mount)
    }

    pub fn delete_agent(&mut self, mount: &str, agent_id: AiAgentId) -> AiMountState {
        self.ensure_mount_entry(mount);
        let mut removed_pending = None;
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            if let Some(agent) = mount_state.agents.remove(&agent_id) {
                removed_pending = agent.pending_request_id;
            }
            mount_state.order.retain(|existing| *existing != agent_id);
            if mount_state.active_agent_id == Some(agent_id) {
                mount_state.active_agent_id = mount_state.order.last().copied();
            }
        }
        if let Some(request_id) = removed_pending {
            let _ = self.runtime.http_cancel(request_id);
            self.inflight.remove(&request_id);
        }
        self.ensure_default_agent(mount);
        self.remove_agent_file_best_effort(mount, agent_id);
        self.persist_mount_state_best_effort(mount);
        self.snapshot(mount)
    }

    pub fn select_agent(&mut self, mount: &str, agent_id: AiAgentId) -> AiMountState {
        self.ensure_mount_entry(mount);
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            if mount_state.agents.contains_key(&agent_id) {
                mount_state.active_agent_id = Some(agent_id);
            }
        }
        self.persist_mount_state_best_effort(mount);
        self.snapshot(mount)
    }

    pub fn set_backend(&mut self, mount: &str, backend_id: &str) -> AiMountState {
        self.ensure_mount_entry(mount);
        if !self.backends.iter().any(|backend| backend.id == backend_id) {
            return self.snapshot(mount);
        }
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            mount_state.active_backend_id = backend_id.to_string();
            if let Some(agent_id) = mount_state.active_agent_id {
                if let Some(agent) = mount_state.agents.get_mut(&agent_id) {
                    agent.backend_id = backend_id.to_string();
                    if !agent.is_pending() {
                        agent.status = "ready".to_string();
                    }
                    agent.updated_at = now_seconds();
                }
            }
        }
        self.persist_mount_state_best_effort(mount);
        self.snapshot(mount)
    }

    pub fn send_prompt(&mut self, mount: &str, agent_id: AiAgentId, text: &str) -> AiMountState {
        self.ensure_mount_entry(mount);
        let prompt = text.trim();
        if prompt.is_empty() {
            return self.snapshot(mount);
        }

        let run_token = self.alloc_run_token();
        {
            let Some(agent) = self
                .mounts
                .get_mut(mount)
                .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
            else {
                return self.snapshot(mount);
            };
            if agent.is_pending() {
                return self.snapshot(mount);
            }
            if agent.messages.is_empty() && agent.title.starts_with("Chat ") {
                let summary = summarize_title(prompt);
                if !summary.is_empty() {
                    agent.title = summary;
                }
            }
            agent.messages.push(AiMessage {
                role: AiMessageRole::User,
                text: prompt.to_string(),
            });
            agent.history.push(ConversationItem::User {
                text: prompt.to_string(),
            });
            agent.pending_request_id = None;
            agent.pending_tool_batch = false;
            agent.cancel_requested = false;
            agent.run_token = run_token;
            agent.tool_rounds = 0;
            agent.status = "thinking...".to_string();
            agent.messages.push(AiMessage {
                role: AiMessageRole::Thinking,
                text: String::new(),
            });
            agent.updated_at = now_seconds();
        }

        self.note_ai_prompt_task(mount, agent_id, prompt);
        self.persist_mount_state_best_effort(mount);
        self.start_model_request(mount, agent_id, run_token);
        self.snapshot(mount)
    }

    pub fn cancel_prompt(&mut self, mount: &str, agent_id: AiAgentId) -> AiMountState {
        let request_id = self
            .mounts
            .get(mount)
            .and_then(|mount_state| mount_state.agents.get(&agent_id))
            .and_then(|agent| agent.pending_request_id);
        if let Some(request_id) = request_id {
            let _ = self.runtime.http_cancel(request_id);
            self.inflight.remove(&request_id);
        }
        if let Some(agent) = self
            .mounts
            .get_mut(mount)
            .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
        {
            agent.pending_request_id = None;
            if agent.pending_tool_batch {
                agent.cancel_requested = true;
                agent.status = "cancelling...".to_string();
            } else {
                agent.cancel_requested = false;
                agent.status = "cancelled".to_string();
            }
            agent.updated_at = now_seconds();
        }
        self.persist_mount_state_best_effort(mount);
        self.snapshot(mount)
    }

    pub fn process_terminal_observation(
        &mut self,
        mount: &str,
        observation: AiTerminalObservation,
    ) -> Option<AiMountState> {
        self.ensure_mount_entry(mount);
        let (mode, is_codex, summary, codex_status) =
            Self::terminal_mode_and_summary(&observation.terminal_title, &observation.text);
        let snapshot = AiTerminalSnapshot {
            path: observation.path.clone(),
            mode,
            summary,
            visible_text: observation.text,
            is_codex,
            codex_status,
        };

        let mut queue = Vec::new();
        let mut changed = false;
        {
            let mount_state = self.mounts.get_mut(mount)?;
            let previous = mount_state
                .terminal_snapshots
                .insert(snapshot.path.clone(), snapshot.clone());
            if previous
                .as_ref()
                .map(|previous| {
                    previous.mode != snapshot.mode
                        || previous.summary != snapshot.summary
                        || previous.codex_status != snapshot.codex_status
                        || previous.is_codex != snapshot.is_codex
                })
                .unwrap_or(true)
            {
                changed = true;
            }

            for task in mount_state
                .tasks
                .iter_mut()
                .filter(|task| task.terminal_path.as_deref() == Some(snapshot.path.as_str()))
            {
                let previous_mode = task.last_terminal_mode.clone();
                let previous_summary = task.last_terminal_summary.clone();
                let previous_excerpt = task.last_terminal_excerpt.clone();
                let previous_codex_status = task.last_codex_status.clone();

                task.status = if snapshot.mode == "needs-attention" {
                    "needs-attention".to_string()
                } else if snapshot.mode == "done" {
                    "done".to_string()
                } else {
                    "watching".to_string()
                };
                task.last_terminal_mode = snapshot.mode.to_string();
                task.last_terminal_summary = snapshot.summary.clone();
                task.last_terminal_excerpt = Self::truncate_terminal_excerpt(
                    &snapshot.visible_text,
                    AI_TERMINAL_EXCERPT_MAX_CHARS,
                    AI_TERMINAL_EXCERPT_MAX_LINES,
                );
                task.last_codex_status = snapshot.codex_status.clone();

                if previous_mode != task.last_terminal_mode
                    || previous_summary != task.last_terminal_summary
                    || previous_excerpt != task.last_terminal_excerpt
                    || previous_codex_status != task.last_codex_status
                {
                    changed = true;
                }

                if previous_mode != "needs-attention" && snapshot.mode == "needs-attention" {
                    queue.push((
                        task.id,
                        format!(
                            "terminal:{}:attention:{}",
                            snapshot.path, task.last_terminal_summary
                        ),
                        "Tracked terminal needs attention".to_string(),
                    ));
                } else if previous_mode != "done" && snapshot.mode == "done" {
                    queue.push((
                        task.id,
                        format!(
                            "terminal:{}:done:{}",
                            snapshot.path, task.last_terminal_summary
                        ),
                        "Tracked terminal appears done".to_string(),
                    ));
                }
            }
        }

        for (task_id, signature, reason) in queue {
            self.queue_ai_task_followup(mount, task_id, signature, &reason);
        }

        let dispatched = self.dispatch_next_ai_manager_followup(mount);
        if changed || dispatched {
            Some(self.snapshot(mount))
        } else {
            None
        }
    }

    pub fn process_terminal_closed(
        &mut self,
        mount: &str,
        path: &str,
        exit_code: i32,
    ) -> Option<AiMountState> {
        self.ensure_mount_entry(mount);
        let summary = format!("terminal exited ({})", exit_code);
        let snapshot = AiTerminalSnapshot {
            path: path.to_string(),
            mode: "exited",
            summary: summary.clone(),
            visible_text: String::new(),
            is_codex: false,
            codex_status: None,
        };

        let mut queue = Vec::new();
        {
            let mount_state = self.mounts.get_mut(mount)?;
            mount_state
                .terminal_snapshots
                .insert(path.to_string(), snapshot);
            for task in mount_state
                .tasks
                .iter_mut()
                .filter(|task| task.terminal_path.as_deref() == Some(path))
            {
                task.status = if exit_code == 0 {
                    "done".to_string()
                } else {
                    "needs-attention".to_string()
                };
                task.last_terminal_mode = "exited".to_string();
                task.last_terminal_summary = summary.clone();
                task.last_codex_status = None;
                queue.push((
                    task.id,
                    format!("terminal:{}:exit:{}", path, exit_code),
                    format!("Tracked terminal exited with code {}", exit_code),
                ));
            }
        }

        for (task_id, signature, reason) in queue {
            self.queue_ai_task_followup(mount, task_id, signature, &reason);
        }

        self.dispatch_next_ai_manager_followup(mount);
        Some(self.snapshot(mount))
    }

    pub fn process_path_change(&mut self, mount: &str, virtual_path: &str) -> Option<AiMountState> {
        self.ensure_mount_entry(mount);
        let relative_path = if virtual_path == mount {
            return None;
        } else {
            virtual_path
                .strip_prefix(&format!("{}/", mount))
                .unwrap_or(virtual_path)
        };

        let mut queue = Vec::new();
        let mut changed = false;
        {
            let mount_state = self.mounts.get_mut(mount)?;
            for task in &mut mount_state.tasks {
                if !matches_expected_path(relative_path, &task.expected_paths) {
                    continue;
                }
                if !task
                    .touched_paths
                    .iter()
                    .any(|existing| existing == relative_path)
                {
                    task.touched_paths.push(relative_path.to_string());
                    changed = true;
                }
                if matches!(
                    task.last_terminal_mode.as_str(),
                    "done" | "awaiting-input" | "needs-attention"
                ) {
                    queue.push((
                        task.id,
                        format!(
                            "file:{}:{}:{}",
                            relative_path, task.last_terminal_mode, task.last_terminal_summary
                        ),
                        format!("Observed filesystem change for `{}`", relative_path),
                    ));
                }
            }
        }

        for (task_id, signature, reason) in queue {
            self.queue_ai_task_followup(mount, task_id, signature, &reason);
        }

        let dispatched = self.dispatch_next_ai_manager_followup(mount);
        if changed || dispatched {
            Some(self.snapshot(mount))
        } else {
            None
        }
    }

    pub fn handle_http_response(
        &mut self,
        response: NetworkResponse,
    ) -> Option<(String, AiMountState)> {
        match response {
            NetworkResponse::HttpResponse {
                request_id,
                response,
            } => {
                let in_flight = self.inflight.remove(&request_id)?;
                let mount = in_flight.mount.clone();
                let agent_id = in_flight.agent_id;
                let run_token = in_flight.run_token;
                let body = response
                    .body
                    .as_ref()
                    .map(|body| String::from_utf8_lossy(body).to_string())
                    .unwrap_or_default();

                if !self.agent_run_matches(&mount, agent_id, run_token) {
                    return None;
                }

                if response.status_code >= 400 {
                    self.set_agent_error(
                        &mount,
                        agent_id,
                        format!(
                            "HTTP {}: {}",
                            response.status_code,
                            extract_error_text(&body)
                        ),
                    );
                    return Some((mount.clone(), self.snapshot(&mount)));
                }

                match extract_assistant_turn(&body) {
                    Ok(turn) => {
                        let state =
                            self.complete_assistant_turn(&mount, agent_id, run_token, turn, None);
                        return state.map(|state| (mount.clone(), state));
                    }
                    Err(error) => self.set_agent_error(&mount, agent_id, error),
                }
                Some((mount.clone(), self.snapshot(&mount)))
            }
            NetworkResponse::HttpStreamChunk {
                request_id,
                response,
            } => self.handle_stream_chunk_response(request_id, response),
            NetworkResponse::HttpStreamComplete {
                request_id,
                response,
            } => self.handle_stream_complete_response(request_id, response),
            NetworkResponse::HttpError { request_id, error } => {
                let in_flight = self.inflight.remove(&request_id)?;
                if !self.agent_run_matches(
                    &in_flight.mount,
                    in_flight.agent_id,
                    in_flight.run_token,
                ) {
                    return None;
                }
                let mount = in_flight.mount.clone();
                self.set_agent_error(
                    &mount,
                    in_flight.agent_id,
                    format!("network error: {}", error.message),
                );
                Some((mount.clone(), self.snapshot(&mount)))
            }
            NetworkResponse::HttpProgress { .. }
            | NetworkResponse::WsOpened { .. }
            | NetworkResponse::WsMessage { .. }
            | NetworkResponse::WsClosed { .. }
            | NetworkResponse::WsError { .. } => None,
        }
    }

    pub fn handle_tool_execution_done(
        &mut self,
        mount: &str,
        agent_id: AiAgentId,
        run_token: u64,
        results: Vec<AiToolExecutionResult>,
    ) -> Option<AiMountState> {
        if !self.agent_run_matches(mount, agent_id, run_token) {
            return None;
        }

        let mut continue_loop = false;
        if let Some(agent) = self
            .mounts
            .get_mut(mount)
            .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
        {
            agent.pending_tool_batch = false;
            for result in &results {
                agent.history.push(ConversationItem::ToolResult {
                    tool_call_id: result.tool_call_id.clone(),
                    content: result.content.clone(),
                });
                agent.messages.push(AiMessage {
                    role: AiMessageRole::ToolResult,
                    text: format_tool_result_message(result),
                });
            }
            agent.updated_at = now_seconds();
            if agent.cancel_requested {
                agent.cancel_requested = false;
                agent.status = "cancelled".to_string();
            } else if agent.tool_rounds >= MAX_TOOL_ROUNDS {
                self.set_agent_error(
                    mount,
                    agent_id,
                    format!("tool loop exceeded {} rounds", MAX_TOOL_ROUNDS),
                );
            } else {
                agent.tool_rounds += 1;
                agent.status = "thinking...".to_string();
                continue_loop = true;
            }
        }
        for result in &results {
            self.process_ai_tool_result_for_task(mount, agent_id, result);
        }
        self.persist_mount_state_best_effort(mount);

        if continue_loop {
            self.start_model_request(mount, agent_id, run_token);
        }

        Some(self.snapshot(mount))
    }

    fn handle_stream_chunk_response(
        &mut self,
        request_id: LiveId,
        response: makepad_network::HttpResponse,
    ) -> Option<(String, AiMountState)> {
        let in_flight = self.inflight.get(&request_id)?;
        if !self.agent_run_matches(&in_flight.mount, in_flight.agent_id, in_flight.run_token) {
            return None;
        }
        let mount = in_flight.mount.clone();
        let agent_id = in_flight.agent_id;
        let body = response.get_string_body().unwrap_or_default();
        if response.status_code >= 400 {
            self.inflight.remove(&request_id);
            self.set_agent_error(
                &mount,
                agent_id,
                format!(
                    "HTTP {}: {}",
                    response.status_code,
                    extract_error_text(&body)
                ),
            );
            return Some((mount.clone(), self.snapshot(&mount)));
        }
        match self.process_stream_data(request_id, &body, false) {
            Ok(stream_update) => {
                if stream_update.done {
                    return self.finish_stream_request(request_id);
                }
                if stream_update.changed {
                    return Some((mount.clone(), self.snapshot(&mount)));
                }
                None
            }
            Err(error) => {
                self.inflight.remove(&request_id);
                self.set_agent_error(&mount, agent_id, error);
                Some((mount.clone(), self.snapshot(&mount)))
            }
        }
    }

    fn handle_stream_complete_response(
        &mut self,
        request_id: LiveId,
        response: makepad_network::HttpResponse,
    ) -> Option<(String, AiMountState)> {
        let in_flight = self.inflight.get(&request_id)?;
        if !self.agent_run_matches(&in_flight.mount, in_flight.agent_id, in_flight.run_token) {
            self.inflight.remove(&request_id);
            return None;
        }
        let mount = in_flight.mount.clone();
        let agent_id = in_flight.agent_id;
        let body = response.get_string_body().unwrap_or_default();
        if response.status_code >= 400 {
            self.inflight.remove(&request_id);
            self.set_agent_error(
                &mount,
                agent_id,
                format!(
                    "HTTP {}: {}",
                    response.status_code,
                    extract_error_text(&body)
                ),
            );
            return Some((mount.clone(), self.snapshot(&mount)));
        }
        if let Err(error) = self.process_stream_data(request_id, &body, true) {
            self.inflight.remove(&request_id);
            self.set_agent_error(&mount, agent_id, error);
            return Some((mount.clone(), self.snapshot(&mount)));
        }
        self.finish_stream_request(request_id)
    }

    fn process_stream_data(
        &mut self,
        request_id: LiveId,
        data: &str,
        flush: bool,
    ) -> Result<StreamUpdate, String> {
        let Some((mount, agent_id, run_token)) = self.inflight.get(&request_id).map(|in_flight| {
            (
                in_flight.mount.clone(),
                in_flight.agent_id,
                in_flight.run_token,
            )
        }) else {
            return Ok(StreamUpdate::default());
        };
        if !self.agent_run_matches(&mount, agent_id, run_token) {
            return Ok(StreamUpdate::default());
        }

        let events = {
            let in_flight = self.inflight.get_mut(&request_id).expect("checked above");
            if !data.is_empty() {
                in_flight
                    .stream
                    .buffer
                    .push_str(&data.replace("\r\n", "\n"));
            }
            drain_sse_events(&mut in_flight.stream.buffer, flush)
        };

        if events.is_empty() {
            return Ok(StreamUpdate::default());
        }

        let mut thinking_delta = String::new();
        let mut assistant_delta = String::new();
        let mut finish_reason = None;
        let mut tool_call_deltas = Vec::new();
        let mut saw_done = false;

        for event in events {
            let Some(json_data) = extract_sse_event_data(&event) else {
                continue;
            };
            if json_data == "[DONE]" {
                saw_done = true;
                continue;
            }
            let chunk = OpenAiStreamChunk::deserialize_json_lenient(&json_data)
                .map_err(|err| format!("invalid AI stream chunk: {:?}", err))?;
            if let Some(error) = chunk.error {
                return Err(error
                    .message
                    .unwrap_or_else(|| "AI backend returned a stream error".to_string()));
            }
            for choice in chunk.choices {
                if let Some(reason) = choice.finish_reason {
                    finish_reason = Some(reason);
                }
                if let Some(delta) = choice.delta {
                    if let Some(reasoning) = first_non_empty_stream_reasoning(&delta) {
                        thinking_delta.push_str(&reasoning);
                    }
                    if let Some(text) = delta.content {
                        assistant_delta.push_str(&text);
                    }
                    if let Some(tool_calls) = delta.tool_calls {
                        tool_call_deltas.extend(tool_calls);
                    }
                }
            }
        }

        {
            let in_flight = self.inflight.get_mut(&request_id).expect("checked above");
            if !thinking_delta.is_empty() {
                in_flight.stream.thinking_text.push_str(&thinking_delta);
            }
            if !assistant_delta.is_empty() {
                in_flight.stream.assistant_text.push_str(&assistant_delta);
            }
            if let Some(reason) = finish_reason {
                in_flight.stream.finish_reason = Some(reason);
            }
            if saw_done {
                in_flight.stream.done_received = true;
            }
            for delta in tool_call_deltas {
                apply_tool_call_delta(&mut in_flight.stream.tool_calls, delta)?;
            }
        }

        let (thinking_text, assistant_text, mut visible) = {
            let in_flight = self.inflight.get(&request_id).expect("checked above");
            (
                truncate_text(&in_flight.stream.thinking_text, MAX_RESULT_CHARS),
                truncate_text(&in_flight.stream.assistant_text, MAX_RESULT_CHARS),
                in_flight.stream.visible,
            )
        };

        if let Some(agent) = self
            .mounts
            .get_mut(&mount)
            .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
        {
            if agent.run_token != run_token {
                return Ok(StreamUpdate::default());
            }
            visible.thinking_message_index = upsert_stream_message(
                &mut agent.messages,
                visible.thinking_message_index,
                AiMessageRole::Thinking,
                &thinking_text,
            );
            visible.assistant_message_index = upsert_stream_message(
                &mut agent.messages,
                visible.assistant_message_index,
                AiMessageRole::Assistant,
                &assistant_text,
            );
            agent.status = if assistant_text.trim().is_empty() {
                "thinking...".to_string()
            } else {
                "responding...".to_string()
            };
            agent.updated_at = now_seconds();
        }

        let done = self
            .inflight
            .get(&request_id)
            .map(|in_flight| in_flight.stream.done_received)
            .unwrap_or(false);
        if let Some(in_flight) = self.inflight.get_mut(&request_id) {
            in_flight.stream.visible = visible;
        }

        Ok(StreamUpdate {
            changed: true,
            done,
        })
    }

    fn finish_stream_request(&mut self, request_id: LiveId) -> Option<(String, AiMountState)> {
        let in_flight = self.inflight.remove(&request_id)?;
        let mount = in_flight.mount.clone();
        let agent_id = in_flight.agent_id;
        let run_token = in_flight.run_token;
        let turn = match finalize_stream_turn(in_flight.stream) {
            Ok(turn) => turn,
            Err(error) => {
                self.set_agent_error(&mount, agent_id, error);
                return Some((mount.clone(), self.snapshot(&mount)));
            }
        };
        self.complete_assistant_turn(&mount, agent_id, run_token, turn.0, Some(turn.1))
            .map(|state| (mount.clone(), state))
    }

    fn complete_assistant_turn(
        &mut self,
        mount: &str,
        agent_id: AiAgentId,
        run_token: u64,
        turn: AssistantTurn,
        visible: Option<StreamVisibleState>,
    ) -> Option<AiMountState> {
        let mut tool_batch: Option<(PathBuf, Vec<ToolCallRecord>)> = None;
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            if let Some(agent) = mount_state.agents.get_mut(&agent_id) {
                if agent.run_token != run_token {
                    return None;
                }
                agent.pending_request_id = None;
                agent.updated_at = now_seconds();

                if let Some(mut visible) = visible {
                    if turn.thinking_text.trim().is_empty() {
                        if let Some(index) = visible.thinking_message_index {
                            if index < agent.messages.len()
                                && matches!(agent.messages[index].role, AiMessageRole::Thinking)
                            {
                                agent.messages.remove(index);
                                if let Some(assistant_index) = visible.assistant_message_index {
                                    if assistant_index > index {
                                        visible.assistant_message_index = Some(assistant_index - 1);
                                    }
                                }
                            }
                        }
                    } else {
                        upsert_stream_message(
                            &mut agent.messages,
                            visible.thinking_message_index,
                            AiMessageRole::Thinking,
                            &truncate_text(&turn.thinking_text, MAX_RESULT_CHARS),
                        );
                    }
                    upsert_stream_message(
                        &mut agent.messages,
                        visible.assistant_message_index,
                        AiMessageRole::Assistant,
                        &truncate_text(&turn.text, MAX_RESULT_CHARS),
                    );
                } else {
                    if !turn.thinking_text.trim().is_empty() {
                        agent.messages.push(AiMessage {
                            role: AiMessageRole::Thinking,
                            text: truncate_text(turn.thinking_text.trim(), MAX_RESULT_CHARS),
                        });
                    }
                    if !turn.text.trim().is_empty() {
                        agent.messages.push(AiMessage {
                            role: AiMessageRole::Assistant,
                            text: turn.text.clone(),
                        });
                    }
                }

                agent.history.push(ConversationItem::Assistant {
                    text: turn.text.clone(),
                    tool_calls: turn.tool_calls.clone(),
                });

                if turn.tool_calls.is_empty() {
                    if turn.text.trim().is_empty() && turn.thinking_text.trim().is_empty() {
                        self.set_agent_error(
                            mount,
                            agent_id,
                            "AI backend returned an empty assistant response".to_string(),
                        );
                    } else {
                        agent.status = "ready".to_string();
                    }
                } else {
                    for tool_call in &turn.tool_calls {
                        agent.messages.push(AiMessage {
                            role: AiMessageRole::ToolCall,
                            text: format_tool_call_message(tool_call),
                        });
                    }
                    agent.pending_tool_batch = true;
                    agent.status = if turn.tool_calls.len() == 1 {
                        format!("running {}...", turn.tool_calls[0].name)
                    } else {
                        format!("running {} tool calls...", turn.tool_calls.len())
                    };
                    tool_batch = Some((
                        PathBuf::from(mount_state.root_path.clone()),
                        turn.tool_calls,
                    ));
                }
            }
        }
        if let Some((root_path, tool_calls)) = tool_batch {
            self.spawn_tool_execution(
                mount.to_string(),
                agent_id,
                run_token,
                root_path,
                tool_calls,
            );
        }
        self.persist_mount_state_best_effort(mount);
        Some(self.snapshot(mount))
    }

    fn detect_backends() -> Vec<AiBackendConfig> {
        let mut backends = Vec::new();
        let local_url = std::env::var("MAKEPAD_STUDIO_AI_BASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("MAKEPAD_AI_MANAGER_BASE_URL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| DEFAULT_LOCAL_BASE_URL.to_string());
        let local_model = std::env::var("MAKEPAD_STUDIO_AI_LOCAL_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("MAKEPAD_AI_MANAGER_MODEL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| DEFAULT_LOCAL_MODEL.to_string());
        backends.push(AiBackendConfig {
            id: LOCAL_BACKEND_ID.to_string(),
            label: "OpenAI Compatible".to_string(),
            detail: if local_model.is_empty() {
                local_url.clone()
            } else {
                format!("{}  {}", local_model, local_url)
            },
            url: local_url,
            model: local_model,
            api_key: None,
            disable_thinking_via_chat_template: false,
        });

        if let Some(api_key) = read_secret_or_env("OPENAI_API_KEY") {
            let model = std::env::var("MAKEPAD_STUDIO_AI_MODEL")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
            backends.push(AiBackendConfig {
                id: CLOUD_BACKEND_ID.to_string(),
                label: "OpenAI".to_string(),
                detail: model.clone(),
                url: "https://api.openai.com/v1/chat/completions".to_string(),
                model,
                api_key: Some(api_key),
                disable_thinking_via_chat_template: false,
            });
        }

        backends
    }

    fn default_backend_id(&self) -> String {
        self.backends
            .iter()
            .find(|backend| backend.id == LOCAL_BACKEND_ID)
            .or_else(|| self.backends.first())
            .map(|backend| backend.id.clone())
            .unwrap_or_else(|| LOCAL_BACKEND_ID.to_string())
    }

    fn ensure_mount_entry(&mut self, mount: &str) {
        if !self.mounts.contains_key(mount) {
            let default_backend_id = self.default_backend_id();
            self.mounts.insert(
                mount.to_string(),
                MountAgents {
                    root_path: String::new(),
                    active_backend_id: default_backend_id,
                    active_agent_id: None,
                    next_chat_ordinal: 1,
                    next_task_id: 1,
                    loaded_from_disk: false,
                    order: Vec::new(),
                    agents: HashMap::new(),
                    tasks: Vec::new(),
                    queued_followups: VecDeque::new(),
                    terminal_snapshots: HashMap::new(),
                },
            );
        }
        self.ensure_default_agent(mount);
    }

    fn ensure_default_agent(&mut self, mount: &str) {
        let needs_default = self
            .mounts
            .get(mount)
            .map(|state| state.order.is_empty())
            .unwrap_or(true);
        if !needs_default {
            return;
        }
        let agent_id = self.alloc_agent_id();
        let default_backend_id = self.default_backend_id();
        let mount_state = self
            .mounts
            .entry(mount.to_string())
            .or_insert_with(|| MountAgents {
                root_path: String::new(),
                active_backend_id: default_backend_id,
                active_agent_id: None,
                next_chat_ordinal: 1,
                next_task_id: 1,
                loaded_from_disk: false,
                order: Vec::new(),
                agents: HashMap::new(),
                tasks: Vec::new(),
                queued_followups: VecDeque::new(),
                terminal_snapshots: HashMap::new(),
            });
        let title = format!("Chat {}", mount_state.next_chat_ordinal);
        mount_state.next_chat_ordinal += 1;
        mount_state.active_agent_id = Some(agent_id);
        mount_state.order.push(agent_id);
        mount_state.agents.insert(
            agent_id,
            RunningAgent {
                title,
                backend_id: mount_state.active_backend_id.clone(),
                status: "ready".to_string(),
                pending_request_id: None,
                pending_tool_batch: false,
                cancel_requested: false,
                run_token: 0,
                tool_rounds: 0,
                messages: Vec::new(),
                history: Vec::new(),
                updated_at: now_seconds(),
            },
        );
        self.persist_mount_state_best_effort(mount);
    }

    fn alloc_agent_id(&mut self) -> AiAgentId {
        let agent_id = AiAgentId(self.next_agent_id.max(1));
        self.next_agent_id = self.next_agent_id.wrapping_add(1);
        if self.next_agent_id == 0 {
            self.next_agent_id = 1;
        }
        agent_id
    }

    fn alloc_run_token(&mut self) -> u64 {
        let run_token = self.next_run_token.max(1);
        self.next_run_token = self.next_run_token.wrapping_add(1);
        if self.next_run_token == 0 {
            self.next_run_token = 1;
        }
        run_token
    }

    fn agent_run_matches(&self, mount: &str, agent_id: AiAgentId, run_token: u64) -> bool {
        self.mounts
            .get(mount)
            .and_then(|mount_state| mount_state.agents.get(&agent_id))
            .map(|agent| agent.run_token == run_token)
            .unwrap_or(false)
    }

    fn start_model_request(&mut self, mount: &str, agent_id: AiAgentId, run_token: u64) {
        let Some((backend, root_path, history)) = self.mounts.get(mount).and_then(|mount_state| {
            let agent = mount_state.agents.get(&agent_id)?;
            Some((
                self.backend_by_id(&agent.backend_id)?.clone(),
                mount_state.root_path.clone(),
                agent.history.clone(),
            ))
        }) else {
            self.set_agent_error(mount, agent_id, "backend not available".to_string());
            return;
        };

        let request_id = LiveId::unique();
        let body = build_request_body(&backend, mount, &root_path, &history);

        let mut request = HttpRequest::new(backend.url.clone(), HttpMethod::POST);
        request.set_is_streaming();
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_header("Accept".to_string(), "text/event-stream".to_string());
        if let Some(api_key) = &backend.api_key {
            request.set_header("Authorization".to_string(), format!("Bearer {}", api_key));
        }
        request.set_string_body(body);

        match self.runtime.http_start(request_id, request) {
            Ok(()) => {
                self.inflight.insert(
                    request_id,
                    InFlightRequest {
                        mount: mount.to_string(),
                        agent_id,
                        run_token,
                        stream: StreamingTurnState::default(),
                    },
                );
                if let Some(agent) = self
                    .mounts
                    .get_mut(mount)
                    .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
                {
                    if agent.run_token == run_token {
                        let thinking_message_index =
                            agent.messages.len().checked_sub(1).filter(|&index| {
                                agent.messages.get(index).is_some_and(|message| {
                                    matches!(message.role, AiMessageRole::Thinking)
                                        && message.text.is_empty()
                                })
                            });
                        agent.pending_request_id = Some(request_id);
                        agent.pending_tool_batch = false;
                        agent.updated_at = now_seconds();
                        if let Some(in_flight) = self.inflight.get_mut(&request_id) {
                            in_flight.stream.visible.thinking_message_index =
                                thinking_message_index;
                        }
                    }
                }
            }
            Err(err) => {
                self.set_agent_error(mount, agent_id, format!("request failed: {:?}", err));
            }
        }
    }

    fn spawn_tool_execution(
        &self,
        mount: String,
        agent_id: AiAgentId,
        run_token: u64,
        root_path: PathBuf,
        tool_calls: Vec<ToolCallRecord>,
    ) {
        let event_tx = self.event_tx.clone();
        let tool_mount = mount.clone();
        thread::spawn(move || {
            let mut results = Vec::new();
            for tool_call in tool_calls {
                results.push(execute_tool_call(
                    &root_path,
                    &tool_mount,
                    &event_tx,
                    &tool_call,
                ));
            }
            let _ = event_tx.send(HubEvent::AiToolExecutionDone {
                mount,
                agent_id,
                run_token,
                results,
            });
        });
    }

    fn note_ai_prompt_task(&mut self, mount: &str, agent_id: AiAgentId, prompt: &str) {
        if prompt.starts_with(AI_TASK_EVENT_PREFIX) || !should_track_ai_terminal_task(prompt) {
            return;
        }
        let Some(mount_state) = self.mounts.get_mut(mount) else {
            return;
        };
        let task_id = mount_state.next_task_id.max(1);
        mount_state.next_task_id = task_id.saturating_add(1);
        mount_state.tasks.push(AiTrackedTask {
            id: task_id,
            agent_id,
            goal: prompt.trim().to_string(),
            terminal_path: None,
            expected_paths: extract_expected_paths_from_prompt(prompt),
            touched_paths: Vec::new(),
            status: "waiting-terminal".to_string(),
            last_terminal_mode: "waiting-terminal".to_string(),
            last_terminal_summary: "Waiting for the AI to hand work to a terminal".to_string(),
            last_terminal_excerpt: String::new(),
            last_codex_status: None,
        });
    }

    fn process_ai_tool_result_for_task(
        &mut self,
        mount: &str,
        agent_id: AiAgentId,
        result: &AiToolExecutionResult,
    ) -> bool {
        if !is_terminal_tool_name(&result.tool_name) {
            return false;
        }
        let Some(path) = parse_json_string_field(&result.content, "path") else {
            return false;
        };
        self.bind_waiting_ai_task_to_terminal(mount, agent_id, &path)
    }

    fn bind_waiting_ai_task_to_terminal(
        &mut self,
        mount: &str,
        agent_id: AiAgentId,
        path: &str,
    ) -> bool {
        let Some(mount_state) = self.mounts.get_mut(mount) else {
            return false;
        };
        let Some(task) = mount_state
            .tasks
            .iter_mut()
            .find(|task| task.agent_id == agent_id && task.terminal_path.is_none())
        else {
            return false;
        };
        let snapshot = mount_state
            .terminal_snapshots
            .get(path)
            .cloned()
            .unwrap_or_else(|| AiTerminalSnapshot {
                path: path.to_string(),
                mode: "starting",
                summary: format!("Tracking {}", terminal_display_name(path)),
                visible_text: String::new(),
                is_codex: false,
                codex_status: None,
            });
        task.terminal_path = Some(path.to_string());
        task.status = "watching".to_string();
        task.last_terminal_mode = snapshot.mode.to_string();
        task.last_terminal_summary = snapshot.summary;
        task.last_terminal_excerpt = Self::truncate_terminal_excerpt(
            &snapshot.visible_text,
            AI_TERMINAL_EXCERPT_MAX_CHARS,
            AI_TERMINAL_EXCERPT_MAX_LINES,
        );
        task.last_codex_status = snapshot.codex_status;
        true
    }

    fn queue_ai_task_followup(
        &mut self,
        mount: &str,
        task_id: u64,
        signature: String,
        reason: &str,
    ) {
        let Some((agent_id, text)) = self.ai_task_event_prompt(mount, task_id, reason) else {
            return;
        };
        let Some(mount_state) = self.mounts.get_mut(mount) else {
            return;
        };
        if mount_state
            .queued_followups
            .iter()
            .any(|entry| entry.task_id == task_id && entry.signature == signature)
        {
            return;
        }
        mount_state.queued_followups.push_back(AiQueuedFollowup {
            agent_id,
            task_id,
            signature,
            text,
        });
    }

    fn ai_task_event_prompt(
        &self,
        mount: &str,
        task_id: u64,
        reason: &str,
    ) -> Option<(AiAgentId, String)> {
        let task = self
            .mounts
            .get(mount)?
            .tasks
            .iter()
            .find(|task| task.id == task_id)?;
        let mut prompt = String::new();
        prompt.push_str(AI_TASK_EVENT_PREFIX);
        prompt.push(' ');
        prompt.push_str(&format!("task {} update\n", task.id));
        prompt.push_str(&format!("Reason: {}\n", reason));
        prompt.push_str(&format!("Goal: {}\n", task.goal));
        prompt.push_str(&format!("Task state: {}\n", task.status));
        if let Some(path) = &task.terminal_path {
            prompt.push_str(&format!("Terminal path: {}\n", path));
        }
        prompt.push_str(&format!("Terminal mode: {}\n", task.last_terminal_mode));
        if let Some(codex_status) = &task.last_codex_status {
            prompt.push_str(&format!("Codex status: {}\n", codex_status));
        }
        if !task.last_terminal_summary.is_empty() {
            prompt.push_str(&format!("Summary: {}\n", task.last_terminal_summary));
        }
        if !task.expected_paths.is_empty() {
            prompt.push_str(&format!(
                "Expected paths: {}\n",
                task.expected_paths.join(", ")
            ));
        }
        if !task.touched_paths.is_empty() {
            prompt.push_str(&format!(
                "Touched paths: {}\n",
                task.touched_paths.join(", ")
            ));
        }
        if !task.last_terminal_excerpt.is_empty() {
            prompt.push_str("\nLatest output excerpt:\n```text\n");
            prompt.push_str(&task.last_terminal_excerpt);
            prompt.push_str("\n```\n");
        }
        prompt.push_str(
            "\nContinue supervising this delegated terminal task. If it is finished, tell the user briefly. If more work is needed, use terminal tools instead of guessing.",
        );
        Some((task.agent_id, prompt))
    }

    fn dispatch_next_ai_manager_followup(&mut self, mount: &str) -> bool {
        let Some((queue_index, queued)) = self.mounts.get(mount).and_then(|mount_state| {
            mount_state
                .queued_followups
                .iter()
                .enumerate()
                .find(|(_, entry)| {
                    mount_state
                        .agents
                        .get(&entry.agent_id)
                        .map(|agent| !agent.is_pending())
                        .unwrap_or(false)
                })
                .map(|(index, entry)| (index, entry.clone()))
        }) else {
            return false;
        };
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            let _ = mount_state.queued_followups.remove(queue_index);
        }
        self.send_prompt(mount, queued.agent_id, &queued.text);
        true
    }

    fn ai_live_markdown(&self, mount_state: &MountAgents) -> String {
        let mut markdown = String::new();
        if mount_state.tasks.is_empty() {
            markdown.push_str("**Tasks**\n\n_No delegated terminal tasks yet._");
        } else {
            markdown.push_str("**Tasks**\n\n");
            for task in &mount_state.tasks {
                markdown.push_str(&format!(
                    "- `T{}` [{}] {}\n",
                    task.id,
                    task.status,
                    truncate_inline(&task.goal, 96)
                ));
                if let Some(path) = &task.terminal_path {
                    markdown.push_str(&format!(
                        "  `{}` [{}]\n",
                        path,
                        truncate_inline(&task.last_terminal_summary, 96)
                    ));
                } else {
                    markdown.push_str("  waiting for terminal assignment\n");
                }
                if !task.touched_paths.is_empty() {
                    markdown.push_str(&format!("  files: {}\n", task.touched_paths.join(", ")));
                } else if !task.expected_paths.is_empty() {
                    markdown.push_str(&format!(
                        "  expecting: {}\n",
                        task.expected_paths.join(", ")
                    ));
                }
            }
        }

        markdown.push_str("\n\n**Terminals**\n\n");
        if mount_state.terminal_snapshots.is_empty() {
            markdown.push_str("_No terminal activity yet._");
        } else {
            let mut terminals = mount_state
                .terminal_snapshots
                .values()
                .collect::<Vec<&AiTerminalSnapshot>>();
            terminals.sort_by(|left, right| left.path.cmp(&right.path));
            for terminal in terminals {
                markdown.push_str(&format!(
                    "- `{}` [{}{}]\n",
                    terminal.path,
                    terminal.mode,
                    if terminal.is_codex { " / codex" } else { "" }
                ));
                if let Some(codex_status) = &terminal.codex_status {
                    markdown.push_str(&format!("  {}\n", truncate_inline(codex_status, 96)));
                }
                markdown.push_str(&format!("  {}\n", truncate_inline(&terminal.summary, 96)));
            }
        }
        markdown
    }

    pub(crate) fn terminal_mode_and_summary(
        title: &str,
        visible_text: &str,
    ) -> (&'static str, bool, String, Option<String>) {
        let lines: Vec<String> = visible_text.lines().map(|line| line.to_string()).collect();
        let lowered = format!("{}\n{}", title, visible_text).to_lowercase();
        let is_codex = lowered.contains("codex")
            || lowered.contains("apply_patch")
            || lowered.contains("exec_command")
            || lowered.contains("functions.exec_command");
        let codex_status = if is_codex {
            Self::detect_codex_status_line(&lines)
        } else {
            None
        };
        let needs_attention = lowered.contains("permission denied")
            || lowered.contains("sandbox")
            || lowered.contains("panic")
            || lowered.contains("error:")
            || lowered.contains("failed")
            || lowered.contains("blocked")
            || lowered.contains("approve")
            || lowered.contains("how would you like to proceed");
        let awaiting_input = lowered.contains("waiting for user")
            || lowered.contains("request user input")
            || lowered.contains("press enter")
            || lowered.contains("press return")
            || lowered.contains("continue?")
            || lowered.contains("type 'continue'")
            || lowered.contains("type \"continue\"");
        let working = lowered.contains("apply_patch")
            || lowered.contains("exec_command")
            || lowered.contains("searching")
            || lowered.contains("reading")
            || lowered.contains("building")
            || lowered.contains("testing")
            || lowered.contains("running")
            || lowered.contains("patching")
            || codex_status.is_some();
        let codex_prompt_visible = is_codex
            && lines
                .iter()
                .rev()
                .take(6)
                .any(|line| Self::is_codex_prompt_line(line));

        let mode = if needs_attention {
            "needs-attention"
        } else if awaiting_input {
            "awaiting-input"
        } else if is_codex && codex_prompt_visible && codex_status.is_none() {
            "done"
        } else if visible_text.trim().is_empty() {
            "starting"
        } else if working {
            "working"
        } else {
            "idle"
        };

        (
            mode,
            is_codex,
            Self::terminal_summary_line(&lines, is_codex, codex_status.as_deref()),
            codex_status,
        )
    }

    fn is_codex_prompt_line(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.starts_with('\u{203a}')
            || trimmed.starts_with('>')
            || trimmed.contains("Enter a prompt...")
    }

    fn detect_codex_status_line(lines: &[String]) -> Option<String> {
        lines.iter().rev().take(8).find_map(|line| {
            let trimmed = line.trim();
            if trimmed.contains("Working (") && trimmed.contains("esc to interrupt") {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
    }

    fn terminal_summary_line(
        lines: &[String],
        is_codex: bool,
        codex_status: Option<&str>,
    ) -> String {
        lines
            .iter()
            .rev()
            .map(|line| line.trim())
            .find(|line| {
                !line.is_empty()
                    && Some(*line) != codex_status
                    && !(is_codex
                        && (Self::is_codex_prompt_line(line)
                            || line.contains("esc to interrupt")
                            || line.contains("100% left")
                            || line.contains("left \u{00b7}")))
            })
            .map(|line| truncate_inline(line, 140))
            .unwrap_or_else(|| "No visible output yet".to_string())
    }

    fn truncate_terminal_excerpt(text: &str, max_chars: usize, max_lines: usize) -> String {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .collect();
        if lines.is_empty() {
            return String::new();
        }
        let start = lines.len().saturating_sub(max_lines);
        let excerpt = lines[start..].join("\n");
        if excerpt.chars().count() <= max_chars {
            return excerpt;
        }
        let tail: String = excerpt
            .chars()
            .rev()
            .take(max_chars.saturating_sub(3))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("...{}", tail)
    }

    fn snapshot(&self, mount: &str) -> AiMountState {
        let Some(mount_state) = self.mounts.get(mount) else {
            return AiMountState::default();
        };
        let backends = self
            .backends
            .iter()
            .map(|backend| AiBackendInfo {
                id: backend.id.clone(),
                label: backend.label.clone(),
                detail: backend.detail.clone(),
                configured: true,
            })
            .collect::<Vec<_>>();
        let agents = mount_state
            .order
            .iter()
            .filter_map(|agent_id| {
                let agent = mount_state.agents.get(agent_id)?;
                Some(AiAgentSummary {
                    agent_id: *agent_id,
                    title: agent.title.clone(),
                    backend_id: agent.backend_id.clone(),
                    status: agent.status.clone(),
                    pending: agent.is_pending(),
                    updated_at: agent.updated_at,
                    message_count: agent.messages.len(),
                })
            })
            .collect::<Vec<_>>();
        let active_agent = mount_state.active_agent_id.and_then(|agent_id| {
            let agent = mount_state.agents.get(&agent_id)?;
            Some(AiAgentState {
                agent_id,
                title: agent.title.clone(),
                backend_id: agent.backend_id.clone(),
                status: agent.status.clone(),
                pending: agent.is_pending(),
                messages: agent.messages.clone(),
            })
        });
        AiMountState {
            backends,
            active_backend_id: Some(mount_state.active_backend_id.clone()),
            active_agent_id: mount_state.active_agent_id,
            agents,
            active_agent,
            live_markdown: self.ai_live_markdown(mount_state),
        }
    }

    fn load_mount_from_disk(&mut self, mount: &str) {
        let Some((root_path, fallback_backend_id)) = self.mounts.get(mount).map(|mount_state| {
            (
                mount_state.root_path.clone(),
                mount_state.active_backend_id.clone(),
            )
        }) else {
            return;
        };
        if root_path.is_empty() {
            return;
        }

        let Ok(entries) = fs::read_dir(ai_chats_dir(Path::new(&root_path))) else {
            return;
        };

        let mut chats = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(chat) = PersistedAiChat::deserialize_json(&contents) else {
                continue;
            };
            chats.push(chat);
        }

        if chats.is_empty() {
            return;
        }

        chats.sort_by_key(|chat| chat.agent_id.0);
        let mut order = Vec::with_capacity(chats.len());
        let mut agents = HashMap::with_capacity(chats.len());
        let mut next_chat_ordinal = 1u64;
        let mut active_agent_id = None;
        let mut newest_key = None::<(u64, u64)>;

        for chat in chats {
            self.next_agent_id = self.next_agent_id.max(chat.agent_id.0.saturating_add(1));
            next_chat_ordinal =
                next_chat_ordinal.max(chat_title_ordinal(&chat.title).saturating_add(1));

            let agent_id = chat.agent_id;
            let pending = chat.pending;
            let mut messages = chat.messages;
            if pending {
                while messages.last().is_some_and(|message| {
                    matches!(message.role, AiMessageRole::Thinking) && message.text.is_empty()
                }) {
                    messages.pop();
                }
            }
            let backend_id = if self.backend_by_id(&chat.backend_id).is_some() {
                chat.backend_id
            } else {
                fallback_backend_id.clone()
            };
            let status = if pending {
                "ready".to_string()
            } else if chat.status.trim().is_empty() {
                "ready".to_string()
            } else {
                chat.status
            };

            let updated_micros = (chat.updated_at.max(0.0) * 1_000_000.0) as u64;
            let key = (updated_micros, agent_id.0);
            if chat.active.unwrap_or(false) {
                active_agent_id = Some(agent_id);
            } else if newest_key.map(|existing| key >= existing).unwrap_or(true) {
                newest_key = Some(key);
                if active_agent_id.is_none() {
                    active_agent_id = Some(agent_id);
                }
            }

            order.push(agent_id);
            agents.insert(
                agent_id,
                RunningAgent {
                    title: chat.title,
                    backend_id,
                    status,
                    pending_request_id: None,
                    pending_tool_batch: false,
                    cancel_requested: false,
                    run_token: 0,
                    tool_rounds: 0,
                    messages,
                    history: chat.history,
                    updated_at: chat.updated_at,
                },
            );
        }

        if let Some(mount_state) = self.mounts.get_mut(mount) {
            mount_state.order = order;
            mount_state.agents = agents;
            mount_state.active_agent_id =
                active_agent_id.or_else(|| mount_state.order.last().copied());
            mount_state.next_chat_ordinal = next_chat_ordinal.max(1);
        }
    }

    fn persist_mount_state_best_effort(&self, mount: &str) {
        self.suppress_chat_persist_fs_events(mount);
        if let Err(err) = self.persist_mount_state(mount) {
            eprintln!("makepad-studio-hub: failed to persist AI chats for mount {mount}: {err}");
        }
    }

    fn suppress_chat_persist_fs_events(&self, mount: &str) {
        let _ = self.event_tx.send(HubEvent::SuppressMountRootFsEvents {
            mount: mount.to_string(),
            duration: AI_CHAT_PERSIST_FS_SUPPRESS,
        });
    }

    fn persist_mount_state(&self, mount: &str) -> Result<(), String> {
        let Some(mount_state) = self.mounts.get(mount) else {
            return Ok(());
        };
        if mount_state.root_path.is_empty() {
            return Ok(());
        }

        let dir = ai_chats_dir(Path::new(&mount_state.root_path));
        fs::create_dir_all(&dir)
            .map_err(|err| format!("failed to create {}: {}", dir.display(), err))?;

        for agent_id in &mount_state.order {
            let Some(agent) = mount_state.agents.get(agent_id) else {
                continue;
            };
            let persisted = PersistedAiChat {
                version: 1,
                agent_id: *agent_id,
                title: agent.title.clone(),
                backend_id: agent.backend_id.clone(),
                active: Some(mount_state.active_agent_id == Some(*agent_id)),
                status: agent.status.clone(),
                pending: agent.is_pending(),
                updated_at: agent.updated_at,
                messages: agent.messages.clone(),
                history: agent.history.clone(),
            };
            let path = ai_chat_file_path(Path::new(&mount_state.root_path), *agent_id);
            fs::write(&path, persisted.serialize_json())
                .map_err(|err| format!("failed to write {}: {}", path.display(), err))?;
        }

        Ok(())
    }

    fn remove_agent_file_best_effort(&self, mount: &str, agent_id: AiAgentId) {
        let Some(root_path) = self
            .mounts
            .get(mount)
            .map(|mount_state| mount_state.root_path.clone())
        else {
            return;
        };
        if root_path.is_empty() {
            return;
        }
        self.suppress_chat_persist_fs_events(mount);
        let path = ai_chat_file_path(Path::new(&root_path), agent_id);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!(
                    "makepad-studio-hub: failed to remove AI chat {}: {}",
                    path.display(),
                    err
                );
            }
        }
    }

    fn set_agent_error(&mut self, mount: &str, agent_id: AiAgentId, error: String) {
        if let Some(agent) = self
            .mounts
            .get_mut(mount)
            .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
        {
            if let Some(request_id) = agent.pending_request_id.take() {
                self.inflight.remove(&request_id);
            }
            agent.pending_tool_batch = false;
            agent.cancel_requested = false;
            agent.status = error.clone();
            agent.updated_at = now_seconds();
            agent.messages.push(AiMessage {
                role: AiMessageRole::Error,
                text: error,
            });
        }
        self.persist_mount_state_best_effort(mount);
    }

    fn backend_by_id(&self, backend_id: &str) -> Option<&AiBackendConfig> {
        self.backends
            .iter()
            .find(|backend| backend.id == backend_id)
    }
}

impl RunningAgent {
    fn is_pending(&self) -> bool {
        self.pending_request_id.is_some() || self.pending_tool_batch
    }
}

fn forward_runtime_events(
    runtime: Arc<NetworkRuntime>,
    event_tx: Sender<HubEvent>,
    idle_timeout: Duration,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        let Some(response) = runtime.recv_timeout(idle_timeout) else {
            continue;
        };
        if event_tx
            .send(HubEvent::AiHttpResponse { response })
            .is_err()
        {
            break;
        }
    }
}

fn build_request_body(
    backend: &AiBackendConfig,
    mount: &str,
    root_path: &str,
    history: &[ConversationItem],
) -> String {
    let system_prompt = render_system_prompt(mount, root_path);
    let mut out = String::new();
    out.push('{');
    let mut needs_comma = false;

    if !backend.model.trim().is_empty() {
        out.push_str("\"model\":");
        out.push_str(&json_string(&backend.model));
        needs_comma = true;
    }

    if needs_comma {
        out.push(',');
    }
    out.push_str("\"messages\":[");
    let mut first_message = true;

    append_plain_message(&mut out, &mut first_message, "system", &system_prompt);

    for item in history {
        match item {
            ConversationItem::User { text } => {
                append_plain_message(&mut out, &mut first_message, "user", text);
            }
            ConversationItem::Assistant { text, tool_calls } => {
                if tool_calls.is_empty() {
                    append_plain_message(&mut out, &mut first_message, "assistant", text);
                } else {
                    if !first_message {
                        out.push(',');
                    }
                    first_message = false;
                    out.push('{');
                    out.push_str("\"role\":\"assistant\",\"content\":");
                    out.push_str(&json_string(text));
                    out.push_str(",\"tool_calls\":[");
                    for (index, tool_call) in tool_calls.iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        out.push('{');
                        out.push_str("\"id\":");
                        out.push_str(&json_string(&tool_call.id));
                        out.push_str(",\"type\":\"function\",\"function\":{");
                        out.push_str("\"name\":");
                        out.push_str(&json_string(&tool_call.name));
                        out.push_str(",\"arguments\":");
                        out.push_str(&json_string(&tool_call.arguments_json));
                        out.push_str("}}");
                    }
                    out.push_str("]}");
                }
            }
            ConversationItem::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                if !first_message {
                    out.push(',');
                }
                first_message = false;
                out.push('{');
                out.push_str("\"role\":\"tool\",\"content\":");
                out.push_str(&json_string(content));
                out.push_str(",\"tool_call_id\":");
                out.push_str(&json_string(tool_call_id));
                out.push('}');
            }
        }
    }

    out.push_str("],\"tools\":[");
    append_tool_definitions(&mut out);
    out.push_str("],\"tool_choice\":\"auto\",\"max_tokens\":");
    out.push_str(&DEFAULT_MAX_TOKENS.to_string());
    out.push_str(",\"stream\":true");
    if backend.disable_thinking_via_chat_template {
        out.push_str(",\"chat_template_kwargs\":{\"enable_thinking\":false}");
    }
    out.push('}');
    out
}

fn render_system_prompt(mount: &str, root_path: &str) -> String {
    SYSTEM_PROMPT_TEMPLATE
        .replace("{{mount}}", mount)
        .replace("{{root_path}}", root_path)
        .trim()
        .to_string()
}

fn append_plain_message(out: &mut String, first_message: &mut bool, role: &str, content: &str) {
    if !*first_message {
        out.push(',');
    }
    *first_message = false;
    out.push('{');
    out.push_str("\"role\":");
    out.push_str(&json_string(role));
    out.push_str(",\"content\":");
    out.push_str(&json_string(content));
    out.push('}');
}

fn append_tool_definitions(out: &mut String) {
    let mut first = true;
    append_tool_definition(
        out,
        &mut first,
        "read_file",
        "Read a UTF-8 text file from the workspace. Use this before editing.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to the file"},"offset":{"type":"integer","description":"Starting line number (1-indexed)"},"limit":{"type":"integer","description":"Maximum number of lines to read"}},"required":["path"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "list_files",
        "List files and directories in the workspace.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Optional workspace-relative directory"},"limit":{"type":"integer","description":"Maximum number of entries to return"}}}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "search_text",
        "Search text in workspace files and return matching path and line snippets.",
        r#"{"type":"object","properties":{"pattern":{"type":"string","description":"Text to search for"},"path":{"type":"string","description":"Optional workspace-relative directory"},"limit":{"type":"integer","description":"Maximum number of matches to return"}},"required":["pattern"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "write_file",
        "Write a UTF-8 text file in the workspace, creating parent directories if needed.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to write"},"content":{"type":"string","description":"Full file contents"}},"required":["path","content"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "replace_in_file",
        "Replace text in an existing UTF-8 file.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to edit"},"old_text":{"type":"string","description":"Existing text to replace"},"new_text":{"type":"string","description":"Replacement text"},"replace_all":{"type":"boolean","description":"Replace all matches instead of the first one"}},"required":["path","old_text","new_text"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "open_editor",
        "Open a UTF-8 text file in a Studio code editor tab for this workspace, optionally jumping to a line and column.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to open in Studio"},"line":{"type":"integer","description":"Optional 1-indexed line to focus after opening"},"column":{"type":"integer","description":"Optional 1-indexed column to focus after opening"}},"required":["path"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "observe_filesystem",
        "Return recent filesystem changes observed by the Studio hub watcher for this workspace. Use this after other agents edit files.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Optional workspace-relative path prefix to filter changes"},"limit":{"type":"integer","description":"Maximum number of recent changes to return"},"since_secs":{"type":"integer","description":"Only include changes observed within this many seconds"}}}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "open_terminal",
        "Open a Studio terminal for this workspace and optionally run an initial command such as codex.",
        r#"{"type":"object","properties":{"name":{"type":"string","description":"Optional terminal tab name stem"},"command":{"type":"string","description":"Optional command to send after the terminal opens"},"cols":{"type":"integer","description":"Optional terminal column count"},"rows":{"type":"integer","description":"Optional terminal row count"}}}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "list_terminals",
        "List currently open Studio terminals for this workspace. Use the returned path value with other terminal tools.",
        r#"{"type":"object","properties":{}}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "read_terminal",
        "Read visible text and state from an open Studio terminal.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Exact terminal path returned by open_terminal or list_terminals"},"rows":{"type":"integer","description":"Optional number of visible rows to include"},"top_row":{"type":"integer","description":"Optional absolute top row to read; omit to read from the bottom"}},"required":["path"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "send_terminal_text",
        "Send text to an open Studio terminal, optionally submitting it with Enter. Use submit=true when the text should run immediately, especially for codex prompts.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Exact terminal path returned by open_terminal or list_terminals"},"text":{"type":"string","description":"Text to send to the terminal"},"submit":{"type":"boolean","description":"When true, press Enter after the text. Use this for commands and codex prompts that should execute immediately"},"bracketed_paste":{"type":"boolean","description":"Override bracketed paste wrapping for multiline text"}},"required":["path","text"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "send_terminal_key",
        "Send a keypress to an open Studio terminal. Use this for Enter, Ctrl+C, arrows, Tab, Escape, or function keys.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Exact terminal path returned by open_terminal or list_terminals"},"key":{"type":"string","description":"Key name such as enter, tab, up, f5, or a single printable character. Modifier prefixes like ctrl+c are also accepted"},"shift":{"type":"boolean","description":"Optional Shift modifier"},"control":{"type":"boolean","description":"Optional Control modifier"},"alt":{"type":"boolean","description":"Optional Alt modifier"}},"required":["path","key"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "bash",
        "Run a shell command inside the workspace root. Prefer quick inspection and verification commands.",
        r#"{"type":"object","properties":{"command":{"type":"string","description":"Shell command to execute"},"timeout_secs":{"type":"integer","description":"Optional timeout in seconds"}},"required":["command"]}"#,
    );
}

fn append_tool_definition(
    out: &mut String,
    first: &mut bool,
    name: &str,
    description: &str,
    parameters_json: &str,
) {
    if !*first {
        out.push(',');
    }
    *first = false;
    out.push_str("{\"type\":\"function\",\"function\":{");
    out.push_str("\"name\":");
    out.push_str(&json_string(name));
    out.push_str(",\"description\":");
    out.push_str(&json_string(description));
    out.push_str(",\"parameters\":");
    out.push_str(parameters_json);
    out.push_str("}}");
}

fn extract_assistant_turn(body: &str) -> Result<AssistantTurn, String> {
    let response = OpenAiResponse::deserialize_json_lenient(body)
        .map_err(|err| format!("invalid AI response: {:?}", err))?;
    if let Some(error) = response.error {
        return Err(error
            .message
            .unwrap_or_else(|| "AI backend returned an error".to_string()));
    }
    let Some(choice) = response.choices.into_iter().next() else {
        return Err("AI backend returned no choices".to_string());
    };
    let thinking_text = first_non_empty_reasoning(&choice.message).unwrap_or_default();
    let text = choice.message.content.unwrap_or_default();
    let tool_calls = choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tool_call| {
            if let Some(kind) = &tool_call.kind {
                if kind != "function" {
                    return Err(format!("unsupported tool call type '{}'", kind));
                }
            }
            Ok(ToolCallRecord {
                id: tool_call.id,
                name: tool_call.function.name,
                arguments_json: tool_call.function.arguments,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(AssistantTurn {
        text,
        thinking_text,
        tool_calls,
    })
}

fn extract_error_text(body: &str) -> String {
    if body.trim().is_empty() {
        return "empty response body".to_string();
    }
    if let Ok(response) = OpenAiResponse::deserialize_json_lenient(body) {
        if let Some(error) = response.error {
            if let Some(message) = error.message {
                return message;
            }
        }
    }
    body.trim().to_string()
}

fn first_non_empty_reasoning(message: &OpenAiResponseMessage) -> Option<String> {
    [
        message.reasoning_content.as_deref(),
        message.reasoning.as_deref(),
        message.reasoning_text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.trim().is_empty())
    .map(ToOwned::to_owned)
}

fn first_non_empty_stream_reasoning(delta: &OpenAiStreamDelta) -> Option<String> {
    [
        delta.reasoning_content.as_deref(),
        delta.reasoning.as_deref(),
        delta.reasoning_text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.trim().is_empty())
    .map(ToOwned::to_owned)
}

fn drain_sse_events(buffer: &mut String, flush: bool) -> Vec<String> {
    let mut events = Vec::new();
    while let Some(index) = buffer.find("\n\n") {
        let event = buffer[..index].to_string();
        buffer.drain(..index + 2);
        events.push(event);
    }
    if flush {
        let trailing = buffer.trim();
        if !trailing.is_empty() {
            events.push(trailing.to_string());
        }
        buffer.clear();
    }
    events
}

fn extract_sse_event_data(event: &str) -> Option<String> {
    let mut out = String::new();
    for line in event.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.strip_prefix(' ').unwrap_or(data);
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(data);
    }
    (!out.is_empty()).then_some(out)
}

fn apply_tool_call_delta(
    tool_calls: &mut Vec<ToolCallAccumulator>,
    delta: OpenAiStreamToolCallDelta,
) -> Result<(), String> {
    if let Some(kind) = &delta.kind {
        if kind != "function" {
            return Err(format!("unsupported streamed tool call type '{}'", kind));
        }
    }
    let index = delta.index.unwrap_or(0) as usize;
    while tool_calls.len() <= index {
        tool_calls.push(ToolCallAccumulator::default());
    }
    let tool_call = &mut tool_calls[index];
    if let Some(id) = delta.id {
        tool_call.id = id;
    }
    if let Some(function) = delta.function {
        if let Some(name) = function.name {
            tool_call.name = name;
        }
        if let Some(arguments) = function.arguments {
            tool_call.arguments_json.push_str(&arguments);
        }
    }
    Ok(())
}

fn upsert_stream_message(
    messages: &mut Vec<AiMessage>,
    existing_index: Option<usize>,
    role: AiMessageRole,
    text: &str,
) -> Option<usize> {
    if text.trim().is_empty() {
        return existing_index;
    }
    if let Some(index) = existing_index {
        if let Some(message) = messages.get_mut(index) {
            message.text = text.to_string();
            return Some(index);
        }
    }
    messages.push(AiMessage {
        role,
        text: text.to_string(),
    });
    Some(messages.len() - 1)
}

fn finalize_stream_turn(
    stream: StreamingTurnState,
) -> Result<(AssistantTurn, StreamVisibleState), String> {
    let tool_calls = stream
        .tool_calls
        .into_iter()
        .filter(|tool_call| {
            !tool_call.id.is_empty()
                || !tool_call.name.is_empty()
                || !tool_call.arguments_json.is_empty()
        })
        .map(|tool_call| {
            if tool_call.id.is_empty() {
                return Err("AI backend streamed a tool call without an id".to_string());
            }
            if tool_call.name.is_empty() {
                return Err("AI backend streamed a tool call without a name".to_string());
            }
            Ok(ToolCallRecord {
                id: tool_call.id,
                name: tool_call.name,
                arguments_json: tool_call.arguments_json,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((
        AssistantTurn {
            text: stream.assistant_text,
            thinking_text: stream.thinking_text,
            tool_calls,
        },
        stream.visible,
    ))
}

fn execute_tool_call(
    root_path: &Path,
    mount: &str,
    event_tx: &Sender<HubEvent>,
    tool_call: &ToolCallRecord,
) -> AiToolExecutionResult {
    let result = match tool_call.name.as_str() {
        "read_file" => ReadFileArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid read_file arguments: {:?}", err))
            .and_then(|args| tool_read_file(root_path, args)),
        "list_files" => ListFilesArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid list_files arguments: {:?}", err))
            .and_then(|args| tool_list_files(root_path, args)),
        "search_text" => SearchTextArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid search_text arguments: {:?}", err))
            .and_then(|args| tool_search_text(root_path, args)),
        "write_file" => WriteFileArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid write_file arguments: {:?}", err))
            .and_then(|args| tool_write_file(root_path, args)),
        "replace_in_file" => ReplaceInFileArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid replace_in_file arguments: {:?}", err))
            .and_then(|args| tool_replace_in_file(root_path, args)),
        "open_editor" => OpenEditorArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid open_editor arguments: {:?}", err))
            .and_then(|args| tool_open_editor(root_path, mount, event_tx, args)),
        "observe_filesystem" => ObserveFilesystemArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid observe_filesystem arguments: {:?}", err))
            .and_then(|args| tool_observe_filesystem(root_path, mount, event_tx, args)),
        "open_terminal" => OpenTerminalArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid open_terminal arguments: {:?}", err))
            .and_then(|args| tool_open_terminal(mount, event_tx, args)),
        "list_terminals" => tool_list_terminals(mount, event_tx),
        "read_terminal" => ReadTerminalArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid read_terminal arguments: {:?}", err))
            .and_then(|args| tool_read_terminal(mount, event_tx, args)),
        "send_terminal_text" => SendTerminalTextArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid send_terminal_text arguments: {:?}", err))
            .and_then(|args| tool_send_terminal_text(mount, event_tx, args)),
        "send_terminal_key" => SendTerminalKeyArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid send_terminal_key arguments: {:?}", err))
            .and_then(|args| tool_send_terminal_key(mount, event_tx, args)),
        "bash" => BashArgs::deserialize_json(&tool_call.arguments_json)
            .map_err(|err| format!("invalid bash arguments: {:?}", err))
            .and_then(|args| tool_bash(root_path, args)),
        other => Err(format!("unknown tool '{}'", other)),
    };

    match result {
        Ok(content) => AiToolExecutionResult {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content,
            is_error: false,
        },
        Err(error) => AiToolExecutionResult {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            content: error,
            is_error: true,
        },
    }
}

fn tool_open_terminal(
    mount: &str,
    event_tx: &Sender<HubEvent>,
    args: OpenTerminalArgs,
) -> Result<String, String> {
    request_hub_tool(
        event_tx,
        |reply_tx| HubEvent::AiOpenTerminalRequest {
            mount: mount.to_string(),
            name: args.name.map(|value| value.trim().to_string()),
            command: args.command.map(|value| value.trim().to_string()),
            cols: args.cols.unwrap_or(120).max(1),
            rows: args.rows.unwrap_or(40).max(1),
            reply_tx,
        },
        "failed to request terminal open from hub",
        "timed out waiting for hub to open terminal",
    )
}

fn tool_open_editor(
    root_path: &Path,
    mount: &str,
    event_tx: &Sender<HubEvent>,
    args: OpenEditorArgs,
) -> Result<String, String> {
    let path = resolve_workspace_path(root_path, &args.path)?;
    let metadata =
        fs::metadata(&path).map_err(|err| format!("failed to stat '{}': {}", args.path, err))?;
    if !metadata.is_file() {
        return Err(format!("'{}' is not a file", args.path));
    }
    let virtual_path = format!("{}/{}", mount, display_path(root_path, &path));
    request_hub_tool(
        event_tx,
        |reply_tx| HubEvent::AiOpenEditorRequest {
            mount: mount.to_string(),
            path: virtual_path.clone(),
            line: args
                .line
                .or(args.column.map(|_| 1))
                .map(|value| value.max(1)),
            column: args.column.map(|value| value.max(1)),
            reply_tx,
        },
        "failed to request editor open from hub",
        "timed out waiting for hub to open editor",
    )
}

fn tool_observe_filesystem(
    root_path: &Path,
    mount: &str,
    event_tx: &Sender<HubEvent>,
    args: ObserveFilesystemArgs,
) -> Result<String, String> {
    let path = match args.path {
        Some(path) => {
            let trimmed = path.trim();
            if trimmed.is_empty() || trimmed == "." {
                None
            } else {
                let root = root_path.canonicalize().map_err(|err| {
                    format!(
                        "failed to resolve workspace root '{}': {}",
                        root_path.display(),
                        err
                    )
                })?;
                let path = resolve_workspace_path(&root, trimmed)?;
                if path == root {
                    None
                } else {
                    Some(display_path(&root, &path))
                }
            }
        }
        None => None,
    };
    request_hub_tool(
        event_tx,
        |reply_tx| HubEvent::AiObserveFilesystemRequest {
            mount: mount.to_string(),
            path,
            limit: args
                .limit
                .unwrap_or(DEFAULT_OBSERVE_FILESYSTEM_LIMIT)
                .clamp(1, MAX_OBSERVE_FILESYSTEM_LIMIT),
            since_secs: args
                .since_secs
                .unwrap_or(DEFAULT_OBSERVE_FILESYSTEM_WINDOW_SECS)
                .clamp(1, MAX_OBSERVE_FILESYSTEM_WINDOW_SECS),
            reply_tx,
        },
        "failed to request filesystem observation from hub",
        "timed out waiting for hub filesystem observation",
    )
}

fn tool_list_terminals(mount: &str, event_tx: &Sender<HubEvent>) -> Result<String, String> {
    request_hub_tool(
        event_tx,
        |reply_tx| HubEvent::AiListTerminalsRequest {
            mount: mount.to_string(),
            reply_tx,
        },
        "failed to request terminal list from hub",
        "timed out waiting for hub to list terminals",
    )
}

fn tool_read_terminal(
    mount: &str,
    event_tx: &Sender<HubEvent>,
    args: ReadTerminalArgs,
) -> Result<String, String> {
    request_hub_tool(
        event_tx,
        |reply_tx| HubEvent::AiReadTerminalRequest {
            mount: mount.to_string(),
            path: args.path.trim().to_string(),
            rows: args.rows.map(|value| value.max(1)),
            top_row: args.top_row,
            reply_tx,
        },
        "failed to request terminal read from hub",
        "timed out waiting for hub to read terminal",
    )
}

fn tool_send_terminal_text(
    mount: &str,
    event_tx: &Sender<HubEvent>,
    args: SendTerminalTextArgs,
) -> Result<String, String> {
    request_hub_tool(
        event_tx,
        |reply_tx| HubEvent::AiSendTerminalTextRequest {
            mount: mount.to_string(),
            path: args.path.trim().to_string(),
            text: args.text,
            submit: args.submit,
            bracketed_paste: args.bracketed_paste,
            reply_tx,
        },
        "failed to request terminal text input from hub",
        "timed out waiting for hub to send terminal text",
    )
}

fn tool_send_terminal_key(
    mount: &str,
    event_tx: &Sender<HubEvent>,
    args: SendTerminalKeyArgs,
) -> Result<String, String> {
    request_hub_tool(
        event_tx,
        |reply_tx| HubEvent::AiSendTerminalKeyRequest {
            mount: mount.to_string(),
            path: args.path.trim().to_string(),
            key: args.key.trim().to_string(),
            shift: args.shift.unwrap_or(false),
            control: args.control.unwrap_or(false),
            alt: args.alt.unwrap_or(false),
            reply_tx,
        },
        "failed to request terminal key input from hub",
        "timed out waiting for hub to send terminal key",
    )
}

fn request_hub_tool(
    event_tx: &Sender<HubEvent>,
    build_event: impl FnOnce(Sender<Result<String, String>>) -> HubEvent,
    send_error: &str,
    timeout_error: &str,
) -> Result<String, String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    event_tx
        .send(build_event(reply_tx))
        .map_err(|_| send_error.to_string())?;
    reply_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| timeout_error.to_string())?
}

fn tool_read_file(root_path: &Path, args: ReadFileArgs) -> Result<String, String> {
    let path = resolve_workspace_path(root_path, &args.path)?;
    let bytes =
        fs::read(&path).map_err(|err| format!("failed to read '{}': {}", args.path, err))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!(
            "'{}' is too large to read directly ({} bytes)",
            args.path,
            bytes.len()
        ));
    }
    if bytes.iter().any(|byte| *byte == 0) {
        return Err(format!("'{}' looks like a binary file", args.path));
    }
    let text =
        String::from_utf8(bytes).map_err(|_| format!("'{}' is not valid UTF-8", args.path))?;
    let lines = text.lines().collect::<Vec<_>>();
    let total_lines = lines.len().max(1);
    let start_line = args.offset.unwrap_or(1).max(1);
    if start_line > total_lines {
        return Err(format!(
            "offset {} is beyond the end of '{}', which has {} lines",
            start_line, args.path, total_lines
        ));
    }
    let limit = args.limit.unwrap_or(DEFAULT_READ_LIMIT).clamp(1, 500);
    let start_index = start_line - 1;
    let end_index = (start_index + limit).min(lines.len());
    let mut out = String::new();
    for (index, line) in lines[start_index..end_index].iter().enumerate() {
        let line_no = start_index + index + 1;
        out.push_str(&format!("{:>6} | {}\n", line_no, line));
    }
    if end_index < lines.len() {
        out.push_str(&format!(
            "\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
            start_index + 1,
            end_index,
            lines.len(),
            end_index + 1
        ));
    }
    Ok(truncate_text(&out, MAX_RESULT_CHARS))
}

fn tool_list_files(root_path: &Path, args: ListFilesArgs) -> Result<String, String> {
    let path_arg = args.path.unwrap_or_else(|| ".".to_string());
    let path = resolve_workspace_path(root_path, &path_arg)?;
    let limit = args.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, 500);
    let mut entries = Vec::new();
    collect_paths(root_path, &path, &mut entries, limit)?;
    if entries.is_empty() {
        return Ok("No files found.".to_string());
    }
    entries.sort();
    let mut out = entries.join("\n");
    if entries.len() >= limit {
        out.push_str(&format!("\n\n[Stopped after {} entries.]", limit));
    }
    Ok(truncate_text(&out, MAX_RESULT_CHARS))
}

fn tool_search_text(root_path: &Path, args: SearchTextArgs) -> Result<String, String> {
    let search_root = resolve_workspace_path(root_path, args.path.as_deref().unwrap_or("."))?;
    let pattern = args.pattern.trim();
    if pattern.is_empty() {
        return Err("search pattern cannot be empty".to_string());
    }
    let limit = args.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).clamp(1, 500);
    let mut matches = Vec::new();
    search_paths(root_path, &search_root, pattern, &mut matches, limit)?;
    if matches.is_empty() {
        return Ok(format!("No matches found for '{}'.", pattern));
    }
    let mut out = matches.join("\n");
    if matches.len() >= limit {
        out.push_str(&format!("\n\n[Stopped after {} matches.]", limit));
    }
    Ok(truncate_text(&out, MAX_RESULT_CHARS))
}

fn tool_write_file(root_path: &Path, args: WriteFileArgs) -> Result<String, String> {
    let path = resolve_workspace_path(root_path, &args.path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create parent directories for '{}': {}",
                args.path, err
            )
        })?;
    }
    fs::write(&path, args.content.as_bytes())
        .map_err(|err| format!("failed to write '{}': {}", args.path, err))?;
    Ok(format!(
        "Wrote {} bytes to {}.",
        args.content.len(),
        display_path(root_path, &path)
    ))
}

fn tool_replace_in_file(root_path: &Path, args: ReplaceInFileArgs) -> Result<String, String> {
    let path = resolve_workspace_path(root_path, &args.path)?;
    let text = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read '{}': {}", args.path, err))?;
    if args.old_text.is_empty() {
        return Err("old_text cannot be empty".to_string());
    }
    let match_count = text.matches(&args.old_text).count();
    if match_count == 0 {
        return Err(format!(
            "'{}' does not contain the requested text",
            args.path
        ));
    }
    let replace_all = args.replace_all.unwrap_or(false);
    let new_text = if replace_all {
        text.replace(&args.old_text, &args.new_text)
    } else {
        text.replacen(&args.old_text, &args.new_text, 1)
    };
    fs::write(&path, new_text.as_bytes())
        .map_err(|err| format!("failed to write '{}': {}", args.path, err))?;
    Ok(format!(
        "Updated {}. Replaced {} occurrence{}.",
        display_path(root_path, &path),
        if replace_all { match_count } else { 1 },
        if replace_all && match_count != 1 {
            "s"
        } else {
            ""
        }
    ))
}

fn tool_bash(root_path: &Path, args: BashArgs) -> Result<String, String> {
    let timeout_secs = args
        .timeout_secs
        .unwrap_or(DEFAULT_BASH_TIMEOUT_SECS)
        .clamp(1, MAX_BASH_TIMEOUT_SECS);
    let result = run_shell_command(root_path, &args.command, timeout_secs)?;
    let mut out = String::new();
    out.push_str(&format!("$ {}\n", args.command));
    out.push_str(&result.output);
    out.push_str(&format!("\n[exit code: {}]", result.exit_code));
    if result.timed_out {
        out.push_str(" [timed out]");
    }
    if result.exit_code != 0 || result.timed_out {
        return Err(truncate_text(&out, MAX_RESULT_CHARS));
    }
    Ok(truncate_text(&out, MAX_RESULT_CHARS))
}

fn collect_paths(
    root_path: &Path,
    current: &Path,
    out: &mut Vec<String>,
    limit: usize,
) -> Result<(), String> {
    if out.len() >= limit {
        return Ok(());
    }
    let metadata = fs::metadata(current)
        .map_err(|err| format!("failed to stat '{}': {}", current.display(), err))?;
    if metadata.is_file() {
        out.push(display_path(root_path, current));
        return Ok(());
    }
    let mut entries = fs::read_dir(current)
        .map_err(|err| format!("failed to list '{}': {}", current.display(), err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to list '{}': {}", current.display(), err))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        if should_skip_path(&path) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            out.push(format!("{}/", display_path(root_path, &path)));
            collect_paths(root_path, &path, out, limit)?;
        } else if metadata.is_file() {
            out.push(display_path(root_path, &path));
        }
    }
    Ok(())
}

fn search_paths(
    root_path: &Path,
    current: &Path,
    pattern: &str,
    out: &mut Vec<String>,
    limit: usize,
) -> Result<(), String> {
    if out.len() >= limit {
        return Ok(());
    }
    let metadata = fs::metadata(current)
        .map_err(|err| format!("failed to stat '{}': {}", current.display(), err))?;
    if metadata.is_file() {
        search_file(root_path, current, pattern, out, limit)?;
        return Ok(());
    }
    let mut entries = fs::read_dir(current)
        .map_err(|err| format!("failed to list '{}': {}", current.display(), err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to list '{}': {}", current.display(), err))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        if should_skip_path(&path) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            search_paths(root_path, &path, pattern, out, limit)?;
        } else if metadata.is_file() {
            search_file(root_path, &path, pattern, out, limit)?;
        }
    }
    Ok(())
}

fn search_file(
    root_path: &Path,
    path: &Path,
    pattern: &str,
    out: &mut Vec<String>,
    limit: usize,
) -> Result<(), String> {
    if out.len() >= limit {
        return Ok(());
    }
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read '{}': {}", path.display(), err))?;
    if bytes.len() > MAX_FILE_BYTES || bytes.iter().any(|byte| *byte == 0) {
        return Ok(());
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Ok(()),
    };
    let rel = display_path(root_path, path);
    for (index, line) in text.lines().enumerate() {
        if line.contains(pattern) {
            out.push(format!("{}:{}: {}", rel, index + 1, line.trim()));
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(())
}

fn should_skip_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| matches!(name, ".git" | "target" | "node_modules"))
        .unwrap_or(false)
}

fn resolve_workspace_path(root_path: &Path, raw_path: &str) -> Result<PathBuf, String> {
    let root = root_path.canonicalize().map_err(|err| {
        format!(
            "failed to resolve workspace root '{}': {}",
            root_path.display(),
            err
        )
    })?;
    let input = Path::new(raw_path);
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let normalized = normalize_path(&candidate);
    if !normalized.starts_with(&root) {
        return Err(format!("path '{}' escapes the workspace root", raw_path));
    }
    Ok(normalized)
}

fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn display_path(root_path: &Path, path: &Path) -> String {
    path.strip_prefix(root_path)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
}

struct CommandRunResult {
    output: String,
    exit_code: i32,
    timed_out: bool,
}

fn run_shell_command(
    root_path: &Path,
    command: &str,
    timeout_secs: u64,
) -> Result<CommandRunResult, String> {
    #[cfg(windows)]
    let (shell, shell_args) = ("cmd", vec!["/C".to_string(), command.to_string()]);
    #[cfg(not(windows))]
    let (shell, shell_args) = ("/bin/sh", vec!["-lc".to_string(), command.to_string()]);

    let mut child = Command::new(shell)
        .args(shell_args)
        .current_dir(root_path)
        .env("TERM", "dumb")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn shell command: {}", err))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_thread = thread::spawn(move || read_pipe(stdout));
    let stderr_thread = thread::spawn(move || read_pipe(stderr));

    let started = Instant::now();
    let mut timed_out = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if started.elapsed() >= Duration::from_secs(timeout_secs) {
                    timed_out = true;
                    let _ = child.kill();
                    let status = child
                        .wait()
                        .map_err(|err| format!("failed to stop timed out command: {}", err))?;
                    break status.code().unwrap_or(-1);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(format!("failed while waiting for command: {}", err)),
        }
    };

    let stdout = stdout_thread
        .join()
        .unwrap_or_else(|_| Ok(String::new()))
        .map_err(|err| format!("failed to read command stdout: {}", err))?;
    let stderr = stderr_thread
        .join()
        .unwrap_or_else(|_| Ok(String::new()))
        .map_err(|err| format!("failed to read command stderr: {}", err))?;

    let mut output = String::new();
    if !stdout.trim().is_empty() {
        output.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(stderr.trim_end());
    }
    if output.is_empty() {
        output = "[no output]".to_string();
    }

    Ok(CommandRunResult {
        output: truncate_text(&output, MAX_RESULT_CHARS),
        exit_code,
        timed_out,
    })
}

fn read_pipe(pipe: Option<impl Read>) -> Result<String, std::io::Error> {
    let Some(mut pipe) = pipe else {
        return Ok(String::new());
    };
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn format_tool_call_message(tool_call: &ToolCallRecord) -> String {
    format!(
        "`{}`\n```json\n{}\n```",
        tool_call.name,
        tool_call.arguments_json.trim()
    )
}

fn format_tool_result_message(result: &AiToolExecutionResult) -> String {
    let label = if result.is_error {
        format!("`{}` failed", result.tool_name)
    } else {
        format!("`{}` result", result.tool_name)
    };
    format!(
        "{}\n```text\n{}\n```",
        label,
        truncate_text(result.content.trim(), MAX_RESULT_CHARS)
    )
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push_str("\n\n[output truncated]");
    }
    out
}

fn json_string(value: &str) -> String {
    value.to_string().serialize_json()
}

fn summarize_title(prompt: &str) -> String {
    let single_line = prompt
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .replace('\t', " ");
    if single_line.is_empty() {
        return String::new();
    }
    let mut title = single_line.chars().take(40).collect::<String>();
    if single_line.chars().count() > 40 {
        title.push_str("...");
    }
    title
}

fn is_terminal_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_terminal" | "send_terminal_text" | "send_terminal_key" | "open_terminal"
    )
}

fn should_track_ai_terminal_task(prompt: &str) -> bool {
    let lowered = prompt.to_lowercase();
    lowered.contains("codex")
        || lowered.contains("terminal")
        || lowered.contains("other agent")
        || lowered.contains("tell ") && lowered.contains(" to ")
}

fn extract_expected_paths_from_prompt(prompt: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in prompt.split_whitespace() {
        let token = raw.trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
            )
        });
        if token.is_empty() || token.starts_with('-') {
            continue;
        }
        let looks_like_path = token.contains('/')
            || token.contains('\\')
            || token.rsplit_once('.').is_some_and(|(_, ext)| {
                !ext.is_empty()
                    && ext.len() <= 8
                    && ext.chars().all(|ch| ch.is_ascii_alphanumeric())
            });
        if !looks_like_path {
            continue;
        }
        let normalized = token.replace('\\', "/");
        if !out.iter().any(|existing| existing == &normalized) {
            out.push(normalized);
        }
    }
    out
}

fn matches_expected_path(path: &str, expected_paths: &[String]) -> bool {
    expected_paths.iter().any(|expected| {
        path == expected || path.ends_with(&format!("/{}", expected)) || expected.ends_with(path)
    })
}

fn parse_json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let start = json.find(&needle)? + needle.len();
    let mut out = String::new();
    let mut escaped = false;
    for ch in json[start..].chars() {
        if escaped {
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

fn terminal_display_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn chat_title_ordinal(title: &str) -> u64 {
    title
        .strip_prefix("Chat ")
        .and_then(|suffix| suffix.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn ai_chats_dir(root_path: &Path) -> PathBuf {
    root_path.join(".makepad").join("ai_chats")
}

fn ai_chat_file_path(root_path: &Path, agent_id: AiAgentId) -> PathBuf {
    ai_chats_dir(root_path).join(format!("chat-{:020}.json", agent_id.0))
}

fn read_secret_or_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::fs::read_to_string(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_network::backend::{EventSink, NetworkBackend};
    use makepad_network::{HttpResponse, NetworkError, WsSend};
    use std::collections::BTreeMap;
    use std::sync::mpsc::channel;
    use std::thread;

    #[test]
    fn title_summary_trims_and_truncates() {
        assert_eq!(summarize_title("  hello world  "), "hello world");
        assert_eq!(
            summarize_title("01234567890123456789012345678901234567890"),
            "0123456789012345678901234567890123456789..."
        );
    }

    #[test]
    fn extracts_expected_paths_from_prompt() {
        assert_eq!(
            extract_expected_paths_from_prompt("tell codex to write a poem into `poem.txt`"),
            vec!["poem.txt".to_string()]
        );
    }

    #[test]
    fn matches_expected_path_handles_relative_targets() {
        assert!(matches_expected_path(
            "src/poem.txt",
            &[String::from("poem.txt")]
        ));
        assert!(matches_expected_path(
            "poem.txt",
            &[String::from("poem.txt")]
        ));
    }

    #[test]
    fn terminal_mode_detects_codex_working_status() {
        let text = "\n\nWorking (12s) esc to interrupt\n";
        let (mode, is_codex, _summary, codex_status) =
            AiManager::terminal_mode_and_summary("codex", text);
        assert_eq!(mode, "working");
        assert!(is_codex);
        assert_eq!(
            codex_status.as_deref(),
            Some("Working (12s) esc to interrupt")
        );
    }

    #[test]
    fn terminal_observation_updates_hub_live_markdown() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        let state = manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: "repo/.makepad/codex.term".to_string(),
                    terminal_title: "codex".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 42,
                    total_lines: 50,
                    is_tui: true,
                    text: "Working (12s) esc to interrupt\n".to_string(),
                },
            )
            .expect("terminal observation should change state");

        assert!(state.live_markdown.contains("[working / codex]"));
        assert!(state
            .live_markdown
            .contains("Working (12s) esc to interrupt"));
    }

    #[test]
    fn assistant_turn_extracts_tool_calls() {
        let turn = extract_assistant_turn(
            r#"{"choices":[{"message":{"content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}]}}],"error":null}"#,
        )
        .unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "read_file");
    }

    #[test]
    fn assistant_turn_accepts_standard_openai_choice_fields() {
        let turn = extract_assistant_turn(
            r#"{"id":"chatcmpl-1","object":"chat.completion","created":123,"model":"local","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"hello"}}]}"#,
        )
        .unwrap();
        assert_eq!(turn.text, "hello");
        assert_eq!(turn.thinking_text, "");
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn assistant_turn_extracts_reasoning_content() {
        let turn = extract_assistant_turn(
            r#"{"choices":[{"message":{"content":"hello","reasoning_content":"step 1\nstep 2"}}]}"#,
        )
        .unwrap();
        assert_eq!(turn.text, "hello");
        assert_eq!(turn.thinking_text, "step 1\nstep 2");
    }

    #[test]
    fn build_request_body_includes_tool_definitions() {
        let backend = AiBackendConfig {
            id: LOCAL_BACKEND_ID.to_string(),
            label: String::new(),
            detail: String::new(),
            url: DEFAULT_LOCAL_BASE_URL.to_string(),
            model: String::new(),
            api_key: None,
            disable_thinking_via_chat_template: false,
        };
        let body = build_request_body(&backend, "repo", "/tmp/repo", &[]);
        assert!(body.contains("\"tools\""));
        assert!(body.contains("\"read_file\""));
        assert!(body.contains("\"open_editor\""));
        assert!(body.contains("\"observe_filesystem\""));
        assert!(body.contains("\"open_terminal\""));
        assert!(body.contains("\"list_terminals\""));
        assert!(body.contains("\"read_terminal\""));
        assert!(body.contains("\"send_terminal_text\""));
        assert!(body.contains("\"send_terminal_key\""));
        assert!(!body.contains("\"model\""));
        assert!(!body.contains("\"chat_template_kwargs\":{\"enable_thinking\":false}"));
    }

    #[test]
    fn render_system_prompt_replaces_mount_and_root_placeholders() {
        let prompt = render_system_prompt("repo", "/tmp/repo");
        assert!(prompt.contains("mount 'repo'"));
        assert!(prompt.contains("rooted at '/tmp/repo'"));
        assert!(prompt.contains("observe_filesystem"));
        assert!(prompt.contains("open_editor"));
        assert!(prompt.contains("send_terminal_text.submit"));
    }

    struct TestBackend;

    impl NetworkBackend for TestBackend {
        fn http_start(
            &self,
            request_id: LiveId,
            _request: HttpRequest,
            sink: EventSink,
        ) -> Result<(), NetworkError> {
            sink.emit(NetworkResponse::HttpResponse {
                request_id,
                response: HttpResponse::new(LiveId(1), 200, BTreeMap::new(), Some(b"ok".to_vec())),
            })
        }

        fn http_cancel(&self, _request_id: LiveId) -> Result<(), NetworkError> {
            Ok(())
        }

        fn ws_open(
            &self,
            _socket_id: LiveId,
            _request: HttpRequest,
            _sink: EventSink,
        ) -> Result<(), NetworkError> {
            Ok(())
        }

        fn ws_send(&self, _socket_id: LiveId, _message: WsSend) -> Result<(), NetworkError> {
            Ok(())
        }

        fn ws_close(&self, _socket_id: LiveId) -> Result<(), NetworkError> {
            Ok(())
        }
    }

    #[test]
    fn runtime_forwarder_survives_idle_gaps() {
        let runtime = Arc::new(NetworkRuntime::with_backend(Arc::new(TestBackend)));
        let (event_tx, event_rx) = channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let forwarder_runtime = Arc::clone(&runtime);
        let forwarder_shutdown = Arc::clone(&shutdown);
        let join = thread::spawn(move || {
            forward_runtime_events(
                forwarder_runtime,
                event_tx,
                Duration::from_millis(10),
                forwarder_shutdown,
            );
        });

        thread::sleep(Duration::from_millis(35));
        runtime
            .http_start(
                LiveId(7),
                HttpRequest::new("https://example.com".to_string(), HttpMethod::GET),
            )
            .unwrap();

        let event = event_rx.recv_timeout(Duration::from_millis(100)).unwrap();
        match event {
            HubEvent::AiHttpResponse {
                response:
                    NetworkResponse::HttpResponse {
                        request_id,
                        response,
                    },
            } => {
                assert_eq!(request_id, LiveId(7));
                assert_eq!(response.status_code, 200);
            }
            other => panic!("unexpected forwarded event: {other:?}"),
        }

        shutdown.store(true, Ordering::Relaxed);
        join.join().unwrap();
    }
}
