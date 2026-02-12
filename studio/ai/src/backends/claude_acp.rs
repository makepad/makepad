//! Claude ACP (Agent Client Protocol) backend
//!
//! Uses Zed's Claude Code ACP installation to provide Claude access
//! via Claude Pro/Max subscriptions (OAuth-based).

use crate::agent::*;
use crate::types::*;
use makepad_micro_serde::*;
use makepad_widgets2::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

// === JSON-RPC Types for ACP Protocol ===

#[derive(DeJson, Debug)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<u64>,
    result: Option<JsonValue>,
    error: Option<JsonRpcError>,
}

#[derive(DeJson, Debug)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[allow(dead_code)]
    data: Option<JsonValue>,
}

#[derive(DeJson, Debug)]
struct JsonRpcNotification {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    method: String,
    params: Option<JsonValue>,
}

// === ACP Protocol Types ===

#[derive(SerJson, Debug)]
struct InitializeRequest {
    #[rename(protocolVersion)]
    protocol_version: u32,
    #[rename(clientInfo)]
    client_info: ClientInfo,
    #[rename(clientCapabilities)]
    client_capabilities: ClientCapabilities,
}

#[derive(SerJson, Debug)]
struct ClientInfo {
    name: String,
    version: String,
}

#[derive(SerJson, Debug)]
struct ClientCapabilities {
    fs: Option<FsCapability>,
    terminal: Option<bool>,
}

#[derive(SerJson, Debug)]
struct FsCapability {
    #[rename(readTextFile)]
    read_text_file: bool,
    #[rename(writeTextFile)]
    write_text_file: bool,
}

#[derive(DeJson, Debug)]
struct InitializeResponse {
    #[rename(protocolVersion)]
    #[allow(dead_code)]
    protocol_version: Option<u32>,
    #[allow(dead_code)]
    #[rename(agentInfo)]
    agent_info: Option<AgentInfo>,
    #[rename(agentCapabilities)]
    #[allow(dead_code)]
    agent_capabilities: Option<JsonValue>,
    #[rename(authMethods)]
    #[allow(dead_code)]
    auth_methods: Option<Vec<AuthMethod>>,
}

#[derive(DeJson, Debug)]
#[allow(dead_code)]
struct AgentInfo {
    name: Option<String>,
    title: Option<String>,
    version: Option<String>,
}

#[derive(DeJson, Debug)]
#[allow(dead_code)]
struct AuthMethod {
    id: String,
    name: Option<String>,
    description: Option<String>,
}

#[derive(SerJson, Debug)]
struct NewSessionRequest {
    cwd: String,
    #[rename(mcpServers)]
    mcp_servers: Vec<McpServer>,
}

#[derive(SerJson, Debug)]
struct McpServer {
    // Empty struct for empty array serialization
}

#[derive(DeJson, Debug)]
struct NewSessionResponse {
    #[rename(sessionId)]
    session_id: String,
    #[allow(dead_code)]
    models: Option<JsonValue>,
    #[allow(dead_code)]
    modes: Option<JsonValue>,
}

#[derive(SerJson, Debug)]
struct PromptRequest {
    #[rename(sessionId)]
    session_id: String,
    prompt: Vec<PromptContent>,
}

#[derive(SerJson, Debug)]
struct PromptContent {
    #[rename(type)]
    content_type: String,
    text: Option<String>,
}

#[derive(DeJson, Debug)]
struct PromptResponse {
    #[rename(stopReason)]
    stop_reason: Option<String>,
}

// Session update notification types
#[derive(DeJson, Debug)]
struct SessionUpdateParams {
    #[rename(sessionId)]
    #[allow(dead_code)]
    session_id: Option<String>,
    update: Option<SessionUpdate>,
}

#[derive(DeJson, Debug)]
struct SessionUpdate {
    #[rename(sessionUpdate)]
    session_update: Option<String>,
    /// Direct content for agent_message_chunk updates
    content: Option<ChunkContent>,
    /// Message for other update types
    message: Option<MessageUpdate>,
}

#[derive(DeJson, Debug)]
struct ChunkContent {
    #[rename(type)]
    #[allow(dead_code)]
    content_type: Option<String>,
    text: Option<String>,
}

#[derive(DeJson, Debug)]
struct MessageUpdate {
    #[rename(messageId)]
    #[allow(dead_code)]
    message_id: Option<String>,
    role: Option<String>,
    content: Option<Vec<ContentUpdate>>,
}

#[derive(DeJson, Debug)]
struct ContentUpdate {
    #[rename(type)]
    content_type: Option<String>,
    #[allow(dead_code)]
    index: Option<u32>,
    delta: Option<ContentDelta>,
}

#[derive(DeJson, Debug)]
struct ContentDelta {
    #[rename(type)]
    delta_type: Option<String>,
    text: Option<String>,
}

// === Child Process Communication ===

enum AcpStdIn {
    Send(String),
}

enum AcpStdOut {
    Line(String),
    Term,
}

struct AcpProcess {
    #[allow(dead_code)]
    child: Child,
    stdin_sender: Sender<AcpStdIn>,
    stdout_receiver: Receiver<AcpStdOut>,
}

impl AcpProcess {
    fn start(cmd_path: &str, arg: &str, cwd: &str) -> Result<Self, std::io::Error> {
        // If arg is "--acp", we're using the claude CLI directly
        // Otherwise, cmd_path is node and arg is the script path
        let mut child = Command::new(cmd_path)
            .arg(arg)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (stdin_sender, stdin_receiver) = mpsc::channel::<AcpStdIn>();
        let (stdout_sender, stdout_receiver) = mpsc::channel::<AcpStdOut>();

        // Stdin writer thread
        thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(msg) = stdin_receiver.recv() {
                match msg {
                    AcpStdIn::Send(line) => {
                        if stdin.write_all(line.as_bytes()).is_err() {
                            break;
                        }
                        if stdin.write_all(b"\n").is_err() {
                            break;
                        }
                        let _ = stdin.flush();
                    }
                }
            }
        });

        // Stdout reader thread
        let stdout_sender_clone = stdout_sender.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stdout_sender_clone.send(AcpStdOut::Line(line)).is_err() {
                            break;
                        }
                        SignalToUI::set_ui_signal();
                    }
                    Err(_) => break,
                }
            }
            let _ = stdout_sender_clone.send(AcpStdOut::Term);
            SignalToUI::set_ui_signal();
        });

        // Stderr reader thread (for debugging)
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    // Only log stderr if it looks like an error
                    if line.contains("error") || line.contains("Error") {
                        log!("ACP stderr: {}", line);
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin_sender,
            stdout_receiver,
        })
    }

    fn send(&self, msg: &str) {
        let _ = self.stdin_sender.send(AcpStdIn::Send(msg.to_string()));
    }

    fn try_recv(&self) -> Option<String> {
        match self.stdout_receiver.try_recv() {
            Ok(AcpStdOut::Line(line)) => Some(line),
            Ok(AcpStdOut::Term) => None,
            Err(_) => None,
        }
    }
}

// === ACP Session State ===

#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Initializing,
    WaitingForSession,
    Ready,
    Prompting,
    Error,
}

struct AcpSession {
    /// Our session ID (used by the Agent trait)
    #[allow(dead_code)]
    id: SessionId,
    /// ACP's session ID (string from the server)
    acp_session_id: Option<String>,
    /// Current state
    state: SessionState,
    /// Error message if in error state
    error: Option<String>,
    /// Current prompt being processed
    current_prompt: Option<PromptId>,
    /// Accumulated text for current response
    accumulated_text: String,
    /// Pending prompt text (if sent before ready)
    pending_prompt: Option<(PromptId, String)>,
}

// === Claude ACP Agent ===

pub struct ClaudeAcpAgent {
    process: Option<AcpProcess>,
    sessions: HashMap<LiveId, AcpSession>,
    next_rpc_id: u64,
    /// Maps RPC IDs to (SessionId, PromptId) for tracking responses
    pending_rpcs: HashMap<u64, (SessionId, Option<PromptId>)>,
    cwd: String,
}

impl ClaudeAcpAgent {
    pub fn new() -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());

        Self {
            process: None,
            sessions: HashMap::new(),
            next_rpc_id: 1,
            pending_rpcs: HashMap::new(),
            cwd,
        }
    }

    /// Check if ACP is available
    pub fn is_available() -> bool {
        Self::get_acp_paths().is_some()
    }

    /// Find Claude Code ACP paths via Zed external agents installation
    fn get_acp_paths() -> Option<(String, String)> {
        Self::find_claude_zed()
    }

    /// Find Claude Code ACP in Zed external agents (macOS)
    fn find_claude_zed() -> Option<(String, String)> {
        let node_path = which_node()?;
        let home = std::env::var("HOME").ok()?;

        let acp_base = format!(
            "{}/Library/Application Support/Zed/external_agents/claude-code-acp",
            home
        );

        let entries = std::fs::read_dir(&acp_base).ok()?;
        let mut versions: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                n.chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
            })
            .collect();
        versions.sort();
        let version = versions.pop()?;

        let script_path = format!(
            "{}/{}/node_modules/@zed-industries/claude-code-acp/dist/index.js",
            acp_base, version
        );

        if std::path::Path::new(&script_path).exists() {
            Some((node_path, script_path))
        } else {
            None
        }
    }

    fn ensure_process(&mut self, cwd: &str) -> Result<(), String> {
        if self.process.is_some() {
            return Ok(());
        }

        let (node_path, script_path) =
            Self::get_acp_paths().ok_or("Could not find Claude Code ACP installation")?;

        match AcpProcess::start(&node_path, &script_path, cwd) {
            Ok(process) => {
                self.process = Some(process);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_rpc_id;
        self.next_rpc_id += 1;
        id
    }

    fn send_rpc(&mut self, method: &str, params: impl SerJson) -> u64 {
        let id = self.next_id();
        let params_json = params.serialize_json();

        let request = format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"{}","params":{}}}"#,
            id, method, params_json
        );

        if let Some(process) = &self.process {
            process.send(&request);
        }

        id
    }

    fn send_initialize(&mut self, session_id: SessionId) {
        let params = InitializeRequest {
            protocol_version: 1,
            client_info: ClientInfo {
                name: "makepad-agent".to_string(),
                version: "0.1.0".to_string(),
            },
            client_capabilities: ClientCapabilities {
                fs: Some(FsCapability {
                    read_text_file: false,
                    write_text_file: false,
                }),
                terminal: Some(false),
            },
        };
        let rpc_id = self.send_rpc("initialize", params);
        self.pending_rpcs.insert(rpc_id, (session_id, None));
    }

    fn send_new_session(&mut self, session_id: SessionId, cwd: &str) {
        let params = NewSessionRequest {
            cwd: cwd.to_string(),
            mcp_servers: vec![],
        };
        let rpc_id = self.send_rpc("session/new", params);
        self.pending_rpcs.insert(rpc_id, (session_id, None));
    }

    fn send_prompt_rpc(
        &mut self,
        session_id: SessionId,
        prompt_id: PromptId,
        acp_session_id: &str,
        text: &str,
    ) {
        let params = PromptRequest {
            session_id: acp_session_id.to_string(),
            prompt: vec![PromptContent {
                content_type: "text".to_string(),
                text: Some(text.to_string()),
            }],
        };

        let rpc_id = self.send_rpc("session/prompt", params);
        self.pending_rpcs
            .insert(rpc_id, (session_id, Some(prompt_id)));
    }

    fn handle_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let mut events = vec![];

        // Try to parse as response first
        if let Ok(response) = JsonRpcResponse::deserialize_json(line) {
            if response.id.is_some() {
                events.extend(self.handle_response(response));
                return events;
            }
        }

        // Try to parse as notification
        if let Ok(notification) = JsonRpcNotification::deserialize_json(line) {
            events.extend(self.handle_notification(notification));
        }

        events
    }

    fn handle_response(&mut self, response: JsonRpcResponse) -> Vec<AgentEvent> {
        let mut events = vec![];

        let rpc_id = match response.id {
            Some(id) => id,
            None => return events,
        };

        let (session_id, prompt_id) = match self.pending_rpcs.remove(&rpc_id) {
            Some(ids) => ids,
            None => return events,
        };

        let session = match self.sessions.get_mut(&session_id.0) {
            Some(s) => s,
            None => return events,
        };

        // Handle errors
        if let Some(error) = response.error {
            if error.code == -32001 {
                // Auth required
                session.state = SessionState::Error;
                session.error =
                    Some("Authentication required. Run 'claude /login' first.".to_string());
                events.push(AgentEvent::SessionError {
                    session_id,
                    error: session.error.clone().unwrap(),
                });
            } else if let Some(prompt_id) = prompt_id {
                events.push(AgentEvent::PromptError {
                    prompt_id,
                    error: error.message,
                });
                session.state = SessionState::Ready;
                session.current_prompt = None;
            }
            return events;
        }

        // Handle success based on current state
        match session.state {
            SessionState::Initializing => {
                if let Some(result) = &response.result {
                    let result_str = result.serialize_json();
                    match InitializeResponse::deserialize_json(&result_str) {
                        Ok(_) => {
                            session.state = SessionState::WaitingForSession;
                            let cwd = self.cwd.clone();
                            self.send_new_session(session_id, &cwd);
                        }
                        Err(e) => {
                            log!("ACP initialize parse error: {:?}", e);
                            session.state = SessionState::Error;
                            session.error = Some(format!("Initialize parse error: {:?}", e));
                            events.push(AgentEvent::SessionError {
                                session_id,
                                error: session.error.clone().unwrap(),
                            });
                        }
                    }
                }
            }
            SessionState::WaitingForSession => {
                if let Some(result) = &response.result {
                    let result_str = result.serialize_json();
                    match NewSessionResponse::deserialize_json(&result_str) {
                        Ok(resp) => {
                            session.acp_session_id = Some(resp.session_id);
                            session.state = SessionState::Ready;
                            events.push(AgentEvent::SessionReady { session_id });

                            // Send pending prompt if any
                            if let Some((prompt_id, text)) = session.pending_prompt.take() {
                                if let Some(acp_sid) = &session.acp_session_id {
                                    let acp_sid = acp_sid.clone();
                                    session.state = SessionState::Prompting;
                                    session.current_prompt = Some(prompt_id);
                                    self.send_prompt_rpc(session_id, prompt_id, &acp_sid, &text);
                                }
                            }
                        }
                        Err(e) => {
                            log!("ACP session parse error: {:?}", e);
                            session.state = SessionState::Error;
                            session.error = Some(format!("Session parse error: {:?}", e));
                            events.push(AgentEvent::SessionError {
                                session_id,
                                error: session.error.clone().unwrap(),
                            });
                        }
                    }
                }
            }
            SessionState::Prompting => {
                if let Some(result) = &response.result {
                    let result_str = result.serialize_json();
                    if let Ok(resp) = PromptResponse::deserialize_json(&result_str) {
                        if let Some(prompt_id) = session.current_prompt.take() {
                            let stop_reason = match resp.stop_reason.as_deref() {
                                Some("end_turn") => StopReason::EndTurn,
                                Some("max_tokens") => StopReason::MaxTokens,
                                Some("tool_use") => StopReason::ToolUse,
                                _ => StopReason::EndTurn,
                            };
                            events.push(AgentEvent::TurnComplete {
                                prompt_id,
                                stop_reason,
                            });
                        }

                        session.state = SessionState::Ready;
                        session.accumulated_text.clear();
                    }
                }
            }
            _ => {}
        }

        events
    }

    fn handle_notification(&mut self, notification: JsonRpcNotification) -> Vec<AgentEvent> {
        let mut events = vec![];

        if notification.method == "session/update" {
            if let Some(params_json) = notification.params {
                let params_str = params_json.serialize_json();
                if let Ok(params) = SessionUpdateParams::deserialize_json(&params_str) {
                    // Find session by ACP session ID
                    let acp_sid = params.session_id.as_deref();
                    let session_entry = self
                        .sessions
                        .iter_mut()
                        .find(|(_, s)| s.acp_session_id.as_deref() == acp_sid);

                    if let Some((_, session)) = session_entry {
                        if let Some(update) = params.update {
                            // Handle agent_message_chunk updates (streaming text)
                            if update.session_update.as_deref() == Some("agent_message_chunk") {
                                if let Some(content) = update.content {
                                    if let Some(text) = content.text {
                                        if !text.is_empty() {
                                            session.accumulated_text.push_str(&text);
                                            if let Some(prompt_id) = session.current_prompt {
                                                events.push(AgentEvent::TextDelta {
                                                    prompt_id,
                                                    text,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            // Handle other message-based updates (legacy format)
                            else if let Some(message) = update.message {
                                if message.role.as_deref() == Some("assistant") {
                                    if let Some(content) = message.content {
                                        for item in content {
                                            if item.content_type.as_deref() == Some("text") {
                                                if let Some(delta) = item.delta {
                                                    if delta.delta_type.as_deref() == Some("text") {
                                                        if let Some(text) = delta.text {
                                                            session
                                                                .accumulated_text
                                                                .push_str(&text);
                                                            if let Some(prompt_id) =
                                                                session.current_prompt
                                                            {
                                                                events.push(
                                                                    AgentEvent::TextDelta {
                                                                        prompt_id,
                                                                        text,
                                                                    },
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        events
    }
}

impl Default for ClaudeAcpAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent for ClaudeAcpAgent {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();
        let cwd = config.cwd.unwrap_or_else(|| self.cwd.clone());

        // Start process if needed
        if let Err(e) = self.ensure_process(&cwd) {
            log!("ACP error: Failed to start process: {}", e);
            let session = AcpSession {
                id: session_id,
                acp_session_id: None,
                state: SessionState::Error,
                error: Some(e),
                current_prompt: None,
                accumulated_text: String::new(),
                pending_prompt: None,
            };
            self.sessions.insert(session_id.0, session);
            return session_id;
        }

        let session = AcpSession {
            id: session_id,
            acp_session_id: None,
            state: SessionState::Initializing,
            error: None,
            current_prompt: None,
            accumulated_text: String::new(),
            pending_prompt: None,
        };
        self.sessions.insert(session_id.0, session);

        // Start initialization
        self.send_initialize(session_id);

        session_id
    }

    fn send_prompt(&mut self, _cx: &mut Cx, session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();

        let session = match self.sessions.get_mut(&session_id.0) {
            Some(s) => s,
            None => return prompt_id,
        };

        match session.state {
            SessionState::Ready => {
                if let Some(acp_sid) = session.acp_session_id.clone() {
                    session.state = SessionState::Prompting;
                    session.current_prompt = Some(prompt_id);
                    session.accumulated_text.clear();
                    self.send_prompt_rpc(session_id, prompt_id, &acp_sid, text);
                }
            }
            SessionState::Initializing | SessionState::WaitingForSession => {
                // Queue the prompt

                session.pending_prompt = Some((prompt_id, text.to_string()));
            }
            _ => {}
        }

        prompt_id
    }

    fn send_tool_result(
        &mut self,
        _cx: &mut Cx,
        _session_id: SessionId,
        _tool_use_id: &str,
        _result: &str,
        _is_error: bool,
    ) {
        // TODO: Implement tool result sending
        // TODO: Implement tool result sending
    }

    fn cancel_prompt(&mut self, _cx: &mut Cx, prompt_id: PromptId) {
        // Find session with this prompt
        for session in self.sessions.values_mut() {
            if session.current_prompt == Some(prompt_id) {
                session.current_prompt = None;
                session.accumulated_text.clear();
                if session.state == SessionState::Prompting {
                    session.state = SessionState::Ready;
                }
                // TODO: Send cancel notification to ACP
                break;
            }
        }
    }

    fn handle_event(&mut self, _cx: &mut Cx, event: &Event) -> Vec<AgentEvent> {
        let mut all_events = vec![];

        if let Event::Signal = event {
            let mut lines = Vec::new();
            if let Some(process) = &self.process {
                while let Some(line) = process.try_recv() {
                    lines.push(line);
                }
            }
            for line in lines {
                all_events.extend(self.handle_line(&line));
            }
        }

        all_events
    }

    fn is_session_ready(&self, session_id: SessionId) -> bool {
        self.sessions
            .get(&session_id.0)
            .map(|s| s.state == SessionState::Ready)
            .unwrap_or(false)
    }
}

fn which_node() -> Option<String> {
    let candidates = [
        "/usr/local/bin/node",
        "/opt/homebrew/bin/node",
        "/usr/bin/node",
    ];

    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Some(path.to_string());
        }
    }

    std::process::Command::new("which")
        .arg("node")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
