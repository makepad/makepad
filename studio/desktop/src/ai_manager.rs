use crate::{makepad_micro_serde::*, makepad_widgets::*, push_capped_deque, App};
use makepad_ai::{
    Agent, AgentEvent, BackendConfig, ClaudeBackend, GeminiBackend, Message, OpenAiBackend,
    PromptId, SessionConfig, SessionId, StatelessBackendAdapter, ToolDefinition,
};
use makepad_studio_protocol::hub_protocol::TerminalFramebuffer;
use std::collections::VecDeque;
use std::path::PathBuf;

const AI_MANAGER_LOCALHOST_BASE_URL: &str = "http://127.0.0.1:8080/v1/chat/completions";
const AI_MANAGER_LOCALHOST_MODEL: &str = "local-model";
const AI_MANAGER_REPORT_FILE: &str = ".makepad/ai-manager-report.md";
const AI_MANAGER_TOOL_LOG_MAX: usize = 24;
const AI_MANAGER_TASK_OUTPUT_MAX_CHARS: usize = 1600;
const AI_MANAGER_TASK_OUTPUT_MAX_LINES: usize = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiManagerBackend {
    OpenAiLocalhost,
    OpenAiCloud,
    ClaudeApi,
    Gemini,
}

const ALL_AI_MANAGER_BACKENDS: [AiManagerBackend; 4] = [
    AiManagerBackend::OpenAiLocalhost,
    AiManagerBackend::OpenAiCloud,
    AiManagerBackend::ClaudeApi,
    AiManagerBackend::Gemini,
];

impl AiManagerBackend {
    pub fn to_index(self) -> usize {
        ALL_AI_MANAGER_BACKENDS
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0)
    }

    pub fn from_index(index: usize) -> Option<Self> {
        ALL_AI_MANAGER_BACKENDS.get(index).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAiLocalhost => "OpenAI Localhost",
            Self::OpenAiCloud => "OpenAI",
            Self::ClaudeApi => "Claude",
            Self::Gemini => "Gemini",
        }
    }

    fn system_prompt(self) -> &'static str {
        match self {
            Self::OpenAiLocalhost | Self::OpenAiCloud | Self::ClaudeApi | Self::Gemini => {
                "You are the singleton AI manager inside Makepad Studio.\n\n\
Use tools instead of guessing when you need terminal state.\n\
Read a terminal before sending input to it.\n\
Keep track of active Codex sessions, task goals per terminal, surface blockers, and keep the report markdown current.\n\
When a task-specific follow-up prompt arrives, use the task goal plus terminal state to decide the next step.\n\
Be concise, operational, and specific about which terminal path you are talking about."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiManagerMessageRole {
    User,
    Task,
    Assistant,
    Error,
}

#[derive(Clone, Debug)]
pub struct AiManagerMessage {
    pub role: AiManagerMessageRole,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct AiManagerToolLogEntry {
    pub name: String,
    pub summary: String,
    pub ok: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiTerminalAutoAction {
    Continue,
    Return,
}

#[derive(Clone, Debug)]
pub struct AiManagerTask {
    pub id: u64,
    pub goal: String,
    pub terminal_path: String,
    pub last_terminal_mode: String,
    pub last_terminal_summary: String,
    pub last_codex_status: Option<String>,
    pub last_output_excerpt: String,
    pub last_auto_action: Option<String>,
    pub last_auto_action_frame_id: Option<u64>,
    pub pending_followup: bool,
    pub pending_followup_reason: Option<String>,
}

pub struct AiManagerState {
    pub available_backends: Vec<AiManagerBackend>,
    pub active_backend: Option<AiManagerBackend>,
    pub agent: Option<Box<dyn Agent>>,
    pub session_id: Option<SessionId>,
    pub current_prompt: Option<PromptId>,
    pub history_injected: bool,
    pub initialized: bool,
    pub messages: Vec<AiManagerMessage>,
    pub streaming_text: String,
    pub tool_log: VecDeque<AiManagerToolLogEntry>,
    pub report_notes: String,
    pub report_markdown: String,
    pub tasks: Vec<AiManagerTask>,
    pub next_task_id: u64,
    pub draft_task_terminal_path: Option<String>,
}

impl Default for AiManagerState {
    fn default() -> Self {
        Self {
            available_backends: Vec::new(),
            active_backend: None,
            agent: None,
            session_id: None,
            current_prompt: None,
            history_injected: false,
            initialized: false,
            messages: Vec::new(),
            streaming_text: String::new(),
            tool_log: VecDeque::new(),
            report_notes: String::new(),
            report_markdown: String::new(),
            tasks: Vec::new(),
            next_task_id: 1,
            draft_task_terminal_path: None,
        }
    }
}

#[derive(Default, DeJson)]
struct ReadTerminalArgs {
    path: String,
}

#[derive(Default, DeJson)]
struct TerminalInputArgs {
    path: String,
    text: String,
}

#[derive(Default, DeJson)]
struct WriteReportArgs {
    markdown: String,
}

#[derive(SerJson)]
struct ToolAck {
    ok: bool,
    summary: String,
}

#[derive(SerJson)]
struct ToolTerminalSummary {
    mount: String,
    path: String,
    title: String,
    mode: String,
    summary: String,
    is_codex: bool,
    needs_attention: bool,
}

#[derive(SerJson)]
struct ToolLogSummary {
    name: String,
    summary: String,
    ok: bool,
}

#[derive(SerJson)]
struct ToolTaskSummary {
    id: u64,
    goal: String,
    terminal_path: String,
    terminal_title: String,
    state: String,
    terminal_mode: String,
    last_summary: String,
    codex_status: Option<String>,
    pending_followup: bool,
    last_auto_action: Option<String>,
}

#[derive(SerJson)]
struct ManagerContextResult {
    active_mount: Option<String>,
    backend: Option<String>,
    report_path: Option<String>,
    tasks: Vec<ToolTaskSummary>,
    terminals: Vec<ToolTerminalSummary>,
    recent_tools: Vec<ToolLogSummary>,
}

#[derive(SerJson)]
struct ReadTerminalResult {
    path: String,
    title: String,
    mode: String,
    summary: String,
    codex_status: Option<String>,
    visible_text: String,
}

struct AiTerminalSnapshot {
    mount: String,
    path: String,
    title: String,
    mode: &'static str,
    summary: String,
    visible_text: String,
    is_codex: bool,
    needs_attention: bool,
    codex_status: Option<String>,
    auto_action: Option<AiTerminalAutoAction>,
}

impl App {
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

    fn ai_manager_local_base_url() -> String {
        std::env::var("MAKEPAD_AI_MANAGER_BASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| AI_MANAGER_LOCALHOST_BASE_URL.to_string())
    }

    fn ai_manager_local_model() -> String {
        std::env::var("MAKEPAD_AI_MANAGER_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| AI_MANAGER_LOCALHOST_MODEL.to_string())
    }

    fn detect_ai_manager_backends() -> Vec<AiManagerBackend> {
        let mut backends = vec![AiManagerBackend::OpenAiLocalhost];
        if Self::read_secret_or_env("OPENAI_API_KEY").is_some() {
            backends.push(AiManagerBackend::OpenAiCloud);
        }
        if Self::read_secret_or_env("ANTHROPIC_API_KEY").is_some() {
            backends.push(AiManagerBackend::ClaudeApi);
        }
        if Self::read_secret_or_env("GOOGLE_API_KEY").is_some() {
            backends.push(AiManagerBackend::Gemini);
        }
        backends
    }

    fn ai_manager_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "get_manager_context".to_string(),
                description:
                    "Return the current Studio manager context, including task goals, terminals, backend, and report path."
                        .to_string(),
                parameters:
                    "{\"type\":\"object\",\"properties\":{},\"additionalProperties\":false}"
                        .to_string(),
            },
            ToolDefinition {
                name: "read_terminal".to_string(),
                description:
                    "Read the visible contents of a Studio terminal before deciding what to do."
                        .to_string(),
                parameters:
                    "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}},\"required\":[\"path\"],\"additionalProperties\":false}"
                        .to_string(),
            },
            ToolDefinition {
                name: "send_terminal_input".to_string(),
                description:
                    "Send literal text to a Studio terminal without pressing return."
                        .to_string(),
                parameters:
                    "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"},\"text\":{\"type\":\"string\"}},\"required\":[\"path\",\"text\"],\"additionalProperties\":false}"
                        .to_string(),
            },
            ToolDefinition {
                name: "send_terminal_return".to_string(),
                description:
                    "Press return in a Studio terminal after you have read it."
                        .to_string(),
                parameters:
                    "{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}},\"required\":[\"path\"],\"additionalProperties\":false}"
                        .to_string(),
            },
            ToolDefinition {
                name: "write_report".to_string(),
                description:
                    "Replace the manager report notes section with concise markdown."
                        .to_string(),
                parameters:
                    "{\"type\":\"object\",\"properties\":{\"markdown\":{\"type\":\"string\"}},\"required\":[\"markdown\"],\"additionalProperties\":false}"
                        .to_string(),
            },
        ]
    }

    fn create_ai_manager_session_config(backend: AiManagerBackend) -> SessionConfig {
        SessionConfig {
            system_prompt: Some(backend.system_prompt().to_string()),
            tools: Self::ai_manager_tools(),
            ..Default::default()
        }
    }

    fn create_ai_manager_agent(&self, backend: AiManagerBackend) -> Option<Box<dyn Agent>> {
        match backend {
            AiManagerBackend::OpenAiLocalhost => Some(Box::new(StatelessBackendAdapter::new(
                Box::new(OpenAiBackend::new(BackendConfig::OpenAI {
                    api_key: String::new(),
                    model: Self::ai_manager_local_model(),
                    base_url: Some(Self::ai_manager_local_base_url()),
                    reasoning_effort: None,
                })),
            ))),
            AiManagerBackend::OpenAiCloud => {
                Self::read_secret_or_env("OPENAI_API_KEY").map(|api_key| {
                    Box::new(StatelessBackendAdapter::new(Box::new(OpenAiBackend::new(
                        BackendConfig::OpenAI {
                            api_key,
                            model: "gpt-4o".to_string(),
                            base_url: None,
                            reasoning_effort: None,
                        },
                    )))) as Box<dyn Agent>
                })
            }
            AiManagerBackend::ClaudeApi => {
                Self::read_secret_or_env("ANTHROPIC_API_KEY").map(|api_key| {
                    Box::new(StatelessBackendAdapter::new(Box::new(ClaudeBackend::new(
                        BackendConfig::Claude {
                            api_key: Some(api_key),
                            oauth_token: None,
                            model: "claude-sonnet-4-5-20250929".to_string(),
                        },
                    )))) as Box<dyn Agent>
                })
            }
            AiManagerBackend::Gemini => Self::read_secret_or_env("GOOGLE_API_KEY").map(|api_key| {
                Box::new(StatelessBackendAdapter::new(Box::new(GeminiBackend::new(
                    BackendConfig::Gemini {
                        api_key,
                        model: "gemini-3-pro-preview".to_string(),
                    },
                )))) as Box<dyn Agent>
            }),
        }
    }

    fn ai_manager_backend_details(&self, backend: AiManagerBackend) -> String {
        match backend {
            AiManagerBackend::OpenAiLocalhost => format!(
                "{}  {}",
                Self::ai_manager_local_model(),
                Self::ai_manager_local_base_url()
            ),
            AiManagerBackend::OpenAiCloud => "gpt-4o".to_string(),
            AiManagerBackend::ClaudeApi => "claude-sonnet-4-5-20250929".to_string(),
            AiManagerBackend::Gemini => "gemini-3-pro-preview".to_string(),
        }
    }

    pub(super) fn ensure_ai_manager_tab(&mut self, cx: &mut Cx) -> Option<LiveId> {
        let dock = self.ui.dock(cx, ids!(mount_dock));
        if dock.find_tab_bar_of_tab(id!(ai_manager_tab)).is_some() {
            self.sync_mount_tab_bar_visibility(cx);
            return Some(id!(ai_manager_tab));
        }
        let anchor = self
            .data
            .mounts
            .values()
            .filter_map(|state| state.tab_id)
            .next()
            .unwrap_or(id!(mount_first));
        let (tab_bar, _) = Self::reachable_tab_bar_of_tab(&dock, anchor)?;
        dock.create_tab(
            cx,
            tab_bar,
            id!(ai_manager_tab),
            id!(AiManagerPane),
            "AI Manager".to_string(),
            id!(AiManagerTab),
            None,
        )?;
        self.sync_mount_tab_bar_visibility(cx);
        Some(id!(ai_manager_tab))
    }

    pub(super) fn init_ai_manager(&mut self, cx: &mut Cx) {
        let _ = self.ensure_ai_manager_tab(cx);
        self.ai_manager.available_backends = Self::detect_ai_manager_backends();
        if self.ai_manager.initialized {
            self.refresh_ai_manager_report(cx);
            return;
        }
        self.ai_manager.initialized = true;
        let default_backend = self
            .ai_manager
            .available_backends
            .first()
            .copied()
            .unwrap_or(AiManagerBackend::OpenAiLocalhost);
        let _ = self.switch_ai_manager_backend(cx, default_backend);
        self.refresh_ai_manager_report(cx);
    }

    pub(super) fn switch_ai_manager_backend(
        &mut self,
        cx: &mut Cx,
        backend: AiManagerBackend,
    ) -> bool {
        let Some(agent) = self.create_ai_manager_agent(backend) else {
            self.set_ai_manager_status(cx, &format!("{} is not configured", backend.label()));
            if let Some(active) = self.ai_manager.active_backend {
                self.ui
                    .drop_down(cx, ids!(ai_backend_dropdown))
                    .set_selected_item(cx, active.to_index());
            }
            return false;
        };

        self.ai_manager.agent = Some(agent);
        self.ai_manager.active_backend = Some(backend);
        self.ai_manager.session_id = None;
        self.ai_manager.current_prompt = None;
        self.ai_manager.history_injected = false;
        self.ai_manager.streaming_text.clear();

        if let Some(agent) = self.ai_manager.agent.as_mut() {
            self.ai_manager.session_id =
                Some(agent.create_session(cx, Self::create_ai_manager_session_config(backend)));
        }

        self.ui
            .drop_down(cx, ids!(ai_backend_dropdown))
            .set_selected_item(cx, backend.to_index());
        self.set_ai_manager_status(
            cx,
            &format!(
                "{} [{}]",
                backend.label(),
                self.ai_manager_backend_details(backend)
            ),
        );
        self.refresh_ai_manager_report(cx);
        true
    }

    pub(super) fn send_ai_manager_prompt(&mut self, cx: &mut Cx) {
        let input = self.ui.text_input(cx, ids!(ai_prompt_input));
        let text = input.text();
        if self.send_ai_manager_prompt_text(cx, text, AiManagerMessageRole::User) {
            input.set_text(cx, "");
        }
    }

    fn send_ai_manager_prompt_text(
        &mut self,
        cx: &mut Cx,
        text: String,
        role: AiManagerMessageRole,
    ) -> bool {
        if self.ai_manager.current_prompt.is_some() {
            self.set_ai_manager_status(cx, "manager is already running");
            return false;
        }
        if text.trim().is_empty() {
            return false;
        }

        let (agent, session_id) = match (&mut self.ai_manager.agent, self.ai_manager.session_id) {
            (Some(agent), Some(session_id)) => (agent, session_id),
            _ => {
                self.set_ai_manager_status(cx, "manager backend is not ready");
                return false;
            }
        };

        self.ai_manager.messages.push(AiManagerMessage {
            role,
            text: text.clone(),
        });
        self.ai_manager.streaming_text.clear();

        if !self.ai_manager.history_injected && agent.is_stateless() {
            let history: Vec<Message> = self.ai_manager.messages
                [..self.ai_manager.messages.len() - 1]
                .iter()
                .filter_map(|message| match message.role {
                    AiManagerMessageRole::User | AiManagerMessageRole::Task => {
                        Some(Message::user(&message.text))
                    }
                    AiManagerMessageRole::Assistant => Some(Message::assistant(&message.text)),
                    AiManagerMessageRole::Error => None,
                })
                .collect();
            if !history.is_empty() {
                agent.inject_history(session_id, history);
            }
            self.ai_manager.history_injected = true;
        }

        self.ai_manager.current_prompt = Some(agent.send_prompt(cx, session_id, &text));
        self.set_ai_manager_status(
            cx,
            if role == AiManagerMessageRole::Task {
                "manager applying task..."
            } else {
                "manager running..."
            },
        );
        self.sync_ai_manager_widgets(cx);
        true
    }

    pub(super) fn add_ai_manager_task(&mut self, cx: &mut Cx) {
        let goal = self
            .ui
            .text_input(cx, ids!(ai_task_input))
            .text()
            .trim()
            .to_string();
        let Some(path) = self.ai_manager.draft_task_terminal_path.clone() else {
            self.set_ai_manager_status(cx, "choose a terminal for the task");
            return;
        };
        if goal.is_empty() {
            self.set_ai_manager_status(cx, "enter a task goal");
            return;
        }

        let snapshot = self.ai_terminal_snapshot(&path);
        let pending_followup = snapshot.mode != "working";
        let task_id = self.ai_manager.next_task_id;
        self.ai_manager.next_task_id += 1;
        self.ai_manager.tasks.push(AiManagerTask {
            id: task_id,
            goal: goal.clone(),
            terminal_path: path.clone(),
            last_terminal_mode: snapshot.mode.to_string(),
            last_terminal_summary: snapshot.summary.clone(),
            last_codex_status: snapshot.codex_status.clone(),
            last_output_excerpt: Self::truncate_terminal_excerpt(
                &snapshot.visible_text,
                AI_MANAGER_TASK_OUTPUT_MAX_CHARS,
                AI_MANAGER_TASK_OUTPUT_MAX_LINES,
            ),
            last_auto_action: None,
            last_auto_action_frame_id: None,
            pending_followup,
            pending_followup_reason: pending_followup.then_some("task created".to_string()),
        });
        self.ui.text_input(cx, ids!(ai_task_input)).set_text(cx, "");
        self.set_ai_manager_status(
            cx,
            &format!("added task {} for {}", task_id, snapshot.title),
        );
        self.process_ai_manager_task_terminal_update(cx, &path);
        self.refresh_ai_manager_report(cx);
        let _ = self.dispatch_next_ai_manager_task_followup(cx);
    }

    pub(super) fn clear_ai_manager_tasks(&mut self, cx: &mut Cx) {
        self.ai_manager.tasks.clear();
        self.set_ai_manager_status(cx, "cleared AI manager tasks");
        self.refresh_ai_manager_report(cx);
    }

    pub(super) fn process_ai_manager_task_terminal_update(&mut self, cx: &mut Cx, path: &str) {
        let task_indices: Vec<usize> = self
            .ai_manager
            .tasks
            .iter()
            .enumerate()
            .filter_map(|(index, task)| (task.terminal_path == path).then_some(index))
            .collect();
        if task_indices.is_empty() {
            return;
        }

        let snapshot = self.ai_terminal_snapshot(path);
        let frame_id = self.data.terminal_frame_id_by_path.get(path).copied();
        let output_excerpt = Self::truncate_terminal_excerpt(
            &snapshot.visible_text,
            AI_MANAGER_TASK_OUTPUT_MAX_CHARS,
            AI_MANAGER_TASK_OUTPUT_MAX_LINES,
        );
        let mut queued_followup = false;
        let mut should_auto_act = false;
        let mut did_change = false;

        for &index in &task_indices {
            let task = &mut self.ai_manager.tasks[index];
            let previous_mode = task.last_terminal_mode.clone();
            if task.last_terminal_mode != snapshot.mode
                || task.last_terminal_summary != snapshot.summary
                || task.last_codex_status != snapshot.codex_status
                || task.last_output_excerpt != output_excerpt
            {
                did_change = true;
            }
            task.last_terminal_mode = snapshot.mode.to_string();
            task.last_terminal_summary = snapshot.summary.clone();
            task.last_codex_status = snapshot.codex_status.clone();
            task.last_output_excerpt = output_excerpt.clone();

            if previous_mode == "working" && snapshot.mode == "done" {
                task.pending_followup = true;
                task.pending_followup_reason =
                    Some("assigned terminal finished a working step".to_string());
                queued_followup = true;
                did_change = true;
            }

            if snapshot.auto_action.is_some() && task.last_auto_action_frame_id != frame_id {
                should_auto_act = true;
            }
        }

        if should_auto_act {
            let (data, action_summary, tool_name) = match snapshot.auto_action {
                Some(AiTerminalAutoAction::Continue) => (
                    b"continue\r".to_vec(),
                    format!("sent continue to {}", path),
                    "task_auto_continue",
                ),
                Some(AiTerminalAutoAction::Return) => (
                    b"\r".to_vec(),
                    format!("pressed return in {}", path),
                    "task_auto_return",
                ),
                None => (Vec::new(), String::new(), ""),
            };
            if !data.is_empty() {
                self.send_terminal_input(path, data);
                self.log_ai_manager_tool(tool_name, action_summary.clone(), true);
                for &index in &task_indices {
                    let task = &mut self.ai_manager.tasks[index];
                    task.last_auto_action = Some(action_summary.clone());
                    task.last_auto_action_frame_id = frame_id;
                }
                did_change = true;
            }
        }

        if queued_followup {
            let _ = self.dispatch_next_ai_manager_task_followup(cx);
        }
        if did_change {
            self.refresh_ai_manager_report(cx);
        }
    }

    pub(super) fn cancel_ai_manager_prompt(&mut self, cx: &mut Cx) {
        if let (Some(agent), Some(prompt_id)) = (
            &mut self.ai_manager.agent,
            self.ai_manager.current_prompt.take(),
        ) {
            agent.cancel_prompt(cx, prompt_id);
        }
        self.commit_ai_manager_streaming_message();
        self.set_ai_manager_status(cx, "manager stopped");
        self.refresh_ai_manager_report(cx);
    }

    fn commit_ai_manager_streaming_message(&mut self) {
        let text = std::mem::take(&mut self.ai_manager.streaming_text);
        if text.trim().is_empty() {
            return;
        }
        self.ai_manager.messages.push(AiManagerMessage {
            role: AiManagerMessageRole::Assistant,
            text,
        });
    }

    fn set_ai_manager_status(&mut self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(ai_status_label)).set_text(cx, text);
    }

    fn ai_manager_report_path(&self) -> Option<PathBuf> {
        let mount = self
            .data
            .active_mount
            .as_ref()
            .or_else(|| self.data.mounts.keys().next())?;
        Some(
            self.data
                .mounts
                .get(mount)?
                .root
                .join(AI_MANAGER_REPORT_FILE),
        )
    }

    fn truncate_for_summary(text: &str, max_chars: usize) -> String {
        let trimmed = text.trim();
        if trimmed.chars().count() <= max_chars {
            return trimmed.to_string();
        }
        let mut out = String::new();
        for ch in trimmed.chars().take(max_chars.saturating_sub(1)) {
            out.push(ch);
        }
        out.push('…');
        out
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
            .take(max_chars.saturating_sub(1))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{}", tail)
    }

    fn ai_terminal_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for mount_state in self.data.mounts.values() {
            for path in &mount_state.terminal_files {
                if !paths.iter().any(|existing| existing == path) {
                    paths.push(path.clone());
                }
            }
        }
        for path in self.data.terminal_framebuffer_by_path.keys() {
            if !paths.iter().any(|existing| existing == path) {
                paths.push(path.clone());
            }
        }
        paths.sort();
        paths
    }

    fn decode_terminal_frame_lines(frame: &TerminalFramebuffer) -> Vec<String> {
        let cols = frame.cols as usize;
        let rows = frame.rows as usize;
        let mut lines = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut line = String::with_capacity(cols);
            for col in 0..cols {
                let idx = (row * cols + col) * 10;
                if idx + 3 >= frame.cells.len() {
                    break;
                }
                let codepoint = u32::from_le_bytes([
                    frame.cells[idx],
                    frame.cells[idx + 1],
                    frame.cells[idx + 2],
                    frame.cells[idx + 3],
                ]);
                line.push(char::from_u32(codepoint).unwrap_or(' '));
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }

    fn is_codex_prompt_line(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.starts_with('›')
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

    fn detect_codex_auto_action(lowered: &str) -> Option<AiTerminalAutoAction> {
        let wants_continue = lowered.contains("type 'continue'")
            || lowered.contains("type \"continue\"")
            || lowered.contains("type continue")
            || lowered.contains("reply with continue")
            || lowered.contains("send continue")
            || lowered.contains("allow this action")
            || lowered.contains("allow the action")
            || lowered.contains("would you like to allow")
            || lowered.contains("approve this action")
            || lowered.contains("approve this request")
            || lowered.contains("escalated privileges")
            || lowered.contains("require_escalated");
        if wants_continue {
            return Some(AiTerminalAutoAction::Continue);
        }

        let wants_return = lowered.contains("press enter to continue")
            || lowered.contains("press return to continue")
            || lowered.contains("hit enter to continue")
            || lowered.contains("hit return to continue")
            || lowered.contains("enter to continue")
            || lowered.contains("press enter when ready")
            || lowered.contains("press return when ready");
        if wants_return {
            return Some(AiTerminalAutoAction::Return);
        }
        None
    }

    fn terminal_summary_line(
        lines: &[String],
        is_codex: bool,
        codex_status: Option<&str>,
    ) -> String {
        let summary = lines
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
                            || line.contains("left ·")))
            })
            .map(|line| Self::truncate_for_summary(line, 140));
        summary.unwrap_or_else(|| "No visible output yet".to_string())
    }

    fn terminal_mode_and_summary(
        title: &str,
        visible_text: &str,
    ) -> (
        &'static str,
        bool,
        bool,
        String,
        Option<String>,
        Option<AiTerminalAutoAction>,
    ) {
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
        let auto_action = if is_codex {
            Self::detect_codex_auto_action(&lowered)
        } else {
            None
        };
        let needs_attention = lowered.contains("permission denied")
            || lowered.contains("sandbox")
            || lowered.contains("panic")
            || lowered.contains("error:")
            || lowered.contains("failed")
            || lowered.contains("blocked")
            || lowered.contains("approve");
        let awaiting_input = lowered.contains("how would you like to proceed")
            || lowered.contains("need user input")
            || lowered.contains("waiting for user")
            || lowered.contains("request user input")
            || lowered.contains("press enter")
            || lowered.contains("continue?")
            || auto_action.is_some();
        let working = lowered.contains("apply_patch")
            || lowered.contains("exec_command")
            || lowered.contains("searching")
            || lowered.contains("reading")
            || lowered.contains("building")
            || lowered.contains("testing")
            || lowered.contains("running")
            || codex_status.is_some()
            || lowered.contains("patching");
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

        let summary = Self::terminal_summary_line(&lines, is_codex, codex_status.as_deref());

        (
            mode,
            is_codex,
            needs_attention,
            summary,
            codex_status,
            auto_action,
        )
    }

    fn ai_terminal_snapshot(&self, path: &str) -> AiTerminalSnapshot {
        let title = self.terminal_tab_title(path);
        let visible_text = self
            .data
            .terminal_framebuffer_by_path
            .get(path)
            .map(Self::decode_terminal_frame_lines)
            .unwrap_or_default()
            .join("\n");
        let (mode, is_codex, needs_attention, summary, codex_status, auto_action) =
            Self::terminal_mode_and_summary(&title, &visible_text);
        AiTerminalSnapshot {
            mount: Self::mount_from_virtual_path(path)
                .unwrap_or_default()
                .to_string(),
            path: path.to_string(),
            title,
            mode,
            summary,
            visible_text,
            is_codex,
            needs_attention,
            codex_status,
            auto_action,
        }
    }

    fn ai_terminal_snapshots(&self) -> Vec<AiTerminalSnapshot> {
        self.ai_terminal_paths()
            .into_iter()
            .map(|path| self.ai_terminal_snapshot(&path))
            .collect()
    }

    fn ai_manager_task_terminal_labels(&self) -> Vec<(String, String)> {
        self.ai_terminal_snapshots()
            .into_iter()
            .map(|terminal| {
                (
                    terminal.path.clone(),
                    format!("{} [{}]", terminal.title, terminal.mode),
                )
            })
            .collect()
    }

    pub(super) fn set_ai_manager_task_terminal_selection(&mut self, cx: &mut Cx, index: usize) {
        self.ai_manager.draft_task_terminal_path = if index == 0 {
            None
        } else {
            self.ai_manager_task_terminal_labels()
                .into_iter()
                .nth(index - 1)
                .map(|(path, _)| path)
        };
        self.sync_ai_manager_widgets(cx);
    }

    fn ai_manager_task_state(task: &AiManagerTask) -> &'static str {
        if task.pending_followup {
            "queued-followup"
        } else {
            "active"
        }
    }

    fn ai_manager_tasks_markdown(&self) -> String {
        if self.ai_manager.tasks.is_empty() {
            return "_No tracked tasks yet._".to_string();
        }

        let mut markdown = String::new();
        for task in &self.ai_manager.tasks {
            markdown.push_str(&format!("### Task {}\n\n", task.id));
            markdown.push_str(&format!("**Goal:** {}\n\n", task.goal));
            markdown.push_str(&format!(
                "**Terminal:** {} `{}`\n\n",
                self.terminal_tab_title(&task.terminal_path),
                task.terminal_path
            ));
            markdown.push_str(&format!(
                "**Task State:** {}\n\n",
                Self::ai_manager_task_state(task)
            ));
            markdown.push_str(&format!(
                "**Terminal Mode:** {}\n\n",
                task.last_terminal_mode
            ));
            if let Some(codex_status) = &task.last_codex_status {
                markdown.push_str(&format!("**Codex Status:** {}\n\n", codex_status));
            }
            markdown.push_str(&format!(
                "**Last Summary:** {}\n\n",
                task.last_terminal_summary
            ));
            if let Some(last_auto_action) = &task.last_auto_action {
                markdown.push_str(&format!("**Last Auto Action:** {}\n\n", last_auto_action));
            }
            if !task.last_output_excerpt.is_empty() {
                markdown.push_str("**Latest Output Excerpt:**\n\n```text\n");
                markdown.push_str(&task.last_output_excerpt);
                markdown.push_str("\n```\n\n");
            }
        }
        markdown
    }

    fn ai_manager_task_prompt(&self, task: &AiManagerTask, reason: &str) -> String {
        let terminal = self.ai_terminal_snapshot(&task.terminal_path);
        let mut prompt = String::new();
        prompt.push_str(&format!(
            "Task {} for terminal `{}` needs a follow-up.\n\n",
            task.id, task.terminal_path
        ));
        prompt.push_str(&format!("Reason: {}\n", reason));
        prompt.push_str(&format!("Goal: {}\n", task.goal));
        prompt.push_str(&format!("Terminal title: {}\n", terminal.title));
        prompt.push_str(&format!("Terminal mode: {}\n", terminal.mode));
        if let Some(codex_status) = &terminal.codex_status {
            prompt.push_str(&format!("Codex status: {}\n", codex_status));
        }
        prompt.push_str(&format!("Visible summary: {}\n\n", terminal.summary));
        prompt.push_str(
            "Read the terminal, decide the next concrete step for this task, keep it moving if appropriate, and update the report.",
        );
        prompt
    }

    fn dispatch_next_ai_manager_task_followup(&mut self, cx: &mut Cx) -> bool {
        if self.ai_manager.current_prompt.is_some() {
            return false;
        }

        let Some(task_index) = self
            .ai_manager
            .tasks
            .iter()
            .position(|task| task.pending_followup)
        else {
            return false;
        };

        let prompt = {
            let task = &self.ai_manager.tasks[task_index];
            self.ai_manager_task_prompt(
                task,
                task.pending_followup_reason
                    .as_deref()
                    .unwrap_or("task follow-up"),
            )
        };

        if !self.send_ai_manager_prompt_text(cx, prompt, AiManagerMessageRole::Task) {
            return false;
        }

        if let Some(task) = self.ai_manager.tasks.get_mut(task_index) {
            task.pending_followup = false;
            task.pending_followup_reason = None;
        }
        true
    }

    fn ai_manager_chat_markdown(&self) -> String {
        let mut markdown = String::new();
        if self.ai_manager.messages.is_empty() && self.ai_manager.streaming_text.is_empty() {
            markdown.push_str("_No manager conversation yet._");
            return markdown;
        }

        for message in &self.ai_manager.messages {
            let heading = match message.role {
                AiManagerMessageRole::User => "### User",
                AiManagerMessageRole::Task => "### Task",
                AiManagerMessageRole::Assistant => "### Manager",
                AiManagerMessageRole::Error => "### Error",
            };
            markdown.push_str(heading);
            markdown.push_str("\n\n");
            markdown.push_str(&message.text);
            markdown.push_str("\n\n");
        }

        if !self.ai_manager.streaming_text.is_empty() {
            markdown.push_str("### Manager\n\n");
            markdown.push_str(&self.ai_manager.streaming_text);
            markdown.push_str("\n");
        }

        markdown
    }

    fn ai_manager_overview_markdown(&self) -> String {
        let mut markdown = String::new();
        markdown.push_str("### Tasks\n\n");

        if self.ai_manager.tasks.is_empty() {
            markdown.push_str("_No tracked tasks yet._\n");
        } else {
            for task in &self.ai_manager.tasks {
                markdown.push_str(&format!(
                    "- **Task {}** [{} / {}] `{}`\n",
                    task.id,
                    Self::ai_manager_task_state(task),
                    task.last_terminal_mode,
                    task.terminal_path
                ));
                markdown.push_str(&format!("  {}\n", task.goal));
                markdown.push_str(&format!("  {}\n", task.last_terminal_summary));
            }
        }

        markdown.push_str("\n");
        markdown.push_str("### Terminals\n\n");

        let terminals = self.ai_terminal_snapshots();
        if terminals.is_empty() {
            markdown.push_str("_No terminals yet._\n");
        } else {
            for terminal in terminals {
                let kind = if terminal.is_codex { "codex" } else { "shell" };
                markdown.push_str(&format!(
                    "- **{}** `{}`  [{} / {}]\n",
                    terminal.title, terminal.path, terminal.mode, kind
                ));
                if let Some(codex_status) = &terminal.codex_status {
                    markdown.push_str(&format!("  {}\n", codex_status));
                }
                markdown.push_str(&format!("  {}\n", terminal.summary));
            }
        }

        markdown.push_str("\n### Recent Tools\n\n");
        if self.ai_manager.tool_log.is_empty() {
            markdown.push_str("_No tool calls yet._\n");
        } else {
            for entry in self.ai_manager.tool_log.iter().rev().take(10) {
                let state = if entry.ok { "ok" } else { "error" };
                markdown.push_str(&format!(
                    "- **{}** [{}] {}\n",
                    entry.name, state, entry.summary
                ));
            }
        }

        markdown
    }

    fn rebuild_ai_manager_report(&mut self) {
        let terminals = self.ai_terminal_snapshots();
        let mut markdown = String::new();
        markdown.push_str("# AI Manager Report\n\n");

        if let Some(backend) = self.ai_manager.active_backend {
            markdown.push_str("## Backend\n\n");
            markdown.push_str(&format!(
                "- **Backend:** {}\n- **Target:** {}\n\n",
                backend.label(),
                self.ai_manager_backend_details(backend)
            ));
        }

        markdown.push_str("## Context\n\n");
        markdown.push_str(&format!(
            "- **Active mount:** {}\n- **Tracked terminals:** {}\n\n",
            self.data
                .active_mount
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            terminals.len()
        ));

        markdown.push_str("## Tasks\n\n");
        if self.ai_manager.tasks.is_empty() {
            markdown.push_str("_No tracked tasks yet._\n\n");
        } else {
            for task in &self.ai_manager.tasks {
                markdown.push_str(&format!("### Task {}\n\n", task.id));
                markdown.push_str(&format!("- **Goal:** {}\n", task.goal));
                markdown.push_str(&format!(
                    "- **Terminal:** {} `{}`\n",
                    self.terminal_tab_title(&task.terminal_path),
                    task.terminal_path
                ));
                markdown.push_str(&format!(
                    "- **Task state:** {}\n",
                    Self::ai_manager_task_state(task)
                ));
                markdown.push_str(&format!(
                    "- **Terminal mode:** {}\n",
                    task.last_terminal_mode
                ));
                markdown.push_str(&format!(
                    "- **Last summary:** {}\n",
                    task.last_terminal_summary
                ));
                if let Some(codex_status) = &task.last_codex_status {
                    markdown.push_str(&format!("- **Codex status:** {}\n", codex_status));
                }
                if let Some(last_auto_action) = &task.last_auto_action {
                    markdown.push_str(&format!("- **Last auto action:** {}\n", last_auto_action));
                }
                markdown.push('\n');
            }
        }

        markdown.push_str("## Notes\n\n");
        if self.ai_manager.report_notes.trim().is_empty() {
            markdown.push_str("_No manager notes yet._\n\n");
        } else {
            markdown.push_str(&self.ai_manager.report_notes);
            markdown.push_str("\n\n");
        }

        markdown.push_str("## Terminal Summary\n\n");
        if terminals.is_empty() {
            markdown.push_str("_No terminals yet._\n\n");
        } else {
            let tracked_paths: std::collections::HashSet<&str> = self
                .ai_manager
                .tasks
                .iter()
                .map(|task| task.terminal_path.as_str())
                .collect();
            for terminal in terminals {
                markdown.push_str(&format!(
                    "- **{}** `{}` [{}]\n",
                    terminal.title, terminal.path, terminal.mode
                ));
                if tracked_paths.contains(terminal.path.as_str()) {
                    markdown.push_str("  Assigned to at least one task.\n");
                }
                if let Some(codex_status) = &terminal.codex_status {
                    markdown.push_str(&format!("  {}\n", codex_status));
                }
                markdown.push_str(&format!("  {}\n", terminal.summary));
            }
            markdown.push('\n');
        }

        markdown.push_str("## Recent Tool Calls\n\n");
        if self.ai_manager.tool_log.is_empty() {
            markdown.push_str("_No tool calls yet._\n");
        } else {
            for entry in self.ai_manager.tool_log.iter().rev().take(12) {
                markdown.push_str(&format!(
                    "- **{}** [{}] {}\n",
                    entry.name,
                    if entry.ok { "ok" } else { "error" },
                    entry.summary
                ));
            }
        }

        self.ai_manager.report_markdown = markdown;
    }

    fn write_ai_manager_report_to_disk(&mut self, cx: &mut Cx) {
        let Some(path) = self.ai_manager_report_path() else {
            self.ui
                .label(cx, ids!(ai_report_path_label))
                .set_text(cx, "No report path");
            return;
        };
        self.ui
            .label(cx, ids!(ai_report_path_label))
            .set_text(cx, &path.display().to_string());
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, &self.ai_manager.report_markdown);
    }

    pub(super) fn sync_ai_manager_widgets(&mut self, cx: &mut Cx) {
        self.ui
            .widget(cx, ids!(ai_chat_markdown))
            .set_text(cx, &self.ai_manager_chat_markdown());
        self.ui
            .widget(cx, ids!(ai_tasks_markdown))
            .set_text(cx, &self.ai_manager_tasks_markdown());
        self.ui
            .widget(cx, ids!(ai_overview_markdown))
            .set_text(cx, &self.ai_manager_overview_markdown());
        self.ui
            .widget(cx, ids!(ai_report_markdown))
            .set_text(cx, &self.ai_manager.report_markdown);
        self.ui
            .view(cx, ids!(ai_cancel_button))
            .set_visible(cx, self.ai_manager.current_prompt.is_some());
        if let Some(backend) = self.ai_manager.active_backend {
            self.ui
                .drop_down(cx, ids!(ai_backend_dropdown))
                .set_selected_item(cx, backend.to_index());
        }
        let terminals = self.ai_manager_task_terminal_labels();
        let labels = std::iter::once("Choose terminal…".to_string())
            .chain(terminals.iter().map(|(_, label)| label.clone()))
            .collect::<Vec<_>>();
        self.ui
            .drop_down(cx, ids!(ai_task_terminal_dropdown))
            .set_labels(cx, labels);
        let selected_index = self
            .ai_manager
            .draft_task_terminal_path
            .as_ref()
            .and_then(|selected_path| {
                terminals
                    .iter()
                    .position(|(path, _)| path == selected_path)
                    .map(|index| index + 1)
            })
            .unwrap_or(0);
        self.ui
            .drop_down(cx, ids!(ai_task_terminal_dropdown))
            .set_selected_item(cx, selected_index);
        if let Some(path) = self.ai_manager_report_path() {
            self.ui
                .label(cx, ids!(ai_report_path_label))
                .set_text(cx, &path.display().to_string());
        }
    }

    pub(super) fn refresh_ai_manager_report(&mut self, cx: &mut Cx) {
        self.rebuild_ai_manager_report();
        self.write_ai_manager_report_to_disk(cx);
        self.sync_ai_manager_widgets(cx);
    }

    pub(super) fn refresh_ai_manager_preview(&mut self, cx: &mut Cx) {
        self.rebuild_ai_manager_report();
        self.sync_ai_manager_widgets(cx);
    }

    fn log_ai_manager_tool(&mut self, name: &str, summary: String, ok: bool) {
        push_capped_deque(
            &mut self.ai_manager.tool_log,
            AiManagerToolLogEntry {
                name: name.to_string(),
                summary,
                ok,
            },
            AI_MANAGER_TOOL_LOG_MAX,
        );
    }

    fn execute_ai_manager_tool(
        &mut self,
        cx: &mut Cx,
        tool_name: &str,
        tool_input: &str,
    ) -> (String, bool) {
        match tool_name {
            "get_manager_context" => {
                let result = ManagerContextResult {
                    active_mount: self.data.active_mount.clone(),
                    backend: self
                        .ai_manager
                        .active_backend
                        .map(|backend| backend.label().to_string()),
                    report_path: self
                        .ai_manager_report_path()
                        .map(|path| path.display().to_string()),
                    tasks: self
                        .ai_manager
                        .tasks
                        .iter()
                        .map(|task| ToolTaskSummary {
                            id: task.id,
                            goal: task.goal.clone(),
                            terminal_path: task.terminal_path.clone(),
                            terminal_title: self.terminal_tab_title(&task.terminal_path),
                            state: Self::ai_manager_task_state(task).to_string(),
                            terminal_mode: task.last_terminal_mode.clone(),
                            last_summary: task.last_terminal_summary.clone(),
                            codex_status: task.last_codex_status.clone(),
                            pending_followup: task.pending_followup,
                            last_auto_action: task.last_auto_action.clone(),
                        })
                        .collect(),
                    terminals: self
                        .ai_terminal_snapshots()
                        .into_iter()
                        .map(|terminal| ToolTerminalSummary {
                            mount: terminal.mount,
                            path: terminal.path,
                            title: terminal.title,
                            mode: terminal.mode.to_string(),
                            summary: terminal.summary,
                            is_codex: terminal.is_codex,
                            needs_attention: terminal.needs_attention,
                        })
                        .collect(),
                    recent_tools: self
                        .ai_manager
                        .tool_log
                        .iter()
                        .rev()
                        .take(10)
                        .map(|entry| ToolLogSummary {
                            name: entry.name.clone(),
                            summary: entry.summary.clone(),
                            ok: entry.ok,
                        })
                        .collect(),
                };
                let summary = format!(
                    "captured context for {} terminal(s)",
                    result.terminals.len()
                );
                self.log_ai_manager_tool(tool_name, summary, true);
                (result.serialize_json(), false)
            }
            "read_terminal" => match ReadTerminalArgs::deserialize_json(tool_input) {
                Ok(args) => {
                    let terminal = self.ai_terminal_snapshot(&args.path);
                    let summary = format!("read {}", terminal.path);
                    self.log_ai_manager_tool(tool_name, summary, true);
                    (
                        ReadTerminalResult {
                            path: terminal.path,
                            title: terminal.title,
                            mode: terminal.mode.to_string(),
                            summary: terminal.summary,
                            codex_status: terminal.codex_status,
                            visible_text: terminal.visible_text,
                        }
                        .serialize_json(),
                        false,
                    )
                }
                Err(error) => (
                    ToolAck {
                        ok: false,
                        summary: format!("invalid read_terminal args: {:?}", error),
                    }
                    .serialize_json(),
                    true,
                ),
            },
            "send_terminal_input" => match TerminalInputArgs::deserialize_json(tool_input) {
                Ok(args) => {
                    self.send_terminal_input(&args.path, args.text.clone().into_bytes());
                    let summary = format!("sent input to {}", args.path);
                    self.log_ai_manager_tool(tool_name, summary.clone(), true);
                    self.refresh_ai_manager_report(cx);
                    (ToolAck { ok: true, summary }.serialize_json(), false)
                }
                Err(error) => (
                    ToolAck {
                        ok: false,
                        summary: format!("invalid send_terminal_input args: {:?}", error),
                    }
                    .serialize_json(),
                    true,
                ),
            },
            "send_terminal_return" => match ReadTerminalArgs::deserialize_json(tool_input) {
                Ok(args) => {
                    self.send_terminal_input(&args.path, b"\r".to_vec());
                    let summary = format!("pressed return in {}", args.path);
                    self.log_ai_manager_tool(tool_name, summary.clone(), true);
                    self.refresh_ai_manager_report(cx);
                    (ToolAck { ok: true, summary }.serialize_json(), false)
                }
                Err(error) => (
                    ToolAck {
                        ok: false,
                        summary: format!("invalid send_terminal_return args: {:?}", error),
                    }
                    .serialize_json(),
                    true,
                ),
            },
            "write_report" => match WriteReportArgs::deserialize_json(tool_input) {
                Ok(args) => {
                    self.ai_manager.report_notes = args.markdown;
                    self.log_ai_manager_tool(tool_name, "updated report notes".to_string(), true);
                    self.refresh_ai_manager_report(cx);
                    (
                        ToolAck {
                            ok: true,
                            summary: "report notes updated".to_string(),
                        }
                        .serialize_json(),
                        false,
                    )
                }
                Err(error) => (
                    ToolAck {
                        ok: false,
                        summary: format!("invalid write_report args: {:?}", error),
                    }
                    .serialize_json(),
                    true,
                ),
            },
            _ => (
                ToolAck {
                    ok: false,
                    summary: format!("unknown tool {}", tool_name),
                }
                .serialize_json(),
                true,
            ),
        }
    }

    pub(super) fn handle_ai_manager_agent_events(&mut self, cx: &mut Cx, event: &Event) {
        let events = match self.ai_manager.agent.as_mut() {
            Some(agent) => agent.handle_event(cx, event),
            None => return,
        };

        for agent_event in events {
            match agent_event {
                AgentEvent::SessionReady { .. } => {
                    if let Some(backend) = self.ai_manager.active_backend {
                        self.set_ai_manager_status(
                            cx,
                            &format!(
                                "{} [{}]",
                                backend.label(),
                                self.ai_manager_backend_details(backend)
                            ),
                        );
                    }
                }
                AgentEvent::SessionError { error, .. } => {
                    self.ai_manager.messages.push(AiManagerMessage {
                        role: AiManagerMessageRole::Error,
                        text: error.clone(),
                    });
                    self.set_ai_manager_status(cx, &format!("session error: {}", error));
                    self.refresh_ai_manager_report(cx);
                }
                AgentEvent::TextDelta { text, .. } => {
                    self.ai_manager.streaming_text.push_str(&text);
                    self.sync_ai_manager_widgets(cx);
                }
                AgentEvent::ToolRequest {
                    tool_use_id,
                    tool_name,
                    tool_input,
                    ..
                } => {
                    self.commit_ai_manager_streaming_message();
                    let (result, is_error) =
                        self.execute_ai_manager_tool(cx, &tool_name, &tool_input);
                    if let (Some(agent), Some(session_id)) =
                        (&mut self.ai_manager.agent, self.ai_manager.session_id)
                    {
                        agent.send_tool_result(cx, session_id, &tool_use_id, &result, is_error);
                    }
                    self.sync_ai_manager_widgets(cx);
                }
                AgentEvent::TurnComplete { .. } => {
                    self.commit_ai_manager_streaming_message();
                    self.ai_manager.current_prompt = None;
                    self.set_ai_manager_status(cx, "manager idle");
                    self.refresh_ai_manager_report(cx);
                    let _ = self.dispatch_next_ai_manager_task_followup(cx);
                }
                AgentEvent::PromptError { error, .. } => {
                    self.ai_manager.current_prompt = None;
                    self.ai_manager.streaming_text.clear();
                    self.ai_manager.messages.push(AiManagerMessage {
                        role: AiManagerMessageRole::Error,
                        text: error.clone(),
                    });
                    self.set_ai_manager_status(cx, &format!("prompt error: {}", error));
                    self.refresh_ai_manager_report(cx);
                    let _ = self.dispatch_next_ai_manager_task_followup(cx);
                }
            }
        }
    }
}
