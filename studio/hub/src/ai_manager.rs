use crate::dispatch::HubEvent;
mod providers;

use makepad_chatgpt_provider::{
    ChatGptContentBlock, ChatGptCredentials, ChatGptMessage, ChatGptMessageRole, ChatGptModel,
    ChatGptOAuthConfig, ChatGptProvider, ChatGptRequest, ChatGptTokenResponse, ChatGptTool,
};
use makepad_live_id::LiveId;
use makepad_micro_serde::*;
use makepad_network::{NetworkConfig, NetworkResponse, NetworkRuntime};
use makepad_studio_protocol::ai_format::{
    parse_json_string_field, AI_TASK_EVENT_PREFIX, AI_TERMINAL_OBSERVATION_PREFIX,
    AI_WAITING_MESSAGE_PREFIX,
};
use makepad_studio_protocol::hub_protocol::{
    ActiveWorkflowState, AiAgentId, AiAgentState, AiAgentSummary, AiBackendInfo, AiMessage,
    AiMessageRole, AiMountState, WorkflowStepState,
};
use providers::AiProviderKind;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const LOCAL_BACKEND_ID: &str = "openai_localhost";
const CLOUD_BACKEND_ID: &str = "openai";
const CHATGPT_BACKEND_ID: &str = "chatgpt";
const DEFAULT_LOCAL_BASE_URL: &str = "http://10.0.0.217:8080/v1/chat/completions";
const DEFAULT_LOCAL_MODEL: &str = "";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";
const DEFAULT_CHATGPT_MODEL: &str = "gpt-5.4-mini";
const DEFAULT_MAX_TOKENS: u32 = 2048;
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
    chatgpt: Option<ChatGptProvider>,
    configured: bool,
    configuration_url: Option<String>,
    configuration_hint: Option<String>,
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
    pending_tool_message_start: Option<usize>,
    cancel_requested: bool,
    run_token: u64,
    messages: Vec<AiMessage>,
    history: Vec<ConversationItem>,
    updated_at: f64,
    parent_agent_id: Option<AiAgentId>,
    role: Option<String>,
    task: Option<String>,
    subagents: Vec<AiAgentId>,
    current_action: Option<String>,
    last_terminal_excerpt: Option<String>,
    files_touched: Vec<String>,
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
    active_workflow: Option<ActiveWorkflowState>,
    skills: Vec<ParsedSkill>,
    workflows: Vec<ParsedWorkflow>,
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
    handled_followup_signatures: Vec<String>,
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
    provider_kind: AiProviderKind,
    stream: StreamingTurnState,
}

struct PendingChatGptOAuth {
    mount: String,
    backend_id: String,
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
    raw_event_sample: String,
    saw_text_delta: bool,
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
    parent_agent_id: Option<AiAgentId>,
    role: Option<String>,
    task: Option<String>,
    subagents: Option<Vec<AiAgentId>>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
struct PersistedChatGptCredentials {
    access_token: String,
    refresh_token: Option<String>,
    account_id: Option<String>,
    expires_at_unix: Option<u64>,
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

#[derive(DeJson)]
struct SpawnSubagentArgs {
    role: String,
    task: String,
    model_override: Option<String>,
}

#[derive(DeJson)]
struct CompleteTaskArgs {
    summary: String,
    success: bool,
}

struct AssistantTurn {
    text: String,
    thinking_text: String,
    tool_calls: Vec<ToolCallRecord>,
    raw_event_sample: String,
}

#[derive(Clone, Debug)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct ParsedWorkflow {
    pub name: String,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Clone, Debug)]
pub struct WorkflowStep {
    pub name: String,
    pub description: String,
}

pub struct AiManager {
    event_tx: Sender<HubEvent>,
    runtime: Arc<NetworkRuntime>,
    backends: Vec<AiBackendConfig>,
    mounts: HashMap<String, MountAgents>,
    inflight: HashMap<LiveId, InFlightRequest>,
    pending_chatgpt_oauth: HashMap<LiveId, PendingChatGptOAuth>,
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
            pending_chatgpt_oauth: HashMap::new(),
            next_agent_id: 1,
            next_run_token: 1,
        }
    }

    pub fn register_mount(&mut self, mount: &str, root: &Path) {
        let default_backend_id = self.default_backend_id();
        let root_path_str = root.to_string_lossy().to_string();
        let skills = self.load_skills_for_mount(&root_path_str);
        let workflows = self.load_workflows_for_mount(&root_path_str);
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
                    active_workflow: None,
                    skills: Vec::new(),
                    workflows: Vec::new(),
                });
            entry.skills = skills;
            entry.workflows = workflows;
            entry.root_path = root_path_str;
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
                pending_tool_message_start: None,
                cancel_requested: false,
                run_token: 0,
                messages: Vec::new(),
                history: Vec::new(),
                updated_at: now_seconds(),
                parent_agent_id: None,
                role: None,
                task: None,
                subagents: Vec::new(),
                current_action: None,
                last_terminal_excerpt: None,
                files_touched: Vec::new(),
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
                if let Some(parent_id) = agent.parent_agent_id {
                    if let Some(parent) = mount_state.agents.get_mut(&parent_id) {
                        parent.subagents.retain(|id| *id != agent_id);
                    }
                }
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

    pub fn configure_backend(&mut self, mount: &str, backend_id: &str) -> AiMountState {
        self.ensure_mount_entry(mount);
        let Some(backend) = self.backend_by_id(backend_id).cloned() else {
            return self.snapshot(mount);
        };
        if backend.id == CHATGPT_BACKEND_ID && !backend.configured {
            self.start_chatgpt_oauth_listener(mount, backend_id);
        }
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            mount_state.active_backend_id = backend_id.to_string();
            if let Some(agent_id) = mount_state.active_agent_id {
                if let Some(agent) = mount_state.agents.get_mut(&agent_id) {
                    agent.backend_id = backend_id.to_string();
                    agent.messages.push(AiMessage {
                        role: AiMessageRole::System,
                        text: backend_configuration_message(&backend),
                    });
                    agent.status = if backend.configured {
                        "backend configured".to_string()
                    } else {
                        "backend needs configuration".to_string()
                    };
                    agent.updated_at = now_seconds();
                }
            }
        }
        self.persist_mount_state_best_effort(mount);
        self.snapshot(mount)
    }

    pub fn handle_chatgpt_oauth_code(
        &mut self,
        mount: &str,
        backend_id: &str,
        code: &str,
    ) -> AiMountState {
        self.ensure_mount_entry(mount);
        let Some(backend_index) = self
            .backends
            .iter()
            .position(|backend| backend.id == backend_id)
        else {
            return self.snapshot(mount);
        };
        let Some(provider) = self.backends[backend_index].chatgpt.clone() else {
            return self.snapshot(mount);
        };
        let Some(verifier) = chatgpt_pkce_verifier_from_hint(
            self.backends[backend_index].configuration_hint.as_deref(),
        ) else {
            self.append_backend_system_message(
                mount,
                backend_id,
                "ChatGPT authorization failed: missing PKCE verifier. Start configuration again.",
            );
            return self.snapshot(mount);
        };
        let body = match provider.authorization_request_body(code, &verifier) {
            Ok(body) => body,
            Err(error) => {
                self.append_backend_system_message(
                    mount,
                    backend_id,
                    &format!("ChatGPT authorization failed: {}", error),
                );
                return self.snapshot(mount);
            }
        };
        let request = provider.build_token_request(body);
        let request_id = LiveId::unique();
        self.pending_chatgpt_oauth.insert(
            request_id,
            PendingChatGptOAuth {
                mount: mount.to_string(),
                backend_id: backend_id.to_string(),
            },
        );
        if let Err(error) = self.runtime.http_start(request_id, request) {
            self.pending_chatgpt_oauth.remove(&request_id);
            self.append_backend_system_message(
                mount,
                backend_id,
                &format!("ChatGPT token exchange failed to start: {:?}", error),
            );
        } else {
            self.append_backend_system_message(
                mount,
                backend_id,
                "ChatGPT authorization received; exchanging token...",
            );
        }
        self.snapshot(mount)
    }

    pub fn send_prompt(&mut self, mount: &str, agent_id: AiAgentId, text: &str) -> AiMountState {
        self.ensure_mount_entry(mount);
        let prompt = text.trim();
        if prompt.is_empty() {
            return self.snapshot(mount);
        }

        let prompt_accepted = self
            .mounts
            .get(mount)
            .and_then(|mount_state| mount_state.agents.get(&agent_id))
            .map(|agent| !agent.is_pending())
            .unwrap_or(false);
        if !prompt_accepted {
            return self.snapshot(mount);
        }

        let workflow_start = self
            .mounts
            .get(mount)
            .and_then(|mount_state| workflow_prompt_from_command(prompt, &mount_state.workflows));
        let (workflow_to_activate, prompt_text) =
            if let Some((active_workflow, workflow_prompt)) = workflow_start {
                (Some(active_workflow), workflow_prompt)
            } else {
                (None, prompt.to_string())
            };

        let run_token = self.alloc_run_token();
        {
            let Some(mount_state) = self.mounts.get_mut(mount) else {
                return self.snapshot(mount);
            };
            if let Some(active_workflow) = workflow_to_activate {
                mount_state.active_workflow = Some(active_workflow);
            }
            let Some(agent) = mount_state.agents.get_mut(&agent_id) else {
                return self.snapshot(mount);
            };
            if agent.messages.is_empty() && agent.title.starts_with("Chat ") {
                let summary = summarize_title(&prompt_text);
                if !summary.is_empty() {
                    agent.title = summary;
                }
            }
            agent.messages.push(AiMessage {
                role: AiMessageRole::User,
                text: prompt_text.clone(),
            });
            agent.history.push(ConversationItem::User {
                text: prompt_text.clone(),
            });
            agent.pending_request_id = None;
            agent.pending_tool_batch = false;
            agent.pending_tool_message_start = None;
            agent.cancel_requested = false;
            agent.run_token = run_token;
            agent.status = "thinking...".to_string();
            agent.messages.push(AiMessage {
                role: AiMessageRole::Thinking,
                text: String::new(),
            });
            agent.updated_at = now_seconds();
        }

        self.note_ai_prompt_task(mount, agent_id, &prompt_text);
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
                agent.pending_tool_message_start = None;
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
        let mut chat_updates = Vec::new();
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

            let created_observed_task =
                Self::ensure_observed_codex_terminal_task(mount_state, &snapshot);
            if created_observed_task {
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

                if created_observed_task
                    || previous_mode != task.last_terminal_mode
                    || previous_summary != task.last_terminal_summary
                    || previous_excerpt != task.last_terminal_excerpt
                    || previous_codex_status != task.last_codex_status
                {
                    changed = true;
                }
                if created_observed_task
                    || previous_mode != task.last_terminal_mode
                    || previous_summary != task.last_terminal_summary
                    || previous_excerpt != task.last_terminal_excerpt
                    || previous_codex_status != task.last_codex_status
                {
                    chat_updates.push((
                        task.agent_id,
                        format_terminal_observation_message(
                            &snapshot.path,
                            snapshot.mode,
                            &snapshot.summary,
                            snapshot.codex_status.as_deref(),
                            &task.last_terminal_excerpt,
                        ),
                    ));
                }

                if previous_mode != "awaiting-input" && snapshot.mode == "awaiting-input" {
                    queue.push((
                        task.id,
                        terminal_followup_signature("awaiting-input", &snapshot.path, task),
                        "Tracked terminal is awaiting input".to_string(),
                    ));
                } else if previous_mode != "needs-attention" && snapshot.mode == "needs-attention" {
                    queue.push((
                        task.id,
                        terminal_followup_signature("attention", &snapshot.path, task),
                        "Tracked terminal needs attention".to_string(),
                    ));
                } else if previous_mode != "done" && snapshot.mode == "done" {
                    queue.push((
                        task.id,
                        terminal_followup_signature("done", &snapshot.path, task),
                        "Tracked terminal appears done".to_string(),
                    ));
                }
            }

            for (agent_id, message) in chat_updates {
                if let Some(agent) = mount_state.agents.get_mut(&agent_id) {
                    upsert_terminal_observation_message(&mut agent.messages, &message);
                    upsert_terminal_observation_history(&mut agent.history, &message);
                    collapse_repeated_tail_messages(&mut agent.messages);
                    agent.updated_at = now_seconds();
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

    pub fn process_terminal_input(&mut self, mount: &str, path: &str) -> Option<AiMountState> {
        self.ensure_mount_entry(mount);
        let mut changed = false;
        {
            let mount_state = self.mounts.get_mut(mount)?;
            let path_lowered = path.to_lowercase();
            let (is_codex, summary, snapshot) = {
                let snapshot = mount_state
                    .terminal_snapshots
                    .entry(path.to_string())
                    .or_insert_with(|| AiTerminalSnapshot {
                        path: path.to_string(),
                        mode: "input",
                        summary: String::new(),
                        visible_text: String::new(),
                        is_codex: path_lowered.contains("codex"),
                        codex_status: None,
                    });
                let is_codex = snapshot.is_codex || path_lowered.contains("codex");
                let summary = if is_codex {
                    "Input sent to Codex"
                } else {
                    "Input sent to terminal"
                };
                if snapshot.mode != "input"
                    || snapshot.summary != summary
                    || snapshot.is_codex != is_codex
                    || snapshot.codex_status.is_some()
                {
                    snapshot.mode = "input";
                    snapshot.summary = summary.to_string();
                    snapshot.is_codex = is_codex;
                    snapshot.codex_status = None;
                    changed = true;
                }
                (is_codex, summary.to_string(), snapshot.clone())
            };

            if is_codex && Self::ensure_observed_codex_terminal_task(mount_state, &snapshot) {
                changed = true;
            }

            for task in mount_state
                .tasks
                .iter_mut()
                .filter(|task| task.terminal_path.as_deref() == Some(path))
            {
                if task.last_terminal_mode != "input" || task.last_terminal_summary != summary {
                    changed = true;
                }
                task.status = "watching".to_string();
                task.last_terminal_mode = "input".to_string();
                task.last_terminal_summary = summary.clone();
                task.last_codex_status = None;
            }
        }

        if changed {
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
                    terminal_followup_signature("exit", path, task),
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
                if let Some(pending) = self.pending_chatgpt_oauth.remove(&request_id) {
                    let body = response
                        .body
                        .as_ref()
                        .map(|body| String::from_utf8_lossy(body).to_string())
                        .unwrap_or_default();
                    let mount = pending.mount.clone();
                    if response.status_code >= 400 {
                        self.append_backend_system_message(
                            &mount,
                            &pending.backend_id,
                            &format!(
                                "ChatGPT token exchange failed: HTTP {}: {}",
                                response.status_code,
                                extract_error_text(&body)
                            ),
                        );
                        return Some((mount.clone(), self.snapshot(&mount)));
                    }
                    match ChatGptTokenResponse::deserialize_json_lenient(&body) {
                        Ok(token_response) => {
                            self.complete_chatgpt_oauth(&pending, token_response);
                        }
                        Err(error) => {
                            self.append_backend_system_message(
                                &mount,
                                &pending.backend_id,
                                &format!(
                                    "ChatGPT token exchange returned invalid JSON: {:?}",
                                    error
                                ),
                            );
                        }
                    }
                    return Some((mount.clone(), self.snapshot(&mount)));
                }
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

                if in_flight.provider_kind.backend().response_is_stream() {
                    match self.process_stream_data(request_id, &body, true) {
                        Ok(stream_update) => {
                            if stream_update.done {
                                return self.finish_stream_request(request_id);
                            }
                            if stream_update.changed {
                                return Some((mount.clone(), self.snapshot(&mount)));
                            }
                            return Some((mount.clone(), self.snapshot(&mount)));
                        }
                        Err(error) => self.set_agent_error(&mount, agent_id, error),
                    }
                    return Some((mount.clone(), self.snapshot(&mount)));
                }

                match in_flight
                    .provider_kind
                    .backend()
                    .extract_assistant_turn(&body)
                {
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
                if let Some(pending) = self.pending_chatgpt_oauth.remove(&request_id) {
                    let mount = pending.mount.clone();
                    self.append_backend_system_message(
                        &mount,
                        &pending.backend_id,
                        &format!("ChatGPT token exchange network error: {}", error.message),
                    );
                    return Some((mount.clone(), self.snapshot(&mount)));
                }
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
        let waiting_message = if results.len() == 1 {
            format_terminal_waiting_message(&results[0])
        } else {
            None
        };
        if let Some(agent) = self
            .mounts
            .get_mut(mount)
            .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
        {
            agent.pending_tool_batch = false;
            let pending_tool_message_start = agent.pending_tool_message_start.take();
            for result in &results {
                agent.history.push(ConversationItem::ToolResult {
                    tool_call_id: result.tool_call_id.clone(),
                    content: result.content.clone(),
                });
                if waiting_message.is_none() {
                    agent.messages.push(AiMessage {
                        role: AiMessageRole::ToolResult,
                        text: format_tool_result_message(result),
                    });
                }
            }
            if let Some(waiting_message) = waiting_message.clone() {
                if let Some(start) = pending_tool_message_start {
                    if start <= agent.messages.len() {
                        agent.messages.truncate(start);
                    } else {
                        trim_terminal_waiting_tail(&mut agent.messages);
                    }
                } else {
                    trim_terminal_waiting_tail(&mut agent.messages);
                }
                upsert_terminal_waiting_message(&mut agent.messages, waiting_message);
            }
            agent.updated_at = now_seconds();
            if agent.cancel_requested {
                agent.cancel_requested = false;
                agent.status = "cancelled".to_string();
            } else {
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
        let (mount, agent_id) = {
            let in_flight = self.inflight.get(&request_id)?;
            if !self.agent_run_matches(&in_flight.mount, in_flight.agent_id, in_flight.run_token) {
                return None;
            }
            (in_flight.mount.clone(), in_flight.agent_id)
        };
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
        let (mount, agent_id, provider_kind, matches_run) = {
            let in_flight = self.inflight.get(&request_id)?;
            (
                in_flight.mount.clone(),
                in_flight.agent_id,
                in_flight.provider_kind,
                self.agent_run_matches(&in_flight.mount, in_flight.agent_id, in_flight.run_token),
            )
        };
        if !matches_run {
            self.inflight.remove(&request_id);
            return None;
        }
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
        if provider_kind.backend().response_is_stream() {
            if let Err(error) = self.process_stream_data(request_id, &body, true) {
                self.inflight.remove(&request_id);
                self.set_agent_error(&mount, agent_id, error);
                return Some((mount.clone(), self.snapshot(&mount)));
            }
            return self.finish_stream_request(request_id);
        }
        if let Ok(turn) = provider_kind.backend().extract_assistant_turn(&body) {
            let in_flight = self.inflight.remove(&request_id)?;
            return self
                .complete_assistant_turn(
                    &mount,
                    agent_id,
                    in_flight.run_token,
                    turn,
                    Some(in_flight.stream.visible),
                )
                .map(|state| (mount.clone(), state));
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
        let Some((mount, agent_id, run_token, provider_kind)) =
            self.inflight.get(&request_id).map(|in_flight| {
                (
                    in_flight.mount.clone(),
                    in_flight.agent_id,
                    in_flight.run_token,
                    in_flight.provider_kind,
                )
            })
        else {
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

        let deltas = {
            let in_flight = self.inflight.get_mut(&request_id).expect("checked above");
            provider_kind
                .backend()
                .process_stream_events(events, &mut in_flight.stream)?
        };

        {
            let in_flight = self.inflight.get_mut(&request_id).expect("checked above");
            if !deltas.thinking_delta.is_empty() {
                in_flight
                    .stream
                    .thinking_text
                    .push_str(&deltas.thinking_delta);
            }
            if !deltas.assistant_delta.is_empty() {
                in_flight
                    .stream
                    .assistant_text
                    .push_str(&deltas.assistant_delta);
            }
            if let Some(reason) = deltas.finish_reason {
                in_flight.stream.finish_reason = Some(reason);
            }
            if deltas.saw_done {
                in_flight.stream.done_received = true;
            }
            for delta in deltas.tool_call_deltas {
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
        let mut spawn_subagent_call = None;
        let mut complete_task_call = None;
        for tool_call in &turn.tool_calls {
            if tool_call.name == "spawn_subagent" {
                spawn_subagent_call = Some(tool_call.clone());
            } else if tool_call.name == "complete_task" {
                complete_task_call = Some(tool_call.clone());
            }
        }

        let mut pre_sub_id = None;
        let mut pre_sub_run_token = None;
        let mut pre_parent_run_token = None;

        if spawn_subagent_call.is_some() {
            pre_sub_id = Some(self.alloc_agent_id());
            pre_sub_run_token = Some(self.alloc_run_token());
        }
        if complete_task_call.is_some() {
            pre_parent_run_token = Some(self.alloc_run_token());
        }

        if spawn_subagent_call.is_some() || complete_task_call.is_some() {
            let event_tx = self.event_tx.clone();
            if let Some(mount_state) = self.mounts.get_mut(mount) {
                let (parent_id, backend_id) = {
                    if let Some(agent) = mount_state.agents.get(&agent_id) {
                        if agent.run_token != run_token {
                            return None;
                        }
                        (agent.parent_agent_id, agent.backend_id.clone())
                    } else {
                        return None;
                    }
                };

                // Update the current agent first, scoped so the borrow finishes
                {
                    let agent = mount_state.agents.get_mut(&agent_id).unwrap();
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
                                            visible.assistant_message_index =
                                                Some(assistant_index - 1);
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

                    if let Some(call) = &spawn_subagent_call {
                        let args = match SpawnSubagentArgs::deserialize_json(&call.arguments_json) {
                            Ok(args) => args,
                            Err(err) => {
                                agent.history.push(ConversationItem::Assistant {
                                    text: turn.text.clone(),
                                    tool_calls: vec![call.clone()],
                                });
                                agent.messages.push(AiMessage {
                                    role: AiMessageRole::ToolCall,
                                    text: format_tool_call_message(call),
                                });
                                agent.history.push(ConversationItem::ToolResult {
                                    tool_call_id: call.id.clone(),
                                    content: format!("Deserialization error: {:?}", err),
                                });
                                agent.messages.push(AiMessage {
                                    role: AiMessageRole::ToolResult,
                                    text: format!("`spawn_subagent` failed: {:?}", err),
                                });
                                agent.status = "thinking...".to_string();
                                self.persist_mount_state_best_effort(mount);
                                self.start_model_request(mount, agent_id, run_token);
                                return Some(self.snapshot(mount));
                            }
                        };

                        let sub_id = pre_sub_id.unwrap();
                        agent.history.push(ConversationItem::Assistant {
                            text: turn.text.clone(),
                            tool_calls: vec![call.clone()],
                        });
                        agent.messages.push(AiMessage {
                            role: AiMessageRole::ToolCall,
                            text: format_tool_call_message(call),
                        });
                        agent.subagents.push(sub_id);
                        agent.status = format!("waiting for subagent: {}...", args.role);
                    }

                    if let Some(call) = &complete_task_call {
                        let _args = match CompleteTaskArgs::deserialize_json(&call.arguments_json) {
                            Ok(args) => args,
                            Err(err) => {
                                agent.history.push(ConversationItem::Assistant {
                                    text: turn.text.clone(),
                                    tool_calls: vec![call.clone()],
                                });
                                agent.messages.push(AiMessage {
                                    role: AiMessageRole::ToolCall,
                                    text: format_tool_call_message(call),
                                });
                                agent.history.push(ConversationItem::ToolResult {
                                    tool_call_id: call.id.clone(),
                                    content: format!("Deserialization error: {:?}", err),
                                });
                                agent.messages.push(AiMessage {
                                    role: AiMessageRole::ToolResult,
                                    text: format!("`complete_task` failed: {:?}", err),
                                });
                                agent.status = "thinking...".to_string();
                                self.persist_mount_state_best_effort(mount);
                                self.start_model_request(mount, agent_id, run_token);
                                return Some(self.snapshot(mount));
                            }
                        };

                        agent.history.push(ConversationItem::Assistant {
                            text: turn.text.clone(),
                            tool_calls: vec![call.clone()],
                        });
                        agent.messages.push(AiMessage {
                            role: AiMessageRole::ToolCall,
                            text: format_tool_call_message(call),
                        });
                        agent.history.push(ConversationItem::ToolResult {
                            tool_call_id: call.id.clone(),
                            content: "Task completed successfully. Returning control to parent."
                                .to_string(),
                        });
                        agent.messages.push(AiMessage {
                            role: AiMessageRole::ToolResult,
                            text: "Task completed".to_string(),
                        });
                        agent.status = "completed".to_string();
                    }
                }

                if let Some(call) = &spawn_subagent_call {
                    let args = SpawnSubagentArgs::deserialize_json(&call.arguments_json).unwrap();
                    let sub_id = pre_sub_id.unwrap();
                    let sub_run_token = pre_sub_run_token.unwrap();
                    let title = format!("{} Subagent", args.role);
                    let kickoff_prompt = subagent_kickoff_prompt(&args.role, &args.task);

                    let sub_agent = RunningAgent {
                        title: title.clone(),
                        backend_id,
                        status: "thinking...".to_string(),
                        pending_request_id: None,
                        pending_tool_batch: false,
                        pending_tool_message_start: None,
                        cancel_requested: false,
                        run_token: sub_run_token,
                        messages: vec![
                            AiMessage {
                                role: AiMessageRole::User,
                                text: kickoff_prompt.clone(),
                            },
                            AiMessage {
                                role: AiMessageRole::Thinking,
                                text: String::new(),
                            },
                        ],
                        history: vec![ConversationItem::User {
                            text: kickoff_prompt,
                        }],
                        updated_at: now_seconds(),
                        parent_agent_id: Some(agent_id),
                        role: Some(args.role),
                        task: Some(args.task),
                        subagents: Vec::new(),
                        current_action: None,
                        last_terminal_excerpt: None,
                        files_touched: Vec::new(),
                    };

                    mount_state.order.push(sub_id);
                    mount_state.agents.insert(sub_id, sub_agent);
                    mount_state.active_agent_id = Some(sub_id);

                    self.persist_mount_state_best_effort(mount);
                    self.start_model_request(mount, sub_id, sub_run_token);
                    return Some(self.snapshot(mount));
                }

                if let Some(call) = &complete_task_call {
                    let args = CompleteTaskArgs::deserialize_json(&call.arguments_json).unwrap();
                    let parent_run_token = pre_parent_run_token.unwrap();

                    if let Some(parent_id) = parent_id {
                        if let Some(parent) = mount_state.agents.get_mut(&parent_id) {
                            let mut parent_tool_call_id = String::new();
                            if let Some(ConversationItem::Assistant { tool_calls, .. }) =
                                parent.history.last()
                            {
                                if let Some(tc) =
                                    tool_calls.iter().find(|tc| tc.name == "spawn_subagent")
                                {
                                    parent_tool_call_id = tc.id.clone();
                                }
                            }
                            if parent_tool_call_id.is_empty() {
                                parent_tool_call_id = "parent_call_id_placeholder".to_string();
                            }

                            let result_content = format!(
                                "Subagent completed task. Success: {}. Summary: {}",
                                args.success, args.summary
                            );

                            parent.history.push(ConversationItem::ToolResult {
                                tool_call_id: parent_tool_call_id.clone(),
                                content: result_content,
                            });
                            parent.messages.push(AiMessage {
                                role: AiMessageRole::ToolResult,
                                text: format!(
                                    "`spawn_subagent` completed.\nSuccess: {}\nSummary: {}",
                                    args.success, args.summary
                                ),
                            });

                            mount_state.active_agent_id = Some(parent_id);
                            parent.run_token = parent_run_token;
                            parent.status = "thinking...".to_string();
                            parent.messages.push(AiMessage {
                                role: AiMessageRole::Thinking,
                                text: String::new(),
                            });
                            parent.subagents.retain(|id| *id != agent_id);
                            mount_state.order.retain(|id| *id != agent_id);
                            mount_state.agents.remove(&agent_id);
                            remove_agent_file_for_root_best_effort(
                                &event_tx,
                                mount,
                                &mount_state.root_path,
                                agent_id,
                            );

                            self.persist_mount_state_best_effort(mount);
                            self.start_model_request(mount, parent_id, parent_run_token);
                            return Some(self.snapshot(mount));
                        }
                    }

                    if let Some(agent) = mount_state.agents.get_mut(&agent_id) {
                        agent.status = "ready".to_string();
                    }
                    self.persist_mount_state_best_effort(mount);
                    return Some(self.snapshot(mount));
                }
            }
        }

        let mut tool_batch: Option<(PathBuf, Vec<ToolCallRecord>)> = None;
        let mut empty_assistant_response = false;
        let mut retry_empty_terminal_response = false;
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            if let Some(agent) = mount_state.agents.get_mut(&agent_id) {
                if agent.run_token != run_token {
                    return None;
                }
                agent.pending_request_id = None;
                agent.updated_at = now_seconds();

                let visible_message_start = visible
                    .as_ref()
                    .and_then(|visible| {
                        [
                            visible.thinking_message_index,
                            visible.assistant_message_index,
                        ]
                        .into_iter()
                        .flatten()
                        .min()
                    })
                    .unwrap_or(agent.messages.len());

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
                        collapse_repeated_tail_messages(&mut agent.messages);
                    }
                }

                if turn.tool_calls.is_empty() {
                    agent.pending_tool_message_start = None;
                    if turn.text.trim().is_empty() && turn.thinking_text.trim().is_empty() {
                        if let Some(waiting_message) =
                            last_terminal_waiting_message_from_history(&agent.history)
                        {
                            trim_terminal_waiting_tail(&mut agent.messages);
                            upsert_terminal_waiting_message(&mut agent.messages, waiting_message);
                            agent.status = "thinking...".to_string();
                            retry_empty_terminal_response = true;
                        } else {
                            empty_assistant_response = true;
                        }
                    } else {
                        if !turn.text.trim().is_empty() {
                            push_assistant_history_dedup(
                                &mut agent.history,
                                turn.text.clone(),
                                Vec::new(),
                            );
                            collapse_repeated_tail_messages(&mut agent.messages);
                        }
                        agent.status = "ready".to_string();
                        if agent.parent_agent_id.is_none() {
                            Self::advance_active_workflow_step(mount_state);
                        }
                    }
                } else {
                    push_assistant_history_dedup(
                        &mut agent.history,
                        turn.text.clone(),
                        turn.tool_calls.clone(),
                    );
                    let compact_waiting_read =
                        turn.tool_calls.len() == 1 && turn.tool_calls[0].name == "read_terminal";
                    for tool_call in &turn.tool_calls {
                        agent.messages.push(AiMessage {
                            role: AiMessageRole::ToolCall,
                            text: format_tool_call_message(tool_call),
                        });
                    }
                    agent.pending_tool_message_start =
                        compact_waiting_read.then_some(visible_message_start);
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
        if empty_assistant_response {
            let message = if turn.raw_event_sample.trim().is_empty() {
                "AI backend returned an empty assistant response".to_string()
            } else {
                format!(
                    "AI backend returned an empty assistant response.\n\nRaw ChatGPT stream sample:\n```text\n{}\n```",
                    truncate_text(&turn.raw_event_sample, 4000)
                )
            };
            self.set_agent_error(mount, agent_id, message);
        }
        if retry_empty_terminal_response {
            self.start_model_request(mount, agent_id, run_token);
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
            chatgpt: None,
            configured: true,
            configuration_url: None,
            configuration_hint: None,
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
                chatgpt: None,
                configured: true,
                configuration_url: None,
                configuration_hint: None,
                disable_thinking_via_chat_template: false,
            });
        }

        backends.push(detect_chatgpt_backend());

        backends
    }

    fn default_backend_id(&self) -> String {
        self.backends
            .iter()
            .find(|backend| backend.id == CHATGPT_BACKEND_ID && backend.configured)
            .or_else(|| {
                self.backends
                    .iter()
                    .find(|backend| backend.id == LOCAL_BACKEND_ID)
            })
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
                    active_workflow: None,
                    skills: Vec::new(),
                    workflows: Vec::new(),
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
                active_workflow: None,
                skills: Vec::new(),
                workflows: Vec::new(),
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
                pending_tool_message_start: None,
                cancel_requested: false,
                run_token: 0,
                messages: Vec::new(),
                history: Vec::new(),
                updated_at: now_seconds(),
                parent_agent_id: None,
                role: None,
                task: None,
                subagents: Vec::new(),
                current_action: None,
                last_terminal_excerpt: None,
                files_touched: Vec::new(),
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
        let Some((
            backend,
            root_path,
            history,
            role,
            task,
            active_terminals,
            skills,
            workflows,
            active_workflow,
        )) = self.mounts.get(mount).and_then(|mount_state| {
            let agent = mount_state.agents.get(&agent_id)?;
            let mut terms = Vec::new();
            for (path, snap) in &mount_state.terminal_snapshots {
                terms.push(format!(
                    "- `{}`: mode={}, summary='{}'",
                    path, snap.mode, snap.summary
                ));
            }
            Some((
                self.backend_by_id(&agent.backend_id)?.clone(),
                mount_state.root_path.clone(),
                agent.history.clone(),
                agent.role.clone(),
                agent.task.clone(),
                terms,
                mount_state.skills.clone(),
                mount_state.workflows.clone(),
                mount_state.active_workflow.clone(),
            ))
        })
        else {
            self.set_agent_error(mount, agent_id, "backend not available".to_string());
            return;
        };

        let request_id = LiveId::unique();
        let provider_kind = backend.provider_kind();
        let request = match backend.provider_backend().build_http_request(
            &backend,
            mount,
            &root_path,
            &history,
            role.as_deref(),
            task.as_deref(),
            &active_terminals,
            &skills,
            &workflows,
            active_workflow.as_ref(),
        ) {
            Ok(request) => request,
            Err(error) => {
                self.set_agent_error(mount, agent_id, error);
                return;
            }
        };

        let Some(thinking_message_index) = self
            .mounts
            .get_mut(mount)
            .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
            .and_then(|agent| {
                if agent.run_token != run_token {
                    return None;
                }
                let thinking_message_index = agent.messages.len().checked_sub(1).filter(|&index| {
                    agent.messages.get(index).is_some_and(|message| {
                        matches!(message.role, AiMessageRole::Thinking) && message.text.is_empty()
                    })
                });
                agent.pending_request_id = Some(request_id);
                agent.pending_tool_batch = false;
                agent.updated_at = now_seconds();
                Some(thinking_message_index)
            })
        else {
            return;
        };

        self.inflight.insert(
            request_id,
            InFlightRequest {
                mount: mount.to_string(),
                agent_id,
                run_token,
                provider_kind,
                stream: StreamingTurnState {
                    visible: StreamVisibleState {
                        thinking_message_index,
                        assistant_message_index: None,
                    },
                    ..StreamingTurnState::default()
                },
            },
        );

        match self.runtime.http_start(request_id, request) {
            Ok(()) => {}
            Err(err) => {
                self.inflight.remove(&request_id);
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
            handled_followup_signatures: Vec::new(),
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

    fn ensure_observed_codex_terminal_task(
        mount_state: &mut MountAgents,
        snapshot: &AiTerminalSnapshot,
    ) -> bool {
        if !snapshot.is_codex {
            return false;
        }
        if !should_track_observed_terminal_mode(snapshot.mode) {
            return false;
        }
        if mount_state
            .tasks
            .iter()
            .any(|task| task.terminal_path.as_deref() == Some(snapshot.path.as_str()))
        {
            return false;
        }
        let Some(agent_id) = mount_state.active_agent_id else {
            return false;
        };
        let task_id = mount_state.next_task_id.max(1);
        mount_state.next_task_id = task_id.saturating_add(1);
        mount_state.tasks.push(AiTrackedTask {
            id: task_id,
            agent_id,
            goal: format!(
                "Observe Codex terminal `{}`",
                terminal_display_name(&snapshot.path)
            ),
            terminal_path: Some(snapshot.path.clone()),
            expected_paths: Vec::new(),
            touched_paths: Vec::new(),
            status: if snapshot.mode == "needs-attention" {
                "needs-attention".to_string()
            } else if snapshot.mode == "done" || snapshot.mode == "exited" {
                "done".to_string()
            } else {
                "observing".to_string()
            },
            last_terminal_mode: snapshot.mode.to_string(),
            last_terminal_summary: snapshot.summary.clone(),
            last_terminal_excerpt: Self::truncate_terminal_excerpt(
                &snapshot.visible_text,
                AI_TERMINAL_EXCERPT_MAX_CHARS,
                AI_TERMINAL_EXCERPT_MAX_LINES,
            ),
            last_codex_status: snapshot.codex_status.clone(),
            handled_followup_signatures: Vec::new(),
        });
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
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .is_some_and(|task| {
                task.handled_followup_signatures
                    .iter()
                    .any(|handled| handled == &signature)
            })
        {
            return;
        }
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
        if task.last_terminal_mode == "awaiting-input" {
            prompt.push_str(
                "\nThe observed terminal is awaiting input. Decide the next response for that terminal and use `send_terminal_text` with submit=true to continue it. If it is actually finished, tell the user briefly instead.",
            );
        } else {
            prompt.push_str(
                "\nContinue supervising this observed terminal task. If it is finished, tell the user briefly. If more work is needed, use terminal tools instead of guessing.",
            );
        }
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
            if let Some(task) = mount_state
                .tasks
                .iter_mut()
                .find(|task| task.id == queued.task_id)
            {
                if !task
                    .handled_followup_signatures
                    .iter()
                    .any(|signature| signature == &queued.signature)
                {
                    task.handled_followup_signatures
                        .push(queued.signature.clone());
                }
            }
        }
        self.send_prompt(mount, queued.agent_id, &queued.text);
        true
    }
    fn advance_active_workflow_step(mount_state: &mut MountAgents) {
        let Some(workflow) = mount_state.active_workflow.as_mut() else {
            return;
        };
        let Some(current) = workflow.steps.get_mut(workflow.current_step) else {
            return;
        };
        if current.status != "done" {
            current.status = "done".to_string();
        }
        if let Some(next_index) = workflow
            .steps
            .iter()
            .position(|step| step.status == "pending")
        {
            workflow.current_step = next_index;
            if let Some(next) = workflow.steps.get_mut(next_index) {
                next.status = "active".to_string();
            }
        }
    }

    fn ai_live_markdown(&self, mount_state: &MountAgents) -> String {
        let mut markdown = String::new();
        let visible_tasks = mount_state
            .tasks
            .iter()
            .filter(|task| should_show_live_task(task))
            .collect::<Vec<_>>();
        if visible_tasks.is_empty() {
            markdown.push_str("**Todo**\n\n_No open AI todos._");
        } else {
            markdown.push_str("**Todo**\n\n");
            for task in visible_tasks {
                markdown.push_str(&format!(
                    "- `T{}` [{}] {}\n",
                    task.id,
                    task.status,
                    live_task_title(task)
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
        let mut terminals = mount_state
            .terminal_snapshots
            .values()
            .filter(|terminal| should_show_live_terminal(terminal))
            .collect::<Vec<&AiTerminalSnapshot>>();
        if terminals.is_empty() {
            markdown.push_str("_No active terminal activity._");
        } else {
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
        let codex_status = Self::detect_codex_status_line(&lines);
        let codex_prompt_visible = lines
            .iter()
            .rev()
            .take(6)
            .any(|line| Self::is_codex_prompt_line(line));
        let strong_codex_prompt_visible = lines
            .iter()
            .rev()
            .take(6)
            .any(|line| Self::is_strong_codex_prompt_line(line));
        let codex_prompt_has_draft = lines
            .iter()
            .rev()
            .take(6)
            .any(|line| Self::is_codex_prompt_line(line) && Self::codex_prompt_has_draft(line));
        let is_codex = lowered.contains("codex")
            || lowered.contains("apply_patch")
            || lowered.contains("exec_command")
            || lowered.contains("functions.exec_command")
            || lowered.contains("esc to interrupt")
            || lowered.contains("left \u{00b7}")
            || lowered.contains("gpt-5")
            || codex_status.is_some()
            || strong_codex_prompt_visible;
        let codex_status = if is_codex { codex_status } else { None };
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

        let mode = if needs_attention {
            "needs-attention"
        } else if working {
            "working"
        } else if is_codex && codex_prompt_has_draft {
            "awaiting-input"
        } else if awaiting_input {
            "awaiting-input"
        } else if is_codex && codex_prompt_visible && codex_status.is_none() {
            "done"
        } else if visible_text.trim().is_empty() {
            "starting"
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

    fn is_strong_codex_prompt_line(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.starts_with('\u{203a}') || trimmed.contains("Enter a prompt...")
    }

    fn codex_prompt_has_draft(line: &str) -> bool {
        let trimmed = line.trim_start();
        let rest = if let Some(rest) = trimmed.strip_prefix('\u{203a}') {
            rest
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            rest
        } else {
            return false;
        };
        let rest = rest.trim();
        !rest.is_empty() && !rest.contains("Enter a prompt...")
    }

    fn detect_codex_status_line(lines: &[String]) -> Option<String> {
        lines.iter().rev().take(8).find_map(|line| {
            let trimmed = line.trim();
            let lowered = trimmed.to_lowercase();
            if (trimmed.contains("Working (") && trimmed.contains("esc to interrupt"))
                || (lowered.contains("working")
                    && (lowered.contains("esc to interrupt")
                        || lowered.contains("gpt-")
                        || lowered.contains("codex")))
            {
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
                configured: backend.configured,
                configuration_url: backend.configuration_url.clone(),
                configuration_hint: backend.configuration_hint.clone(),
            })
            .collect::<Vec<_>>();
        let agents = mount_state
            .order
            .iter()
            .filter_map(|agent_id| {
                let agent = mount_state.agents.get(agent_id)?;
                if is_closed_subagent(agent) {
                    return None;
                }
                Some(AiAgentSummary {
                    agent_id: *agent_id,
                    title: agent.title.clone(),
                    backend_id: agent.backend_id.clone(),
                    status: agent.status.clone(),
                    pending: agent.is_pending(),
                    updated_at: agent.updated_at,
                    message_count: agent.messages.len(),
                    parent_agent_id: agent.parent_agent_id,
                    role: agent.role.clone(),
                    current_action: agent.current_action.clone(),
                    last_terminal_excerpt: agent.last_terminal_excerpt.clone(),
                    files_touched: agent.files_touched.clone(),
                })
            })
            .collect::<Vec<_>>();
        let active_agent_id = mount_state.active_agent_id.and_then(|agent_id| {
            let agent = mount_state.agents.get(&agent_id)?;
            if is_closed_subagent(agent) {
                return agent.parent_agent_id;
            }
            Some(agent_id)
        });
        let active_agent = active_agent_id.and_then(|agent_id| {
            let agent = mount_state.agents.get(&agent_id)?;
            if is_closed_subagent(agent) {
                return None;
            }
            Some(AiAgentState {
                agent_id,
                title: agent.title.clone(),
                backend_id: agent.backend_id.clone(),
                status: agent.status.clone(),
                pending: agent.is_pending(),
                messages: agent.messages.clone(),
                parent_agent_id: agent.parent_agent_id,
                role: agent.role.clone(),
                subagents: agent.subagents.clone(),
                current_action: agent.current_action.clone(),
                last_terminal_excerpt: agent.last_terminal_excerpt.clone(),
                files_touched: agent.files_touched.clone(),
            })
        });
        AiMountState {
            backends,
            active_backend_id: Some(mount_state.active_backend_id.clone()),
            active_agent_id,
            agents,
            active_agent,
            live_markdown: self.ai_live_markdown(mount_state),
            active_workflow: mount_state.active_workflow.clone(),
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
                    pending_tool_message_start: None,
                    cancel_requested: false,
                    run_token: 0,
                    messages,
                    history: sanitize_conversation_history(chat.history),
                    updated_at: chat.updated_at,
                    parent_agent_id: chat.parent_agent_id,
                    role: chat.role,
                    task: chat.task,
                    subagents: chat.subagents.unwrap_or_default(),
                    current_action: None,
                    last_terminal_excerpt: None,
                    files_touched: Vec::new(),
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
                history: sanitize_conversation_history(agent.history.clone()),
                parent_agent_id: agent.parent_agent_id,
                role: agent.role.clone(),
                task: agent.task.clone(),
                subagents: Some(agent.subagents.clone()),
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
        remove_agent_file_for_root_best_effort(&self.event_tx, mount, &root_path, agent_id);
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
            agent.pending_tool_message_start = None;
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

    fn start_chatgpt_oauth_listener(&self, mount: &str, backend_id: &str) {
        let event_tx = self.event_tx.clone();
        let mount = mount.to_string();
        let backend_id = backend_id.to_string();
        thread::spawn(move || {
            let Ok(listener) = TcpListener::bind("localhost:1455") else {
                return;
            };
            let Ok((mut stream, _addr)) = listener.accept() else {
                return;
            };
            let mut request = [0_u8; 8192];
            let read_len = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read_len]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let code = query_param(path, "code").unwrap_or_default();
            let error = query_param(path, "error").unwrap_or_default();
            let body = if !code.is_empty() {
                let _ = event_tx.send(HubEvent::AiChatGptOAuthCode {
                    mount,
                    backend_id,
                    code,
                });
                "<!doctype html><meta charset=\"utf-8\"><title>Makepad Studio ChatGPT</title><style>body{font-family:-apple-system,BlinkMacSystemFont,sans-serif;background:#1f1f1f;color:#ddd;padding:48px;line-height:1.45}</style><h1>ChatGPT authorization received</h1><p>You can return to Makepad Studio. Studio is exchanging the token now.</p>".to_string()
            } else if !error.is_empty() {
                format!(
                    "<!doctype html><meta charset=\"utf-8\"><title>Makepad Studio ChatGPT</title><style>body{{font-family:-apple-system,BlinkMacSystemFont,sans-serif;background:#1f1f1f;color:#ddd;padding:48px;line-height:1.45}}code{{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;color:#ffb0b0}}</style><h1>ChatGPT authorization failed</h1><p><code>{}</code></p>",
                    html_escape(&error)
                )
            } else {
                "<!doctype html><meta charset=\"utf-8\"><title>Makepad Studio ChatGPT</title><style>body{font-family:-apple-system,BlinkMacSystemFont,sans-serif;background:#1f1f1f;color:#ddd;padding:48px;line-height:1.45}</style><h1>ChatGPT authorization callback received</h1><p>No authorization code was present in the callback URL.</p>".to_string()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.as_bytes().len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });
    }

    fn complete_chatgpt_oauth(
        &mut self,
        pending: &PendingChatGptOAuth,
        token_response: ChatGptTokenResponse,
    ) {
        let Some(backend_index) = self
            .backends
            .iter()
            .position(|backend| backend.id == pending.backend_id)
        else {
            return;
        };
        let mut credentials = token_response.into_credentials(None, now_unix_seconds());
        if credentials.account_id.is_none() {
            credentials.account_id =
                ChatGptProvider::extract_account_id_from_jwt(&credentials.access_token);
        }
        if credentials.access_token.trim().is_empty() {
            self.append_backend_system_message(
                &pending.mount,
                &pending.backend_id,
                "ChatGPT token exchange did not return an access token.",
            );
            return;
        }
        if let Some(provider) = self.backends[backend_index].chatgpt.as_mut() {
            provider.credentials = credentials.clone();
        }
        self.backends[backend_index].configured = true;
        self.backends[backend_index].detail = self.backends[backend_index].model.clone();
        self.backends[backend_index].configuration_hint =
            Some("ChatGPT is configured for this Studio session.".to_string());
        let _ = persist_chatgpt_credentials(&credentials);
        self.append_backend_system_message(
            &pending.mount,
            &pending.backend_id,
            "ChatGPT login complete. Credentials were saved for future Studio sessions.",
        );
    }

    fn append_backend_system_message(&mut self, mount: &str, backend_id: &str, text: &str) {
        self.ensure_mount_entry(mount);
        if let Some(mount_state) = self.mounts.get_mut(mount) {
            if let Some(agent_id) = mount_state.active_agent_id {
                if let Some(agent) = mount_state.agents.get_mut(&agent_id) {
                    agent.backend_id = backend_id.to_string();
                    agent.messages.push(AiMessage {
                        role: AiMessageRole::System,
                        text: text.to_string(),
                    });
                    agent.status = text.to_string();
                    agent.updated_at = now_seconds();
                }
            }
        }
        self.persist_mount_state_best_effort(mount);
    }

    pub fn parse_skill_markdown(content: &str) -> Option<ParsedSkill> {
        let lines: Vec<&str> = content.lines().collect();
        let mut first_dash = None;
        let mut second_dash = None;
        for (idx, line) in lines.iter().enumerate() {
            if line.trim() == "---" {
                if first_dash.is_none() {
                    first_dash = Some(idx);
                } else {
                    second_dash = Some(idx);
                    break;
                }
            }
        }

        let (frontmatter_lines, body_lines) = match (first_dash, second_dash) {
            (Some(start), Some(end)) if start < end => (&lines[start + 1..end], &lines[end + 1..]),
            _ => (&[][..], &lines[..]),
        };

        let mut name = String::new();
        let mut description = String::new();

        for line in frontmatter_lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, val)) = line.split_once(':') {
                let key = key.trim();
                let val = val.trim().trim_matches('"').trim_matches('\'').trim();
                if key == "name" {
                    name = val.to_string();
                } else if key == "description" {
                    description = val.to_string();
                }
            }
        }

        let body_content = body_lines.join("\n");

        if name.trim().is_empty() || body_content.trim().is_empty() {
            return None;
        }

        Some(ParsedSkill {
            name,
            description,
            content: body_content,
        })
    }

    pub fn parse_workflow_markdown(content: &str) -> Option<ParsedWorkflow> {
        let mut name = String::new();
        let mut steps = Vec::new();
        let mut in_steps = false;
        let mut current_step_name = None;
        let mut current_step_desc = Vec::new();

        let lines: Vec<&str> = content.lines().collect();
        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("# ") {
                if name.is_empty() {
                    name = trimmed
                        .strip_prefix("# ")
                        .unwrap_or(trimmed)
                        .trim()
                        .to_string();
                }
            } else if trimmed.starts_with("## ") {
                if trimmed
                    .to_lowercase()
                    .strip_prefix("##")
                    .unwrap_or("")
                    .trim()
                    == "steps"
                {
                    in_steps = true;
                } else {
                    in_steps = false;
                }
            } else if in_steps && trimmed.starts_with("### ") {
                if let Some(s_name) = current_step_name.take() {
                    let s_desc = current_step_desc.join("\n").trim().to_string();
                    steps.push(WorkflowStep {
                        name: s_name,
                        description: s_desc,
                    });
                    current_step_desc.clear();
                }

                let raw_step_name = trimmed.strip_prefix("###").unwrap_or(trimmed).trim();
                let mut chars = raw_step_name.chars().peekable();
                let mut has_digits = false;
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() {
                        has_digits = true;
                        chars.next();
                    } else {
                        break;
                    }
                }
                let parsed_name = if has_digits && chars.peek() == Some(&'.') {
                    chars.next(); // consume '.'
                    let rest: String = chars.collect();
                    rest.trim().to_string()
                } else {
                    raw_step_name.to_string()
                };

                current_step_name = Some(parsed_name);
            } else if in_steps && current_step_name.is_some() {
                current_step_desc.push(line);
            }
        }

        if let Some(s_name) = current_step_name {
            let s_desc = current_step_desc.join("\n").trim().to_string();
            steps.push(WorkflowStep {
                name: s_name,
                description: s_desc,
            });
        }

        if name.trim().is_empty() || steps.is_empty() {
            return None;
        }

        Some(ParsedWorkflow { name, steps })
    }

    pub fn load_skills_for_mount(&self, root_path: &str) -> Vec<ParsedSkill> {
        let mut skills = Vec::new();
        let path = Path::new(root_path).join(".studio").join("skills");
        if !path.is_dir() {
            return skills;
        }
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return skills,
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let file_path = entry.path();
            if file_path.is_file() && file_path.extension().map_or(false, |ext| ext == "md") {
                if let Ok(content) = fs::read_to_string(&file_path) {
                    if let Some(parsed) = Self::parse_skill_markdown(&content) {
                        skills.push(parsed);
                    }
                }
            }
        }
        skills
    }

    pub fn load_workflows_for_mount(&self, root_path: &str) -> Vec<ParsedWorkflow> {
        let mut workflows = Vec::new();
        let path = Path::new(root_path).join(".studio").join("workflows");
        if !path.is_dir() {
            return workflows;
        }
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return workflows,
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let file_path = entry.path();
            if file_path.is_file() && file_path.extension().map_or(false, |ext| ext == "md") {
                if let Ok(content) = fs::read_to_string(&file_path) {
                    if let Some(parsed) = Self::parse_workflow_markdown(&content) {
                        workflows.push(parsed);
                    }
                }
            }
        }
        workflows
    }
}

fn remove_agent_file_for_root_best_effort(
    event_tx: &Sender<HubEvent>,
    mount: &str,
    root_path: &str,
    agent_id: AiAgentId,
) {
    if root_path.is_empty() {
        return;
    }
    let _ = event_tx.send(HubEvent::SuppressMountRootFsEvents {
        mount: mount.to_string(),
        duration: AI_CHAT_PERSIST_FS_SUPPRESS,
    });
    let path = ai_chat_file_path(Path::new(root_path), agent_id);
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

fn prune_history(history: &[ConversationItem]) -> Vec<ConversationItem> {
    let history = sanitize_conversation_history(history.to_vec());
    let mut pruned = Vec::new();
    let mut total_chars = 0;
    for item in history.iter().rev() {
        let mut new_item = item.clone();
        match &mut new_item {
            ConversationItem::ToolResult { content, .. } => {
                total_chars += content.len();
                if total_chars > 16000 {
                    if content.len() > 300 {
                        *content =
                            format!("{}... [truncated for context efficiency]", &content[..300]);
                    }
                }
            }
            ConversationItem::User { text } => {
                total_chars += text.len();
            }
            ConversationItem::Assistant { text, .. } => {
                total_chars += text.len();
            }
        }
        pruned.push(new_item);
    }
    pruned.reverse();
    pruned
}

fn build_request_body(
    backend: &AiBackendConfig,
    mount: &str,
    root_path: &str,
    history: &[ConversationItem],
    role: Option<&str>,
    task: Option<&str>,
    active_terminals: &[String],
    skills: &[ParsedSkill],
    workflows: &[ParsedWorkflow],
    active_workflow: Option<&ActiveWorkflowState>,
) -> String {
    let system_prompt = render_system_prompt(
        mount,
        root_path,
        role,
        task,
        active_terminals,
        skills,
        workflows,
        active_workflow,
    );
    let pruned_history = prune_history(history);
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

    for item in &pruned_history {
        match item {
            ConversationItem::User { text } => {
                append_plain_message(&mut out, &mut first_message, "user", text);
            }
            ConversationItem::Assistant { text, tool_calls } => {
                if is_empty_assistant_turn(text, tool_calls) {
                    continue;
                }
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

fn detect_chatgpt_backend() -> AiBackendConfig {
    let default_oauth = ChatGptOAuthConfig::default();
    let client_id = std::env::var("MAKEPAD_STUDIO_CHATGPT_CLIENT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or(default_oauth.client_id);
    let persisted_credentials = read_persisted_chatgpt_credentials();
    let access_token = std::env::var("MAKEPAD_STUDIO_CHATGPT_ACCESS_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            persisted_credentials
                .as_ref()
                .map(|credentials| credentials.access_token.clone())
        })
        .unwrap_or_default();
    let configured = !access_token.is_empty();

    let refresh_token = std::env::var("MAKEPAD_STUDIO_CHATGPT_REFRESH_TOKEN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            persisted_credentials
                .as_ref()
                .and_then(|credentials| credentials.refresh_token.clone())
        });
    let account_id = std::env::var("MAKEPAD_STUDIO_CHATGPT_ACCOUNT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            persisted_credentials
                .as_ref()
                .and_then(|credentials| credentials.account_id.clone())
        })
        .or_else(|| ChatGptProvider::extract_account_id_from_jwt(&access_token));
    let expires_at_unix = persisted_credentials
        .as_ref()
        .and_then(|credentials| credentials.expires_at_unix);
    let model = std::env::var("MAKEPAD_STUDIO_CHATGPT_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_CHATGPT_MODEL.to_string());
    let model_kind = chatgpt_model_from_str(&model);
    let pkce = Some(ChatGptProvider::pkce_pair());
    let provider = ChatGptProvider::new(
        ChatGptOAuthConfig::new(client_id),
        ChatGptCredentials {
            access_token,
            refresh_token,
            account_id,
            expires_at_unix,
        },
        model_kind,
    );
    let configuration_url = pkce.as_ref().map(|pkce| {
        let mut url = provider.authorize_url("makepad-studio", pkce);
        url.push_str("&id_token_add_organizations=true");
        url.push_str("&codex_cli_simplified_flow=true");
        url.push_str("&originator=makepad-studio");
        url
    });
    let configuration_hint = Some(chatgpt_configuration_hint(
        configured,
        provider.oauth.client_id.trim().is_empty(),
        pkce.as_ref().map(|pkce| pkce.verifier.as_str()),
    ));

    AiBackendConfig {
        id: CHATGPT_BACKEND_ID.to_string(),
        label: "ChatGPT".to_string(),
        detail: if configured {
            model.clone()
        } else {
            format!("{}  not configured", model)
        },
        url: provider.responses_url.clone(),
        model,
        api_key: None,
        chatgpt: Some(provider),
        configured,
        configuration_url,
        configuration_hint,
        disable_thinking_via_chat_template: false,
    }
}

fn chatgpt_configuration_hint(
    configured: bool,
    missing_client_id: bool,
    pkce_verifier: Option<&str>,
) -> String {
    if configured {
        return "ChatGPT is configured for this Studio session.".to_string();
    }
    if missing_client_id {
        return "Set MAKEPAD_STUDIO_CHATGPT_CLIENT_ID in the Studio hub environment, then restart Studio.".to_string();
    }
    let verifier = pkce_verifier.unwrap_or("");
    format!(
        "Studio opened the ChatGPT login URL and is listening for the localhost callback. After authorization, Studio exchanges and saves the returned token automatically.\n\nPKCE verifier:\n{}",
        verifier
    )
}

fn backend_configuration_message(backend: &AiBackendConfig) -> String {
    let mut message = format!("Configure {}\n", backend.label);
    if let Some(hint) = &backend.configuration_hint {
        message.push_str(hint);
    } else if backend.configured {
        message.push_str("This backend is already configured.");
    } else {
        message.push_str("This backend needs configuration before it can run prompts.");
    }
    if let Some(url) = &backend.configuration_url {
        message.push_str("\n\nLogin URL:\n");
        message.push_str(url);
    }
    message
}

fn subagent_kickoff_prompt(role: &str, task: &str) -> String {
    format!(
        "You are the `{}` subagent.\n\nTask:\n{}\n\nWork only on this task. Use tools as needed. When finished, call `complete_task` with success and a concise summary so the parent agent can continue.",
        role.trim(),
        task.trim()
    )
}

fn chatgpt_model_from_str(model: &str) -> ChatGptModel {
    match model.trim() {
        "gpt-5.4-mini" => ChatGptModel::Gpt54Mini,
        "codex-mini-latest" => ChatGptModel::CodexMiniLatest,
        "o4-mini" => ChatGptModel::O4Mini,
        "o3" => ChatGptModel::O3,
        other => ChatGptModel::Custom(other.to_string()),
    }
}

fn build_chatgpt_request(
    backend: &AiBackendConfig,
    mount: &str,
    root_path: &str,
    history: &[ConversationItem],
    role: Option<&str>,
    task: Option<&str>,
    active_terminals: &[String],
    skills: &[ParsedSkill],
    workflows: &[ParsedWorkflow],
    active_workflow: Option<&ActiveWorkflowState>,
) -> Result<ChatGptRequest, String> {
    let system_prompt = render_system_prompt(
        mount,
        root_path,
        role,
        task,
        active_terminals,
        skills,
        workflows,
        active_workflow,
    );
    let mut messages = vec![ChatGptMessage::system(system_prompt)];
    for item in prune_history(history) {
        match item {
            ConversationItem::User { text } => {
                messages.push(ChatGptMessage::user(text));
            }
            ConversationItem::Assistant { text, tool_calls } => {
                if is_empty_assistant_turn(&text, &tool_calls) {
                    continue;
                }
                if tool_calls.is_empty() {
                    messages.push(ChatGptMessage::assistant(text));
                } else {
                    let mut content = Vec::new();
                    if !text.trim().is_empty() {
                        content.push(ChatGptContentBlock::Text { text });
                    }
                    for call in tool_calls {
                        content.push(ChatGptContentBlock::ToolCall {
                            id: call.id,
                            name: call.name,
                            arguments_json: call.arguments_json,
                        });
                    }
                    messages.push(ChatGptMessage {
                        role: ChatGptMessageRole::Assistant,
                        content,
                    });
                }
            }
            ConversationItem::ToolResult {
                tool_call_id,
                content,
            } => {
                messages.push(ChatGptMessage {
                    role: ChatGptMessageRole::Tool,
                    content: vec![ChatGptContentBlock::ToolResult {
                        tool_call_id,
                        content,
                        is_error: false,
                    }],
                });
            }
        }
    }

    let model_name = if backend.model.trim().is_empty() {
        DEFAULT_CHATGPT_MODEL
    } else {
        backend.model.as_str()
    };
    let model = chatgpt_model_from_str(model_name);
    Ok(ChatGptRequest {
        messages,
        model: model.clone(),
        max_output_tokens: model.max_output_tokens(),
        temperature: None,
        tools: chatgpt_tools(),
        stream: true,
    })
}

fn chatgpt_tools() -> Vec<ChatGptTool> {
    vec![
        ChatGptTool {
            name: "read_file".to_string(),
            description: "Read a UTF-8 text file from the workspace. Use this before editing."
                .to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to the file"},"offset":{"type":"integer","description":"Starting line number (1-indexed)"},"limit":{"type":"integer","description":"Maximum number of lines to read"}},"required":["path"]}"#.to_string(),
        },
        ChatGptTool {
            name: "list_files".to_string(),
            description: "List files and directories in the workspace.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Optional workspace-relative directory"},"limit":{"type":"integer","description":"Maximum number of entries to return"}}}"#.to_string(),
        },
        ChatGptTool {
            name: "search_text".to_string(),
            description:
                "Search text in workspace files and return matching path and line snippets."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"pattern":{"type":"string","description":"Text to search for"},"path":{"type":"string","description":"Optional workspace-relative directory"},"limit":{"type":"integer","description":"Maximum number of matches to return"}},"required":["pattern"]}"#.to_string(),
        },
        ChatGptTool {
            name: "write_file".to_string(),
            description:
                "Write a UTF-8 text file in the workspace, creating parent directories if needed."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to write"},"content":{"type":"string","description":"Full file contents"}},"required":["path","content"]}"#.to_string(),
        },
        ChatGptTool {
            name: "replace_in_file".to_string(),
            description: "Replace text in an existing UTF-8 file.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to edit"},"old_text":{"type":"string","description":"Existing text to replace"},"new_text":{"type":"string","description":"Replacement text"},"replace_all":{"type":"boolean","description":"Replace all matches instead of the first one"}},"required":["path","old_text","new_text"]}"#.to_string(),
        },
        ChatGptTool {
            name: "open_editor".to_string(),
            description:
                "Open a UTF-8 text file in a Studio code editor tab for this workspace, optionally jumping to a line and column."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path to open in Studio"},"line":{"type":"integer","description":"Optional 1-indexed line to focus after opening"},"column":{"type":"integer","description":"Optional 1-indexed column to focus after opening"}},"required":["path"]}"#.to_string(),
        },
        ChatGptTool {
            name: "observe_filesystem".to_string(),
            description:
                "Return recent filesystem changes observed by the Studio hub watcher for this workspace. Use this after other agents edit files."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Optional workspace-relative path prefix to filter changes"},"limit":{"type":"integer","description":"Maximum number of recent changes to return"},"since_secs":{"type":"integer","description":"Only include changes observed within this many seconds"}}}"#.to_string(),
        },
        ChatGptTool {
            name: "open_terminal".to_string(),
            description:
                "Open a Studio terminal for this workspace and optionally run an initial command such as codex."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"name":{"type":"string","description":"Optional terminal tab name stem"},"command":{"type":"string","description":"Optional command to send after the terminal opens"},"cols":{"type":"integer","description":"Optional terminal column count"},"rows":{"type":"integer","description":"Optional terminal row count"}}}"#.to_string(),
        },
        ChatGptTool {
            name: "list_terminals".to_string(),
            description:
                "List currently open Studio terminals for this workspace. Use the returned path value with other terminal tools."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{}}"#.to_string(),
        },
        ChatGptTool {
            name: "read_terminal".to_string(),
            description: "Read visible text and state from an open Studio terminal.".to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Exact terminal path returned by open_terminal or list_terminals"},"rows":{"type":"integer","description":"Optional number of visible rows to include"},"top_row":{"type":"integer","description":"Optional absolute top row to read; omit to read from the bottom"}},"required":["path"]}"#.to_string(),
        },
        ChatGptTool {
            name: "send_terminal_text".to_string(),
            description:
                "Send text to an open Studio terminal, optionally submitting it with Enter. Use submit=true when the text should run immediately, especially for codex prompts."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Exact terminal path returned by open_terminal or list_terminals"},"text":{"type":"string","description":"Text to send to the terminal"},"submit":{"type":"boolean","description":"When true, press Enter after the text. Use this for commands and codex prompts that should execute immediately"},"bracketed_paste":{"type":"boolean","description":"Override bracketed paste wrapping for multiline text"}},"required":["path","text"]}"#.to_string(),
        },
        ChatGptTool {
            name: "send_terminal_key".to_string(),
            description:
                "Send a keypress to an open Studio terminal. Use this for Enter, Ctrl+C, arrows, Tab, Escape, or function keys."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Exact terminal path returned by open_terminal or list_terminals"},"key":{"type":"string","description":"Key name such as enter, tab, up, f5, or a single printable character. Modifier prefixes like ctrl+c are also accepted"},"shift":{"type":"boolean","description":"Optional Shift modifier"},"control":{"type":"boolean","description":"Optional Control modifier"},"alt":{"type":"boolean","description":"Optional Alt modifier"}},"required":["path","key"]}"#.to_string(),
        },
        ChatGptTool {
            name: "bash".to_string(),
            description:
                "Run a shell command inside the workspace root. Prefer quick inspection and verification commands."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"command":{"type":"string","description":"Shell command to execute"},"timeout_secs":{"type":"integer","description":"Optional timeout in seconds"}},"required":["command"]}"#.to_string(),
        },
        ChatGptTool {
            name: "spawn_subagent".to_string(),
            description:
                "Spawn a specialized subagent to perform a scoped task (e.g. planner, coder, critic, explorer). The parent will wait until the subagent completes."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"role":{"type":"string","description":"The subagent role name, e.g. planner, coder, critic, explorer"},"task":{"type":"string","description":"Concretely describe the subtask/goal for the subagent"},"model_override":{"type":"string","description":"Optional model name override, e.g. gpt-4o-mini"}},"required":["role","task"]}"#.to_string(),
        },
        ChatGptTool {
            name: "complete_task".to_string(),
            description:
                "Called by a subagent to declare its task completed and return the structured findings/results to the parent agent."
                    .to_string(),
            parameters_json: r#"{"type":"object","properties":{"summary":{"type":"string","description":"A detailed summary of the findings or results of the task"},"success":{"type":"boolean","description":"Whether the task was completed successfully"}},"required":["summary","success"]}"#.to_string(),
        },
    ]
}

fn render_system_prompt(
    mount: &str,
    root_path: &str,
    role: Option<&str>,
    task: Option<&str>,
    active_terminals: &[String],
    skills: &[ParsedSkill],
    workflows: &[ParsedWorkflow],
    active_workflow: Option<&ActiveWorkflowState>,
) -> String {
    let mut base = SYSTEM_PROMPT_TEMPLATE
        .replace("{{mount}}", mount)
        .replace("{{root_path}}", root_path)
        .trim()
        .to_string();

    if !active_terminals.is_empty() {
        base.push_str("\n\n--- ACTIVE WORKSPACE TERMINALS ---\nYou currently have the following active terminals running in your environment:\n");
        for term in active_terminals {
            base.push_str(term);
            base.push('\n');
        }
        base.push_str(
            "You can interact with them via `read_terminal` or `send_terminal_text` tools.",
        );
    }

    if !skills.is_empty() {
        base.push_str("\n\n# Workspace Skills\nThe active workspace has these loaded skills. Follow them when relevant.\n");
        for skill in skills {
            base.push_str("\n## ");
            base.push_str(skill.name.trim());
            if !skill.description.trim().is_empty() {
                base.push_str("\nDescription: ");
                base.push_str(skill.description.trim());
            }
            base.push_str("\n\n");
            base.push_str(skill.content.trim());
            base.push('\n');
        }
    }

    if let Some(active_workflow) = active_workflow {
        append_workflow_focus(&mut base, active_workflow, workflows);
    }

    if let (Some(role), Some(task)) = (role, task) {
        base.push_str(&format!(
            "\n\n--- SUBAGENT Swarm Orchestration Mode ---\nYou are a specialized subagent executing the role: '{}'.\nYour task/goal is: '{}'.\nFocus strictly on your assigned task. When you are done, summarize your findings and call the `complete_task` tool to yield control back to the parent agent.",
            role, task
        ));
    }
    base
}

fn append_workflow_focus(
    out: &mut String,
    active_workflow: &ActiveWorkflowState,
    workflows: &[ParsedWorkflow],
) {
    out.push_str("\n\n# Current Workflow\n");
    out.push_str("Workflow: ");
    out.push_str(active_workflow.name.trim());
    out.push('\n');

    if let Some(step) = active_workflow.steps.get(active_workflow.current_step) {
        out.push_str("Current step: ");
        out.push_str(&(active_workflow.current_step + 1).to_string());
        out.push_str(". ");
        out.push_str(step.name.trim());
        out.push('\n');
        out.push_str("Status: ");
        out.push_str(step.status.trim());
        out.push('\n');

        if let Some(description) =
            workflow_step_description(workflows, &active_workflow.name, &step.name)
        {
            out.push_str("Description:\n");
            out.push_str(description.trim());
            out.push('\n');
        }
    }
}
fn workflow_step_description<'a>(
    workflows: &'a [ParsedWorkflow],
    workflow_name: &str,
    step_name: &str,
) -> Option<&'a str> {
    workflows
        .iter()
        .find(|workflow| workflow.name == workflow_name)
        .and_then(|workflow| {
            workflow
                .steps
                .iter()
                .find(|step| step.name == step_name)
                .map(|step| step.description.as_str())
        })
        .filter(|description| !description.trim().is_empty())
}

fn workflow_prompt_from_command(
    prompt: &str,
    workflows: &[ParsedWorkflow],
) -> Option<(ActiveWorkflowState, String)> {
    let command_line = prompt.strip_prefix('/')?;
    let (command, arguments) = command_line
        .split_once(char::is_whitespace)
        .map(|(command, arguments)| (command.trim(), arguments.trim()))
        .unwrap_or((command_line.trim(), ""));
    if command.is_empty() {
        return None;
    }
    let workflow = workflows
        .iter()
        .find(|workflow| workflow_command_matches(&workflow.name, command))?;
    let first_step = workflow.steps.first()?;
    let mut steps = Vec::with_capacity(workflow.steps.len());
    for (index, step) in workflow.steps.iter().enumerate() {
        steps.push(WorkflowStepState {
            name: step.name.clone(),
            status: if index == 0 { "active" } else { "pending" }.to_string(),
        });
    }

    let mut instruction = String::new();
    instruction.push_str("Execute workflow `");
    instruction.push_str(&workflow.name);
    instruction.push_str("`.");
    if !arguments.is_empty() {
        instruction.push_str("\nArguments: ");
        instruction.push_str(arguments);
    }
    instruction.push_str("\nFocus on step 1: ");
    instruction.push_str(&first_step.name);
    instruction.push_str("\nStatus: active");
    if !first_step.description.trim().is_empty() {
        instruction.push_str("\nStep description:\n");
        instruction.push_str(first_step.description.trim());
    }
    instruction.push_str("\n\nComplete this step before moving to later workflow steps.");

    Some((
        ActiveWorkflowState {
            name: workflow.name.clone(),
            current_step: 0,
            steps,
        },
        instruction,
    ))
}

fn workflow_command_matches(workflow_name: &str, command: &str) -> bool {
    workflow_name == command || workflow_command_slug(workflow_name) == command
}

fn workflow_command_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(ch.to_ascii_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    slug
}

fn sanitize_conversation_history(history: Vec<ConversationItem>) -> Vec<ConversationItem> {
    let completed_tool_call_ids = history
        .iter()
        .filter_map(|item| match item {
            ConversationItem::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut retained_tool_call_ids = HashSet::new();
    let mut sanitized = Vec::new();

    for item in history {
        match item {
            ConversationItem::Assistant { text, tool_calls } => {
                let tool_calls = tool_calls
                    .into_iter()
                    .filter(|tool_call| completed_tool_call_ids.contains(&tool_call.id))
                    .collect::<Vec<_>>();
                if is_empty_assistant_turn(&text, &tool_calls) {
                    continue;
                }
                for tool_call in &tool_calls {
                    retained_tool_call_ids.insert(tool_call.id.clone());
                }
                sanitized.push(ConversationItem::Assistant { text, tool_calls });
            }
            ConversationItem::ToolResult {
                tool_call_id,
                content,
            } => {
                if retained_tool_call_ids.contains(&tool_call_id) {
                    sanitized.push(ConversationItem::ToolResult {
                        tool_call_id,
                        content,
                    });
                }
            }
            item => sanitized.push(item),
        }
    }

    sanitized
}

fn push_assistant_history_dedup(
    history: &mut Vec<ConversationItem>,
    text: String,
    tool_calls: Vec<ToolCallRecord>,
) {
    if tool_calls.is_empty()
        && history.last().is_some_and(|item| {
            matches!(
                item,
                ConversationItem::Assistant {
                    text: previous,
                    tool_calls
                } if tool_calls.is_empty() && same_visible_text(previous, &text)
            )
        })
    {
        return;
    }
    history.push(ConversationItem::Assistant { text, tool_calls });
}

fn collapse_repeated_tail_messages(messages: &mut Vec<AiMessage>) {
    loop {
        let len = messages.len();
        if len < 2 {
            return;
        }
        let duplicate = {
            let previous = &messages[len - 2];
            let current = &messages[len - 1];
            matches!(previous.role, AiMessageRole::Assistant)
                && matches!(current.role, AiMessageRole::Assistant)
                && same_visible_text(&previous.text, &current.text)
        };
        if duplicate {
            messages.pop();
        } else {
            return;
        }
    }
}

fn same_visible_text(left: &str, right: &str) -> bool {
    left.split_whitespace().collect::<Vec<_>>() == right.split_whitespace().collect::<Vec<_>>()
}

fn is_closed_subagent(agent: &RunningAgent) -> bool {
    agent.parent_agent_id.is_some() && matches!(agent.status.as_str(), "completed" | "done")
}

fn is_empty_assistant_turn(text: &str, tool_calls: &[ToolCallRecord]) -> bool {
    text.trim().is_empty() && tool_calls.is_empty()
}

fn should_track_observed_terminal_mode(mode: &str) -> bool {
    matches!(mode, "working" | "awaiting-input" | "needs-attention")
}

fn should_show_live_task(task: &AiTrackedTask) -> bool {
    !matches!(task.status.as_str(), "done" | "cancelled")
        && task
            .terminal_path
            .as_deref()
            .map(|_| task.last_terminal_mode != "idle" && task.last_terminal_mode != "done")
            .unwrap_or(true)
}

fn should_show_live_terminal(terminal: &AiTerminalSnapshot) -> bool {
    matches!(
        terminal.mode,
        "working" | "awaiting-input" | "needs-attention" | "input"
    )
}

fn terminal_followup_signature(kind: &str, path: &str, task: &AiTrackedTask) -> String {
    format!(
        "terminal:{}:{}:{}:{}:{}:{}",
        path,
        kind,
        task.last_terminal_mode,
        task.last_terminal_summary,
        task.last_codex_status.as_deref().unwrap_or(""),
        stable_text_fingerprint(&task.last_terminal_excerpt)
    )
}

fn stable_text_fingerprint(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn live_task_title(task: &AiTrackedTask) -> String {
    if task.goal.starts_with("Observe Codex terminal `") {
        let terminal = task
            .terminal_path
            .as_deref()
            .map(terminal_display_name)
            .unwrap_or_else(|| "terminal".to_string());
        let action = match task.last_terminal_mode.as_str() {
            "awaiting-input" => "Reply to",
            "needs-attention" => "Resolve",
            "working" => "Monitor",
            _ => "Review",
        };
        return format!("{} `{}`", action, terminal);
    }
    truncate_inline(&task.goal, 96)
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
    append_tool_definition(
        out,
        &mut first,
        "spawn_subagent",
        "Spawn a specialized subagent to perform a scoped task (e.g. planner, coder, critic, explorer). The parent will wait until the subagent completes.",
        r#"{"type":"object","properties":{"role":{"type":"string","description":"The subagent role name, e.g. planner, coder, critic, explorer"},"task":{"type":"string","description":"Concretely describe the subtask/goal for the subagent"},"model_override":{"type":"string","description":"Optional model name override, e.g. gpt-4o-mini"}},"required":["role","task"]}"#,
    );
    append_tool_definition(
        out,
        &mut first,
        "complete_task",
        "Called by a subagent to declare its task completed and return the structured findings/results to the parent agent.",
        r#"{"type":"object","properties":{"summary":{"type":"string","description":"A detailed summary of the findings or results of the task"},"success":{"type":"boolean","description":"Whether the task was completed successfully"}},"required":["summary","success"]}"#,
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

fn extract_openai_assistant_turn(body: &str) -> Result<AssistantTurn, String> {
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
        raw_event_sample: String::new(),
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
            raw_event_sample: stream.raw_event_sample,
        },
        stream.visible,
    ))
}

fn append_raw_event_sample(sample: &mut String, event: &str) {
    const MAX_RAW_EVENT_SAMPLE: usize = 6000;
    if sample.len() >= MAX_RAW_EVENT_SAMPLE {
        return;
    }
    if !sample.is_empty() {
        sample.push_str("\n\n");
    }
    let remaining = MAX_RAW_EVENT_SAMPLE.saturating_sub(sample.len());
    if event.len() <= remaining {
        sample.push_str(event);
    } else {
        sample.push_str(&event[..remaining]);
    }
}

fn execute_tool_call(
    root_path: &Path,
    mount: &str,
    event_tx: &Sender<HubEvent>,
    tool_call: &ToolCallRecord,
) -> AiToolExecutionResult {
    let arguments_json = normalized_tool_arguments(&tool_call.arguments_json);
    let result = match tool_call.name.as_str() {
        "read_file" => ReadFileArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid read_file arguments: {:?}", err))
            .and_then(|args| tool_read_file(root_path, args)),
        "list_files" => ListFilesArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid list_files arguments: {:?}", err))
            .and_then(|args| tool_list_files(root_path, args)),
        "search_text" => SearchTextArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid search_text arguments: {:?}", err))
            .and_then(|args| tool_search_text(root_path, args)),
        "write_file" => WriteFileArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid write_file arguments: {:?}", err))
            .and_then(|args| tool_write_file(root_path, args)),
        "replace_in_file" => ReplaceInFileArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid replace_in_file arguments: {:?}", err))
            .and_then(|args| tool_replace_in_file(root_path, args)),
        "open_editor" => OpenEditorArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid open_editor arguments: {:?}", err))
            .and_then(|args| tool_open_editor(root_path, mount, event_tx, args)),
        "observe_filesystem" => ObserveFilesystemArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid observe_filesystem arguments: {:?}", err))
            .and_then(|args| tool_observe_filesystem(root_path, mount, event_tx, args)),
        "open_terminal" => OpenTerminalArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid open_terminal arguments: {:?}", err))
            .and_then(|args| tool_open_terminal(mount, event_tx, args)),
        "list_terminals" => tool_list_terminals(mount, event_tx),
        "read_terminal" => ReadTerminalArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid read_terminal arguments: {:?}", err))
            .and_then(|args| tool_read_terminal(mount, event_tx, args)),
        "send_terminal_text" => SendTerminalTextArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid send_terminal_text arguments: {:?}", err))
            .and_then(|args| tool_send_terminal_text(mount, event_tx, args)),
        "send_terminal_key" => SendTerminalKeyArgs::deserialize_json(arguments_json)
            .map_err(|err| format!("invalid send_terminal_key arguments: {:?}", err))
            .and_then(|args| tool_send_terminal_key(mount, event_tx, args)),
        "bash" => BashArgs::deserialize_json(arguments_json)
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

fn normalized_tool_arguments(arguments_json: &str) -> &str {
    if arguments_json.trim().is_empty() {
        "{}"
    } else {
        arguments_json
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
    reject_skipped_workspace_path(root_path, &path, &args.path)?;
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
                reject_skipped_workspace_path(&root, &path, trimmed)?;
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
    reject_skipped_workspace_path(root_path, &path, &args.path)?;
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
    if should_skip_workspace_path(root_path, &path) {
        return Ok("No files found.".to_string());
    }
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
    if should_skip_workspace_path(root_path, &search_root) {
        return Ok(format!("No matches found for '{}'.", pattern));
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
    reject_skipped_workspace_path(root_path, &path, &args.path)?;
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
    reject_skipped_workspace_path(root_path, &path, &args.path)?;
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
        if should_skip_workspace_path(root_path, &path) {
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
        if should_skip_workspace_path(root_path, &path) {
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

fn should_skip_workspace_path(root_path: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root_path).unwrap_or(path);
    should_skip_path(relative)
}

fn should_skip_path(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(should_skip_path_name)
            .unwrap_or(false)
    })
}

fn should_skip_path_name(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".makepad" | ".claude" | ".rustup" | "target" | "node_modules" | "build"
    ) || name.ends_with(".term")
}

fn reject_skipped_workspace_path(
    root_path: &Path,
    path: &Path,
    raw_path: &str,
) -> Result<(), String> {
    if should_skip_workspace_path(root_path, path) {
        Err(format!(
            "'{}' is Studio/internal state and is not available to AI file tools",
            raw_path
        ))
    } else {
        Ok(())
    }
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

fn format_terminal_waiting_message(result: &AiToolExecutionResult) -> Option<String> {
    if result.is_error || result.tool_name != "read_terminal" {
        return None;
    }
    let mode = parse_json_string_field(&result.content, "mode").unwrap_or_default();
    let path = parse_json_string_field(&result.content, "path").unwrap_or_default();
    let detail = parse_json_string_field(&result.content, "codex_status")
        .or_else(|| parse_json_string_field(&result.content, "summary"))
        .unwrap_or_default();
    let detail_lowered = detail.to_ascii_lowercase();
    if mode != "working"
        && !detail_lowered.contains("working")
        && !detail_lowered.contains("esc to interrupt")
    {
        return None;
    }

    let mut message = format!("{}waiting", AI_WAITING_MESSAGE_PREFIX);
    if !path.is_empty() {
        message.push_str(" on `");
        message.push_str(&path);
        message.push('`');
    }
    if !detail.trim().is_empty() {
        message.push_str(" - ");
        message.push_str(&truncate_waiting_detail(&detail));
    }
    Some(message)
}

fn last_terminal_waiting_message_from_history(history: &[ConversationItem]) -> Option<String> {
    let ConversationItem::ToolResult {
        tool_call_id,
        content,
    } = history.last()?
    else {
        return None;
    };

    let tool_call = history.iter().rev().find_map(|item| {
        let ConversationItem::Assistant { tool_calls, .. } = item else {
            return None;
        };
        tool_calls
            .iter()
            .find(|tool_call| tool_call.id == *tool_call_id && tool_call.name == "read_terminal")
    })?;

    format_terminal_waiting_message(&AiToolExecutionResult {
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content: content.clone(),
        is_error: false,
    })
}

fn format_terminal_observation_message(
    path: &str,
    mode: &str,
    summary: &str,
    codex_status: Option<&str>,
    excerpt: &str,
) -> String {
    let mut message = format!("{} {}\n", AI_TERMINAL_OBSERVATION_PREFIX, path);
    message.push_str(&format!("Mode: {}\n", mode));
    if let Some(codex_status) = codex_status.filter(|value| !value.trim().is_empty()) {
        message.push_str(&format!("Codex status: {}\n", codex_status.trim()));
    }
    if !summary.trim().is_empty() {
        message.push_str(&format!("Summary: {}\n", summary.trim()));
    }
    if !excerpt.trim().is_empty() {
        message.push_str("\nLatest output excerpt:\n```text\n");
        message.push_str(excerpt.trim());
        message.push_str("\n```");
    }
    message
}

fn upsert_terminal_observation_message(messages: &mut Vec<AiMessage>, text: &str) {
    let path = terminal_observation_path(text);
    if let Some(path) = path {
        if let Some(last) = messages.last_mut() {
            if matches!(last.role, AiMessageRole::System)
                && terminal_observation_path(&last.text) == Some(path)
            {
                last.text = text.to_string();
                return;
            }
        }
    }
    messages.push(AiMessage {
        role: AiMessageRole::System,
        text: text.to_string(),
    });
}

fn upsert_terminal_observation_history(history: &mut Vec<ConversationItem>, text: &str) {
    let path = terminal_observation_path(text);
    if let Some(path) = path {
        if let Some(ConversationItem::User { text: last }) = history.last_mut() {
            if terminal_observation_path(last) == Some(path) {
                *last = text.to_string();
                return;
            }
        }
    }
    history.push(ConversationItem::User {
        text: text.to_string(),
    });
}

fn terminal_observation_path(text: &str) -> Option<&str> {
    text.lines()
        .next()?
        .strip_prefix(AI_TERMINAL_OBSERVATION_PREFIX)?
        .trim()
        .split_whitespace()
        .next()
}

fn upsert_terminal_waiting_message(messages: &mut Vec<AiMessage>, waiting_message: String) {
    if let Some(last) = messages.last_mut() {
        if matches!(last.role, AiMessageRole::Thinking)
            && last.text.starts_with(AI_WAITING_MESSAGE_PREFIX)
        {
            last.text = waiting_message;
            return;
        }
    }
    messages.push(AiMessage {
        role: AiMessageRole::Thinking,
        text: waiting_message,
    });
}

fn trim_terminal_waiting_tail(messages: &mut Vec<AiMessage>) {
    if messages
        .last()
        .is_some_and(|message| message_tool_name(message) == Some("read_terminal"))
    {
        messages.pop();
    }
    while messages.last().is_some_and(|message| {
        matches!(
            message.role,
            AiMessageRole::Thinking | AiMessageRole::Assistant
        ) && looks_like_terminal_waiting_text(&message.text)
    }) {
        messages.pop();
    }
}

fn message_tool_name(message: &AiMessage) -> Option<&str> {
    if !matches!(message.role, AiMessageRole::ToolCall) {
        return None;
    }
    let rest = message.text.strip_prefix('`')?;
    let (tool_name, _) = rest.split_once('`')?;
    Some(tool_name)
}

fn looks_like_terminal_waiting_text(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("working on the task")
        || lowered.contains("still working")
        || (lowered.contains("codex") && lowered.contains("working"))
        || ((lowered.contains("wait") || lowered.contains("check again"))
            && lowered.contains("progress"))
}

fn truncate_waiting_detail(text: &str) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = single_line.chars().take(160).collect::<String>();
    if single_line.chars().count() > 160 {
        out.push_str("...");
    }
    out
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

fn chatgpt_credentials_path() -> PathBuf {
    PathBuf::from(".makepad").join("studio_chatgpt_credentials.json")
}

fn read_persisted_chatgpt_credentials() -> Option<PersistedChatGptCredentials> {
    let contents = fs::read_to_string(chatgpt_credentials_path()).ok()?;
    PersistedChatGptCredentials::deserialize_json(&contents).ok()
}

fn persist_chatgpt_credentials(credentials: &ChatGptCredentials) -> std::io::Result<()> {
    let path = chatgpt_credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let persisted = PersistedChatGptCredentials {
        access_token: credentials.access_token.clone(),
        refresh_token: credentials.refresh_token.clone(),
        account_id: credentials.account_id.clone(),
        expires_at_unix: credentials.expires_at_unix,
    };
    fs::write(path, persisted.serialize_json())
}

fn chatgpt_pkce_verifier_from_hint(hint: Option<&str>) -> Option<String> {
    hint?
        .lines()
        .last()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn query_param(path: &str, key: &str) -> Option<String> {
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = hex_value(bytes[index + 1]);
                let low = hex_value(bytes[index + 2]);
                if let (Some(high), Some(low)) = (high, low) {
                    out.push((high << 4) | low);
                    index += 3;
                } else {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
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
    use makepad_network::{HttpMethod, HttpRequest, HttpResponse, NetworkError, WsSend};
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
    fn test_parse_skill_markdown() {
        let content = r#"---
name: "Semantic Compression"
description: "Guidelines for compressing and summarizing files"
---
# Semantic Compression
This is the body content of the skill.
It has multiple lines.
"#;
        let parsed = AiManager::parse_skill_markdown(content).unwrap();
        assert_eq!(parsed.name, "Semantic Compression");
        assert_eq!(
            parsed.description,
            "Guidelines for compressing and summarizing files"
        );
        assert!(parsed.content.contains("# Semantic Compression"));
        assert!(parsed.content.contains("It has multiple lines."));
    }

    #[test]
    fn test_parse_workflow_markdown() {
        let content = r#"# Review PRs Command

Some intro text...

## Steps
### 1. Resolve PR Set
Description of step 1...
Detailed instructions...

### 2. Verify Changes
Description of step 2...

## Feedback
Not steps.
"#;
        let parsed = AiManager::parse_workflow_markdown(content).unwrap();
        assert_eq!(parsed.name, "Review PRs Command");
        assert_eq!(parsed.steps.len(), 2);
        assert_eq!(parsed.steps[0].name, "Resolve PR Set");
        assert_eq!(
            parsed.steps[0].description,
            "Description of step 1...\nDetailed instructions..."
        );
        assert_eq!(parsed.steps[1].name, "Verify Changes");
        assert_eq!(parsed.steps[1].description, "Description of step 2...");
    }

    #[test]
    fn test_parse_skill_markdown_validation() {
        // Missing name in frontmatter
        let content_no_name = r#"---
description: "Guidelines for compressing and summarizing files"
---
# Semantic Compression
This is the body content of the skill.
"#;
        assert!(AiManager::parse_skill_markdown(content_no_name).is_none());

        // Missing content body
        let content_no_body = r#"---
name: "Semantic Compression"
---
"#;
        assert!(AiManager::parse_skill_markdown(content_no_body).is_none());
    }

    #[test]
    fn test_parse_workflow_markdown_validation() {
        // Missing workflow name
        let content_no_name = r#"
## Steps
### 1. Resolve PR Set
Description of step 1...
"#;
        assert!(AiManager::parse_workflow_markdown(content_no_name).is_none());

        // Missing steps
        let content_no_steps = r#"# Review PRs Command

Some intro text...

## Steps
"#;
        assert!(AiManager::parse_workflow_markdown(content_no_steps).is_none());
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
    fn ai_file_tools_skip_studio_internal_paths() {
        assert!(should_skip_path(Path::new(".makepad")));
        assert!(should_skip_path(Path::new(".makepad/ai_chats/chat.json")));
        assert!(should_skip_path(Path::new("examples/.makepad/chat.term")));
        assert!(should_skip_path(Path::new("chat.term")));
        assert!(should_skip_path(Path::new(".claude")));
        assert!(should_skip_path(Path::new(".rustup")));
        assert!(should_skip_path(Path::new("target")));
        assert!(should_skip_path(Path::new("node_modules")));
        assert!(should_skip_path(Path::new("build")));
        assert!(!should_skip_path(Path::new("examples")));
        assert!(!should_skip_path(Path::new("src")));

        let root = Path::new("/repo");
        assert!(should_skip_workspace_path(
            root,
            Path::new("/repo/.makepad/ai_chats/chat.json")
        ));
        assert!(!should_skip_workspace_path(
            root,
            Path::new("/repo/examples/aichat/src/main.rs")
        ));
    }

    #[test]
    fn list_files_omits_ai_chat_storage() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "makepad_ai_list_files_test_{}_{}",
            std::process::id(),
            suffix
        ));
        fs::create_dir_all(root.join(".makepad/ai_chats")).unwrap();
        fs::create_dir_all(root.join("examples")).unwrap();
        fs::write(root.join(".makepad/ai_chats/chat.json"), "{}").unwrap();
        fs::write(root.join("examples/main.rs"), "fn main() {}\n").unwrap();

        let result = tool_list_files(
            &root,
            ListFilesArgs {
                path: None,
                limit: Some(50),
            },
        )
        .unwrap();
        assert!(result.contains("examples/"));
        assert!(result.contains("examples/main.rs"));
        assert!(!result.contains(".makepad"));
        assert!(!result.contains("chat.json"));

        let hidden_result = tool_list_files(
            &root,
            ListFilesArgs {
                path: Some(".makepad".to_string()),
                limit: Some(50),
            },
        )
        .unwrap();
        assert_eq!(hidden_result, "No files found.");

        let hidden_read = tool_read_file(
            &root,
            ReadFileArgs {
                path: ".makepad/ai_chats/chat.json".to_string(),
                offset: None,
                limit: None,
            },
        )
        .unwrap_err();
        assert!(hidden_read.contains("Studio/internal state"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn list_files_accepts_empty_tool_arguments() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "makepad_ai_empty_list_files_args_test_{}_{}",
            std::process::id(),
            suffix
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let (event_tx, _event_rx) = mpsc::channel();
        let result = execute_tool_call(
            &root,
            "makepad",
            &event_tx,
            &ToolCallRecord {
                id: "call_1".to_string(),
                name: "list_files".to_string(),
                arguments_json: String::new(),
            },
        );
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("Cargo.toml"));
        fs::remove_dir_all(&root).unwrap();
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
    fn terminal_mode_detects_codex_prompt_draft() {
        let (mode, is_codex, _summary, codex_status) =
            AiManager::terminal_mode_and_summary("", "\n\u{203a} make a hello world example\n");
        assert_eq!(mode, "awaiting-input");
        assert!(is_codex);
        assert_eq!(codex_status, None);
    }

    #[test]
    fn terminal_mode_detects_compact_codex_working_status() {
        let text = "\n[working] gpt-5.5 xhigh fast \u{00b7} ~/makepad/makepad\n";
        let (mode, is_codex, _summary, codex_status) =
            AiManager::terminal_mode_and_summary("", text);
        assert_eq!(mode, "working");
        assert!(is_codex);
        assert_eq!(
            codex_status.as_deref(),
            Some("[working] gpt-5.5 xhigh fast \u{00b7} ~/makepad/makepad")
        );
    }

    #[test]
    fn terminal_mode_prefers_codex_working_status_over_prompt_line() {
        let text = "• Working (20s • esc to interrupt)\n\n› Improve documentation in @filename\n\n  gpt-5.5 xhigh fast \u{00b7} ~/makepad/makepad";
        let (mode, is_codex, _summary, codex_status) =
            AiManager::terminal_mode_and_summary("", text);
        assert_eq!(mode, "working");
        assert!(is_codex);
        assert_eq!(
            codex_status.as_deref(),
            Some("• Working (20s • esc to interrupt)")
        );
    }

    #[test]
    fn terminal_input_marks_existing_codex_snapshot_active() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.process_terminal_observation(
            "repo",
            AiTerminalObservation {
                path: "repo/.makepad/hello-world-makepad.term".to_string(),
                terminal_title: "codex".to_string(),
                cols: 80,
                rows: 8,
                top_row: 42,
                total_lines: 50,
                is_tui: true,
                text: "\u{203a}\n".to_string(),
            },
        );

        let state = manager
            .process_terminal_input("repo", "repo/.makepad/hello-world-makepad.term")
            .expect("terminal input should change state");

        assert!(state.live_markdown.contains("[input / codex]"));
        assert!(state.live_markdown.contains("Input sent to Codex"));
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
        assert!(state.live_markdown.contains("**Todo**"));
        assert!(state.live_markdown.contains("**Terminals**"));
    }

    #[test]
    fn terminal_observation_autotracks_direct_codex_activity() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        let state = manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: "repo/.makepad/manual-codex.term".to_string(),
                    terminal_title: String::new(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: "• Working (2s • esc to interrupt)\n\n  gpt-5.5 xhigh fast · ~/makepad/makepad".to_string(),
                },
            )
            .expect("codex terminal observation should change state");

        let mount_state = manager.mounts.get("repo").unwrap();
        assert_eq!(mount_state.tasks.len(), 1);
        assert_eq!(
            mount_state.tasks[0].terminal_path.as_deref(),
            Some("repo/.makepad/manual-codex.term")
        );
        assert!(mount_state.tasks[0]
            .goal
            .contains("Observe Codex terminal `manual-codex.term`"));
        assert!(state.live_markdown.contains("Monitor `manual-codex.term`"));
        assert!(state.live_markdown.contains("[working / codex]"));
        let agent = mount_state
            .agents
            .get(&mount_state.active_agent_id.unwrap())
            .unwrap();
        assert!(agent.messages.iter().any(|message| {
            matches!(message.role, AiMessageRole::System)
                && message.text.starts_with(AI_TERMINAL_OBSERVATION_PREFIX)
                && message.text.contains("repo/.makepad/manual-codex.term")
                && message.text.contains("Working (2s")
        }));
        assert!(agent.history.iter().any(|item| {
            matches!(
                item,
                ConversationItem::User { text }
                    if text.starts_with(AI_TERMINAL_OBSERVATION_PREFIX)
                        && text.contains("repo/.makepad/manual-codex.term")
                        && text.contains("Working (2s")
            )
        }));

        manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: "repo/.makepad/manual-codex.term".to_string(),
                    terminal_title: String::new(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: "• Working (3s • esc to interrupt)\n\n  gpt-5.5 xhigh fast · ~/makepad/makepad".to_string(),
                },
            )
            .expect("updated codex terminal observation should change state");
        let mount_state = manager.mounts.get("repo").unwrap();
        let agent = mount_state
            .agents
            .get(&mount_state.active_agent_id.unwrap())
            .unwrap();
        let visible_observations = agent
            .messages
            .iter()
            .filter(|message| {
                matches!(message.role, AiMessageRole::System)
                    && message.text.starts_with(AI_TERMINAL_OBSERVATION_PREFIX)
            })
            .collect::<Vec<_>>();
        assert_eq!(visible_observations.len(), 1);
        assert!(visible_observations[0].text.contains("Working (3s"));
        let history_observations = agent
            .history
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ConversationItem::User { text }
                        if text.starts_with(AI_TERMINAL_OBSERVATION_PREFIX)
                )
            })
            .count();
        assert_eq!(history_observations, 1);
    }

    #[test]
    fn idle_codex_terminal_does_not_create_live_todo_or_terminal_line() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        let state = manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: "repo/.makepad/idle-codex.term".to_string(),
                    terminal_title: "codex".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: "wheregmis@MacBookPro makepad %\n".to_string(),
                },
            )
            .expect("idle terminal observation should update snapshots");

        let mount_state = manager.mounts.get("repo").unwrap();
        assert!(mount_state.tasks.is_empty());
        assert!(mount_state
            .terminal_snapshots
            .contains_key("repo/.makepad/idle-codex.term"));
        assert!(state.live_markdown.contains("_No open AI todos._"));
        assert!(state
            .live_markdown
            .contains("_No active terminal activity._"));
        assert!(!state.live_markdown.contains("idle-codex.term"));
    }

    #[test]
    fn awaiting_input_terminal_queues_orchestrator_reply() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: "repo/.makepad/codex-plan.term".to_string(),
                    terminal_title: "codex".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: "Working (2s) esc to interrupt\n".to_string(),
                },
            )
            .expect("initial working observation should change state");

        let state = manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: "repo/.makepad/codex-plan.term".to_string(),
                    terminal_title: "codex".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: "\n› Need more details?\n\n  gpt-5.5 medium · ~/repo\n".to_string(),
                },
            )
            .expect("awaiting input observation should dispatch a follow-up");

        let agent = state.active_agent.expect("active orchestrator agent");
        assert!(agent.pending);
        assert!(agent.messages.iter().any(|message| {
            matches!(message.role, AiMessageRole::User)
                && message.text.starts_with(AI_TASK_EVENT_PREFIX)
                && message.text.contains("Tracked terminal is awaiting input")
                && message.text.contains("Terminal mode: awaiting-input")
                && message.text.contains("send_terminal_text")
                && message.text.contains("submit=true")
        }));
    }

    #[test]
    fn unchanged_awaiting_input_prompt_does_not_loop_after_input() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        let path = "repo/.makepad/codex-plan.term";
        let awaiting_text = "\n› Need more details?\n\n  gpt-5.5 medium · ~/repo\n";

        manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: path.to_string(),
                    terminal_title: "codex".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: "Working (2s) esc to interrupt\n".to_string(),
                },
            )
            .expect("initial working observation should change state");

        manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: path.to_string(),
                    terminal_title: "codex".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: awaiting_text.to_string(),
                },
            )
            .expect("first awaiting input observation should dispatch");

        {
            let mount_state = manager.mounts.get_mut("repo").unwrap();
            let task = mount_state.tasks.first().unwrap();
            assert_eq!(task.handled_followup_signatures.len(), 1);
            let agent = mount_state
                .agents
                .get_mut(&mount_state.active_agent_id.unwrap())
                .unwrap();
            agent.pending_request_id = None;
            agent.pending_tool_batch = false;
            agent.status = "ready".to_string();
        }

        manager
            .process_terminal_input("repo", path)
            .expect("terminal input should update task state");
        let state = manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: path.to_string(),
                    terminal_title: "codex".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: true,
                    text: awaiting_text.to_string(),
                },
            )
            .expect("same awaiting prompt should update state without dispatching");

        let mount_state = manager.mounts.get("repo").unwrap();
        assert_eq!(mount_state.tasks[0].handled_followup_signatures.len(), 1);
        assert!(mount_state.queued_followups.is_empty());
        assert!(state
            .active_agent
            .as_ref()
            .is_some_and(|agent| !agent.pending));
        let followup_prompts = state
            .active_agent
            .as_ref()
            .unwrap()
            .messages
            .iter()
            .filter(|message| {
                matches!(message.role, AiMessageRole::User)
                    && message.text.starts_with(AI_TASK_EVENT_PREFIX)
                    && message.text.contains("Tracked terminal is awaiting input")
            })
            .count();
        assert_eq!(followup_prompts, 1);
    }

    #[test]
    fn terminal_observation_records_regular_terminal_without_task() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        let state = manager
            .process_terminal_observation(
                "repo",
                AiTerminalObservation {
                    path: "repo/.makepad/shell.term".to_string(),
                    terminal_title: "zsh".to_string(),
                    cols: 80,
                    rows: 8,
                    top_row: 0,
                    total_lines: 8,
                    is_tui: false,
                    text: "cargo check\nfinished".to_string(),
                },
            )
            .expect("terminal observation should change state");

        let mount_state = manager.mounts.get("repo").unwrap();
        assert!(mount_state.tasks.is_empty());
        assert!(mount_state
            .terminal_snapshots
            .contains_key("repo/.makepad/shell.term"));
        assert!(!state.live_markdown.contains("repo/.makepad/shell.term"));
        assert!(state
            .live_markdown
            .contains("_No active terminal activity._"));
    }

    #[test]
    fn working_terminal_reads_compact_to_single_waiting_message() {
        let tool_call = ToolCallRecord {
            id: "call_1".to_string(),
            name: "read_terminal".to_string(),
            arguments_json: r#"{"path":"makepad/.makepad/hello-world-makepad.term"}"#.to_string(),
        };
        let result = AiToolExecutionResult {
            tool_call_id: "call_1".to_string(),
            tool_name: "read_terminal".to_string(),
            content: r#"{"path":"makepad/.makepad/hello-world-makepad.term","mode":"working","summary":"gpt-5.5 xhigh fast","codex_status":"Working (12s) esc to interrupt"}"#.to_string(),
            is_error: false,
        };
        let waiting_message = format_terminal_waiting_message(&result).unwrap();

        let mut messages = vec![AiMessage {
            role: AiMessageRole::User,
            text: "watch the task".to_string(),
        }];
        let start = messages.len();
        messages.push(AiMessage {
            role: AiMessageRole::Thinking,
            text: "The codex instance is working on the task. Let me wait a bit more and check again for progress.".to_string(),
        });
        messages.push(AiMessage {
            role: AiMessageRole::ToolCall,
            text: format_tool_call_message(&tool_call),
        });
        messages.truncate(start);
        upsert_terminal_waiting_message(&mut messages, waiting_message);

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[1].role, AiMessageRole::Thinking));
        assert!(messages[1].text.starts_with(AI_WAITING_MESSAGE_PREFIX));
        assert!(messages[1]
            .text
            .contains("makepad/.makepad/hello-world-makepad.term"));

        let start = messages.len();
        messages.push(AiMessage {
            role: AiMessageRole::Thinking,
            text: "The codex instance is still working. I will check again.".to_string(),
        });
        messages.push(AiMessage {
            role: AiMessageRole::ToolCall,
            text: format_tool_call_message(&tool_call),
        });
        messages.truncate(start);
        upsert_terminal_waiting_message(
            &mut messages,
            "WAITING:waiting on `makepad/.makepad/hello-world-makepad.term` - Working (30s)"
                .to_string(),
        );

        assert_eq!(messages.len(), 2);
        assert!(messages[1].text.contains("Working (30s)"));
    }

    #[test]
    fn waiting_message_accepts_legacy_awaiting_input_with_working_status() {
        let result = AiToolExecutionResult {
            tool_call_id: "call_1".to_string(),
            tool_name: "read_terminal".to_string(),
            content: r#"{"path":"makepad/.makepad/hello-world-buttons.term","mode":"awaiting-input","summary":"gpt-5.5 xhigh fast","codex_status":"• Working (20s • esc to interrupt)"}"#.to_string(),
            is_error: false,
        };
        let message = format_terminal_waiting_message(&result).unwrap();
        assert!(message.starts_with(AI_WAITING_MESSAGE_PREFIX));
        assert!(message.contains("Working (20s"));
    }

    #[test]
    fn last_terminal_waiting_message_recovers_from_history() {
        let history = vec![
            ConversationItem::Assistant {
                text: String::new(),
                tool_calls: vec![ToolCallRecord {
                    id: "call_1".to_string(),
                    name: "read_terminal".to_string(),
                    arguments_json: r#"{"path":"makepad/.makepad/hello-world-buttons.term"}"#
                        .to_string(),
                }],
            },
            ConversationItem::ToolResult {
                tool_call_id: "call_1".to_string(),
                content: r#"{"path":"makepad/.makepad/hello-world-buttons.term","mode":"awaiting-input","codex_status":"• Working (20s • esc to interrupt)"}"#.to_string(),
            },
        ];
        let message = last_terminal_waiting_message_from_history(&history).unwrap();
        assert!(message.contains("hello-world-buttons.term"));
        assert!(message.contains("Working (20s"));
    }

    #[test]
    fn assistant_turn_extracts_tool_calls() {
        let turn = extract_openai_assistant_turn(
            r#"{"choices":[{"message":{"content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}]}}],"error":null}"#,
        )
        .unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "read_file");
    }

    #[test]
    fn assistant_turn_accepts_standard_openai_choice_fields() {
        let turn = extract_openai_assistant_turn(
            r#"{"id":"chatcmpl-1","object":"chat.completion","created":123,"model":"local","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"hello"}}]}"#,
        )
        .unwrap();
        assert_eq!(turn.text, "hello");
        assert_eq!(turn.thinking_text, "");
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn assistant_turn_extracts_reasoning_content() {
        let turn = extract_openai_assistant_turn(
            r#"{"choices":[{"message":{"content":"hello","reasoning_content":"step 1\nstep 2"}}]}"#,
        )
        .unwrap();
        assert_eq!(turn.text, "hello");
        assert_eq!(turn.thinking_text, "step 1\nstep 2");
    }

    #[test]
    fn empty_assistant_turn_does_not_enter_history() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.ensure_mount_entry("repo");
        manager.ensure_default_agent("repo");
        let agent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();
        let run_token = 42;
        {
            let agent = manager
                .mounts
                .get_mut("repo")
                .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
                .unwrap();
            agent.run_token = run_token;
        }

        manager.complete_assistant_turn(
            "repo",
            agent_id,
            run_token,
            AssistantTurn {
                text: String::new(),
                thinking_text: String::new(),
                tool_calls: Vec::new(),
                raw_event_sample: String::new(),
            },
            None,
        );

        let agent = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.agents.get(&agent_id))
            .unwrap();
        assert!(agent.history.is_empty());
        assert!(agent.messages.iter().any(|message| {
            matches!(message.role, AiMessageRole::Error)
                && message
                    .text
                    .contains("AI backend returned an empty assistant response")
        }));
    }

    #[test]
    fn repeated_assistant_tail_messages_are_collapsed() {
        let repeated = "Done — the implementation milestone is complete.";
        let mut messages = vec![
            AiMessage {
                role: AiMessageRole::Assistant,
                text: repeated.to_string(),
            },
            AiMessage {
                role: AiMessageRole::Assistant,
                text: format!("{}\n", repeated),
            },
        ];

        collapse_repeated_tail_messages(&mut messages);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, repeated);
    }

    #[test]
    fn spawned_subagent_starts_with_user_task_history() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.ensure_mount_entry("repo");
        manager.ensure_default_agent("repo");
        let agent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();
        let run_token = 42;
        {
            let agent = manager
                .mounts
                .get_mut("repo")
                .and_then(|mount_state| mount_state.agents.get_mut(&agent_id))
                .unwrap();
            agent.run_token = run_token;
        }

        manager.complete_assistant_turn(
            "repo",
            agent_id,
            run_token,
            AssistantTurn {
                text: "I will ask a planner to update this.".to_string(),
                thinking_text: String::new(),
                tool_calls: vec![ToolCallRecord {
                    id: "call_spawn".to_string(),
                    name: "spawn_subagent".to_string(),
                    arguments_json: r#"{"role":"planner","task":"Update the Makepad Studio refactor plan with the latest UI/task tracking changes."}"#.to_string(),
                }],
                raw_event_sample: String::new(),
            },
            None,
        );

        let mount_state = manager.mounts.get("repo").unwrap();
        let parent = mount_state.agents.get(&agent_id).unwrap();
        let sub_id = parent.subagents[0];
        let sub_agent = mount_state.agents.get(&sub_id).unwrap();
        let ConversationItem::User { text } = &sub_agent.history[0] else {
            panic!("subagent should start with a user kickoff message");
        };
        assert!(text.contains("You are the `planner` subagent."));
        assert!(text.contains("Update the Makepad Studio refactor plan"));
        assert!(text.contains("complete_task"));
        assert!(sub_agent.messages.iter().any(|message| {
            matches!(message.role, AiMessageRole::User)
                && message.text.contains("You are the `planner` subagent.")
        }));
    }

    #[test]
    fn completed_subagent_closes_after_returning_result_to_parent() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.ensure_mount_entry("repo");
        manager.ensure_default_agent("repo");
        let parent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();
        let parent_run_token = 42;
        {
            let parent = manager
                .mounts
                .get_mut("repo")
                .and_then(|mount_state| mount_state.agents.get_mut(&parent_id))
                .unwrap();
            parent.run_token = parent_run_token;
        }

        manager.complete_assistant_turn(
            "repo",
            parent_id,
            parent_run_token,
            AssistantTurn {
                text: "I will ask a coder to update the plan.".to_string(),
                thinking_text: String::new(),
                tool_calls: vec![ToolCallRecord {
                    id: "call_spawn".to_string(),
                    name: "spawn_subagent".to_string(),
                    arguments_json: r#"{"role":"coder","task":"Update the plan."}"#.to_string(),
                }],
                raw_event_sample: String::new(),
            },
            None,
        );

        let (sub_id, sub_run_token) = {
            let mount_state = manager.mounts.get("repo").unwrap();
            let parent = mount_state.agents.get(&parent_id).unwrap();
            let sub_id = parent.subagents[0];
            let sub_run_token = mount_state.agents.get(&sub_id).unwrap().run_token;
            (sub_id, sub_run_token)
        };

        manager.complete_assistant_turn(
            "repo",
            sub_id,
            sub_run_token,
            AssistantTurn {
                text: "Done.".to_string(),
                thinking_text: String::new(),
                tool_calls: vec![ToolCallRecord {
                    id: "call_complete".to_string(),
                    name: "complete_task".to_string(),
                    arguments_json: r#"{"summary":"Updated the plan.","success":true}"#.to_string(),
                }],
                raw_event_sample: String::new(),
            },
            None,
        );

        let mount_state = manager.mounts.get("repo").unwrap();
        let parent = mount_state.agents.get(&parent_id).unwrap();
        assert!(parent.subagents.is_empty());
        assert!(!mount_state.order.contains(&sub_id));
        assert!(!mount_state.agents.contains_key(&sub_id));
        assert_eq!(mount_state.active_agent_id, Some(parent_id));
        assert!(parent.history.iter().any(|item| {
            matches!(
                item,
                ConversationItem::ToolResult {
                    tool_call_id,
                    content
                } if tool_call_id == "call_spawn" && content.contains("Updated the plan.")
            )
        }));
    }

    #[test]
    fn snapshot_hides_legacy_completed_subagents() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.ensure_mount_entry("repo");
        manager.ensure_default_agent("repo");
        let parent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();
        let sub_id = manager.alloc_agent_id();
        {
            let mount_state = manager.mounts.get_mut("repo").unwrap();
            mount_state.order.push(sub_id);
            mount_state.active_agent_id = Some(sub_id);
            mount_state
                .agents
                .get_mut(&parent_id)
                .unwrap()
                .subagents
                .push(sub_id);
            mount_state.agents.insert(
                sub_id,
                RunningAgent {
                    title: "coder Subagent".to_string(),
                    backend_id: CHATGPT_BACKEND_ID.to_string(),
                    status: "completed".to_string(),
                    pending_request_id: None,
                    pending_tool_batch: false,
                    pending_tool_message_start: None,
                    cancel_requested: false,
                    run_token: 0,
                    messages: Vec::new(),
                    history: Vec::new(),
                    updated_at: now_seconds(),
                    parent_agent_id: Some(parent_id),
                    role: Some("coder".to_string()),
                    task: Some("Update plan".to_string()),
                    subagents: Vec::new(),
                    current_action: None,
                    last_terminal_excerpt: None,
                    files_touched: Vec::new(),
                },
            );
        }

        let state = manager.snapshot("repo");

        assert_eq!(state.active_agent_id, Some(parent_id));
        assert!(state.agents.iter().all(|agent| agent.agent_id != sub_id));
        assert!(state
            .active_agent
            .as_ref()
            .is_some_and(|agent| agent.agent_id == parent_id));
    }

    #[test]
    fn request_body_skips_persisted_empty_assistant_turns() {
        let backend = AiBackendConfig {
            id: LOCAL_BACKEND_ID.to_string(),
            label: String::new(),
            detail: String::new(),
            url: DEFAULT_LOCAL_BASE_URL.to_string(),
            model: String::new(),
            api_key: None,
            chatgpt: None,
            configured: true,
            configuration_url: None,
            configuration_hint: None,
            disable_thinking_via_chat_template: false,
        };
        let history = vec![
            ConversationItem::User {
                text: "hello".to_string(),
            },
            ConversationItem::Assistant {
                text: String::new(),
                tool_calls: Vec::new(),
            },
            ConversationItem::User {
                text: "again".to_string(),
            },
        ];

        let body = build_request_body(
            &backend,
            "repo",
            "/tmp/repo",
            &history,
            None,
            None,
            &[],
            &[],
            &[],
            None,
        );

        assert!(!body.contains("\"role\":\"assistant\",\"content\":\"\""));
        assert!(body.contains("\"content\":\"hello\""));
        assert!(body.contains("\"content\":\"again\""));
    }

    #[test]
    fn request_body_drops_unresolved_tool_calls() {
        let backend = AiBackendConfig {
            id: LOCAL_BACKEND_ID.to_string(),
            label: String::new(),
            detail: String::new(),
            url: DEFAULT_LOCAL_BASE_URL.to_string(),
            model: String::new(),
            api_key: None,
            chatgpt: None,
            configured: true,
            configuration_url: None,
            configuration_hint: None,
            disable_thinking_via_chat_template: false,
        };
        let history = vec![
            ConversationItem::User {
                text: "spawn a planner".to_string(),
            },
            ConversationItem::Assistant {
                text: "I will ask a planner.".to_string(),
                tool_calls: vec![ToolCallRecord {
                    id: "call_missing".to_string(),
                    name: "spawn_subagent".to_string(),
                    arguments_json: r#"{"role":"planner","task":"make a plan"}"#.to_string(),
                }],
            },
            ConversationItem::User {
                text: "try again".to_string(),
            },
        ];

        let body = build_request_body(
            &backend,
            "repo",
            "/tmp/repo",
            &history,
            None,
            None,
            &[],
            &[],
            &[],
            None,
        );

        assert!(body.contains("\"content\":\"I will ask a planner.\""));
        assert!(!body.contains("call_missing"));
        assert!(!body.contains("\"tool_calls\""));
        assert!(body.contains("\"content\":\"try again\""));
    }

    #[test]
    fn chatgpt_request_drops_unresolved_tool_calls() {
        let backend = AiBackendConfig {
            id: CHATGPT_BACKEND_ID.to_string(),
            label: String::new(),
            detail: String::new(),
            url: String::new(),
            model: DEFAULT_CHATGPT_MODEL.to_string(),
            api_key: None,
            chatgpt: None,
            configured: true,
            configuration_url: None,
            configuration_hint: None,
            disable_thinking_via_chat_template: false,
        };
        let history = vec![
            ConversationItem::User {
                text: "spawn a planner".to_string(),
            },
            ConversationItem::Assistant {
                text: "I will ask a planner.".to_string(),
                tool_calls: vec![ToolCallRecord {
                    id: "call_missing".to_string(),
                    name: "spawn_subagent".to_string(),
                    arguments_json: r#"{"role":"planner","task":"make a plan"}"#.to_string(),
                }],
            },
            ConversationItem::User {
                text: "try again".to_string(),
            },
        ];

        let request = build_chatgpt_request(
            &backend,
            "repo",
            "/tmp/repo",
            &history,
            None,
            None,
            &[],
            &[],
            &[],
            None,
        )
        .unwrap();

        assert!(request.messages.iter().any(|message| {
            matches!(message.role, ChatGptMessageRole::Assistant)
                && message.text() == "I will ask a planner."
                && message
                    .content
                    .iter()
                    .all(|block| matches!(block, ChatGptContentBlock::Text { .. }))
        }));
        assert!(!request.messages.iter().any(|message| {
            message.content.iter().any(|block| match block {
                ChatGptContentBlock::ToolCall { id, .. } => id == "call_missing",
                _ => false,
            })
        }));
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
            chatgpt: None,
            configured: true,
            configuration_url: None,
            configuration_hint: None,
            disable_thinking_via_chat_template: false,
        };
        let body = build_request_body(
            &backend,
            "repo",
            "/tmp/repo",
            &[],
            None,
            None,
            &[],
            &[],
            &[],
            None,
        );
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
        let prompt = render_system_prompt("repo", "/tmp/repo", None, None, &[], &[], &[], None);
        assert!(prompt.contains("mount 'repo'"));
        assert!(prompt.contains("rooted at '/tmp/repo'"));
        assert!(prompt.contains("observe_filesystem"));
        assert!(prompt.contains("open_editor"));
        assert!(prompt.contains("send_terminal_text.submit"));
        assert!(prompt.contains("interpret that as a Makepad app/example"));
        assert!(prompt.contains("not as a Python script, web app"));
    }

    #[test]
    fn render_system_prompt_includes_loaded_skills_and_workflow_focus() {
        let skills = vec![ParsedSkill {
            name: "Semantic Compression".to_string(),
            description: "Guidelines for compressing context".to_string(),
            content: "Use concise summaries for large files.".to_string(),
        }];
        let workflows = vec![ParsedWorkflow {
            name: "review-prs".to_string(),
            steps: vec![
                WorkflowStep {
                    name: "Resolve PR Set".to_string(),
                    description: "Find the pull requests to review.".to_string(),
                },
                WorkflowStep {
                    name: "Verify Changes".to_string(),
                    description: "Run targeted verification.".to_string(),
                },
            ],
        }];
        let active_workflow = ActiveWorkflowState {
            name: "review-prs".to_string(),
            current_step: 0,
            steps: vec![
                WorkflowStepState {
                    name: "Resolve PR Set".to_string(),
                    status: "active".to_string(),
                },
                WorkflowStepState {
                    name: "Verify Changes".to_string(),
                    status: "pending".to_string(),
                },
            ],
        };

        let prompt = render_system_prompt(
            "repo",
            "/tmp/repo",
            None,
            None,
            &[],
            &skills,
            &workflows,
            Some(&active_workflow),
        );

        assert!(prompt.contains("# Workspace Skills"));
        assert!(prompt.contains("## Semantic Compression"));
        assert!(prompt.contains("Guidelines for compressing context"));
        assert!(prompt.contains("Use concise summaries for large files."));
        assert!(prompt.contains("# Current Workflow"));
        assert!(prompt.contains("Workflow: review-prs"));
        assert!(prompt.contains("Current step: 1. Resolve PR Set"));
        assert!(prompt.contains("Status: active"));
        assert!(prompt.contains("Find the pull requests to review."));
    }

    #[test]
    fn slash_workflow_prompt_initializes_active_workflow_and_rewrites_user_message() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.ensure_mount_entry("repo");
        let agent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();
        {
            let mount_state = manager.mounts.get_mut("repo").unwrap();
            mount_state.workflows = vec![ParsedWorkflow {
                name: "review-prs".to_string(),
                steps: vec![
                    WorkflowStep {
                        name: "Resolve PR Set".to_string(),
                        description: "Find PRs.".to_string(),
                    },
                    WorkflowStep {
                        name: "Verify Changes".to_string(),
                        description: "Run checks.".to_string(),
                    },
                ],
            }];
        }

        manager.send_prompt("repo", agent_id, "/review-prs owner/repo#7");

        let mount_state = manager.mounts.get("repo").unwrap();
        let workflow = mount_state.active_workflow.as_ref().unwrap();
        assert_eq!(workflow.name, "review-prs");
        assert_eq!(workflow.current_step, 0);
        assert_eq!(workflow.steps[0].status, "active");
        assert_eq!(workflow.steps[1].status, "pending");
        let agent = mount_state.agents.get(&agent_id).unwrap();
        let ConversationItem::User { text } = &agent.history[0] else {
            panic!("workflow should rewrite the user prompt");
        };
        assert!(text.contains("Execute workflow `review-prs`"));
        assert!(text.contains("Arguments: owner/repo#7"));
        assert!(text.contains("Focus on step 1: Resolve PR Set"));
        assert!(text.contains("Find PRs."));
        assert!(!text.starts_with("/review-prs"));
    }
    #[test]
    fn slash_workflow_prompt_does_not_activate_when_agent_is_pending() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.ensure_mount_entry("repo");
        let agent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();
        {
            let mount_state = manager.mounts.get_mut("repo").unwrap();
            mount_state.workflows = vec![ParsedWorkflow {
                name: "review-prs".to_string(),
                steps: vec![WorkflowStep {
                    name: "Resolve PR Set".to_string(),
                    description: "Find PRs.".to_string(),
                }],
            }];
            let agent = mount_state.agents.get_mut(&agent_id).unwrap();
            agent.status = "thinking...".to_string();
            agent.pending_request_id = Some(LiveId(99));
        }

        manager.send_prompt("repo", agent_id, "/review-prs owner/repo#7");

        let mount_state = manager.mounts.get("repo").unwrap();
        assert!(mount_state.active_workflow.is_none());
        let agent = mount_state.agents.get(&agent_id).unwrap();
        assert!(agent.history.is_empty());
    }

    #[test]
    fn send_prompt_persists_accepted_prompt_before_request() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "makepad_ai_send_prompt_persist_test_{}_{}",
            std::process::id(),
            suffix
        ));
        fs::create_dir_all(&root).unwrap();
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.register_mount("repo", &root);
        let agent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();

        manager.send_prompt("repo", agent_id, "remember this immediately");

        let path = ai_chat_file_path(&root, agent_id);
        let saved = fs::read_to_string(&path).expect("accepted prompt should persist immediately");
        assert!(saved.contains("remember this immediately"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn assistant_completion_advances_active_workflow_step() {
        let (event_tx, _event_rx) = channel();
        let mut manager = AiManager::new(event_tx);
        manager.ensure_mount_entry("repo");
        let agent_id = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_agent_id)
            .unwrap();
        let run_token = 42;
        {
            let mount_state = manager.mounts.get_mut("repo").unwrap();
            mount_state.active_workflow = Some(ActiveWorkflowState {
                name: "review-prs".to_string(),
                current_step: 0,
                steps: vec![
                    WorkflowStepState {
                        name: "Resolve PR Set".to_string(),
                        status: "active".to_string(),
                    },
                    WorkflowStepState {
                        name: "Verify Changes".to_string(),
                        status: "pending".to_string(),
                    },
                ],
            });
            let agent = mount_state.agents.get_mut(&agent_id).unwrap();
            agent.run_token = run_token;
        }

        manager.complete_assistant_turn(
            "repo",
            agent_id,
            run_token,
            AssistantTurn {
                text: "Resolved the PR set.".to_string(),
                thinking_text: String::new(),
                tool_calls: Vec::new(),
                raw_event_sample: String::new(),
            },
            None,
        );

        let workflow = manager
            .mounts
            .get("repo")
            .and_then(|mount_state| mount_state.active_workflow.as_ref())
            .unwrap();
        assert_eq!(workflow.current_step, 1);
        assert_eq!(workflow.steps[0].status, "done");
        assert_eq!(workflow.steps[1].status, "active");
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
