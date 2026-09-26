//! An app's tools, offered to Claude Desktop.
//!
//! While "Claude Desktop" is the F10 panel's provider, the panel holds an
//! [`McpHost`]: the loopback MCP server ([`super::server`]) over the
//! panel's own registry — every connected service's tools, named the way a
//! native tool API sees them (`route__plan`) — plus the discovery file the
//! app's `--mcp` relay (`makepad_platform::mcp_relay`) finds it by.
//!
//! A `tools/call` arrives on a server worker thread. It is queued here and
//! the UI is woken; the panel hands each queued call to its engine
//! (`EngineCore::call_external`), which shows it as a card, holds a
//! destructive one for the person's confirm, and sends the result back
//! through the call's channel when the card lands. The worker waits for
//! that result and answers the HTTP request with it.

use super::server::{tools_json, LaneCaller, McpServer, Outcome, TokenStore, ToolDef, ToolDispatcher, Value};
use crate::engine::ServiceRegistry;
use crate::wire::{api_name, split_name, truncate_to_char_boundary, Risk, ToolResult, MAX_RESULT_BYTES};
use makepad_platform::mcp_relay::{self, McpDiscovery};
use makepad_platform::makepad_micro_serde::SerJson;
use makepad_platform::thread::SignalToUI;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a worker waits for a call's result. Longer than the engine's
/// own hard cap on a running call, so the engine's answer (done, timed
/// out) is what the client normally gets; this bounds a call left waiting
/// for a confirm nobody gives.
const CALL_WAIT: Duration = Duration::from_secs(15 * 60);
/// Tool names the panel remembers for its status line.
const RECENT_CALLS: usize = 3;

/// One call Claude Desktop made, waiting for the engine on the UI thread.
pub struct McpCall {
    /// `service__tool`.
    pub name: String,
    /// The arguments object as JSON text.
    pub args: String,
    pub reply: Sender<ToolResult>,
}

/// What the panel says about the connection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct McpActivity {
    /// Requests that reached the tools (lists and calls): Claude Desktop
    /// has connected once this is not zero.
    pub requests: u64,
    pub calls: u64,
    /// The latest tool names called, oldest first.
    pub recent: Vec<String>,
}

struct Shared {
    registry: ServiceRegistry,
    app_id: String,
    title: Mutex<String>,
    calls: Mutex<VecDeque<McpCall>>,
    activity: Mutex<McpActivity>,
}

impl Shared {
    fn tools(&self) -> Vec<ToolDef> {
        self.registry
            .tool_definitions()
            .into_iter()
            .map(|def| {
                let (service, tool) = split_name(&def.name);
                ToolDef::new(api_name(service, tool), def.description, def.parameters, Risk::Read)
            })
            .collect()
    }
}

struct Dispatcher(Arc<Shared>);

impl ToolDispatcher for Dispatcher {
    fn server_name(&self) -> String {
        self.0.app_id.clone()
    }

    fn instructions(&self) -> Option<String> {
        let title = self.0.title.lock().unwrap().clone();
        Some(format!(
            "The tools of {title}, a Makepad app on this computer. Each call shows in the app's AI panel (F10); \
             a call that deletes or changes things outside the app waits there for the person to confirm it."
        ))
    }

    fn list(&self, _caller: &LaneCaller) -> Vec<ToolDef> {
        self.0.activity.lock().unwrap().requests += 1;
        SignalToUI::set_ui_signal();
        let tools = self.0.tools();
        // The relay answers `tools/list` from this while the app is not
        // running; a failed write only costs that.
        let _ = mcp_relay::write_private(
            &mcp_relay::tools_cache_path(&self.0.app_id),
            tools_json(&tools).to_json().as_bytes(),
        );
        tools
    }

    fn call(&self, _caller: &LaneCaller, name: &str, args: &Value, _request_id: &str) -> Outcome {
        {
            let mut activity = self.0.activity.lock().unwrap();
            activity.requests += 1;
            activity.calls += 1;
            activity.recent.push(name.to_string());
            if activity.recent.len() > RECENT_CALLS {
                activity.recent.remove(0);
            }
        }
        let (reply, answer) = channel();
        self.0.calls.lock().unwrap().push_back(McpCall { name: name.to_string(), args: args.to_json(), reply });
        SignalToUI::set_ui_signal();
        match answer.recv_timeout(CALL_WAIT) {
            Ok(result) => {
                let body = if result.text.trim().is_empty() { result.note.clone() } else { result.text.clone() };
                let mut text = if result.outcome.is_ok() { body } else { format!("[{}] {body}", result.outcome.slug()) };
                truncate_to_char_boundary(&mut text, MAX_RESULT_BYTES);
                Outcome { text, is_error: !result.outcome.is_ok() }
            }
            Err(RecvTimeoutError::Timeout) => Outcome { text: "the app did not answer in time".into(), is_error: true },
            Err(RecvTimeoutError::Disconnected) => {
                Outcome { text: "the app closed its Claude Desktop connection".into(), is_error: true }
            }
        }
    }
}

/// The app's endpoint for Claude Desktop. Dropping it stops the server and
/// removes the discovery file.
pub struct McpHost {
    shared: Arc<Shared>,
    discovery: McpDiscovery,
    discovery_path: PathBuf,
    // Stopped after `drop` has removed the file and answered the queue.
    _server: McpServer,
}

impl McpHost {
    /// Serve `registry`'s tools on an ephemeral loopback port with a fresh
    /// token, and write the discovery file.
    pub fn start(registry: ServiceRegistry, title: &str) -> Result<McpHost, String> {
        let app_id = mcp_relay::app_id();
        let tokens = Arc::new(TokenStore::ephemeral());
        let token = tokens.mint("claude-desktop", "claude-desktop")?;
        let shared = Arc::new(Shared {
            registry,
            app_id: app_id.clone(),
            title: Mutex::new(title.to_string()),
            calls: Mutex::new(VecDeque::new()),
            activity: Mutex::new(McpActivity::default()),
        });
        let server = McpServer::start(tokens, Arc::new(Dispatcher(shared.clone())))?;
        let host = McpHost {
            discovery: McpDiscovery {
                pid: std::process::id(),
                port: server.port(),
                token: token.as_str().to_string(),
                title: title.to_string(),
            },
            discovery_path: mcp_relay::discovery_path(&app_id),
            shared,
            _server: server,
        };
        host.write_discovery()?;
        Ok(host)
    }

    fn write_discovery(&self) -> Result<(), String> {
        mcp_relay::write_private(&self.discovery_path, self.discovery.serialize_json().as_bytes())
    }

    /// The calls waiting for the engine, in arrival order.
    pub fn take_calls(&self) -> Vec<McpCall> {
        self.shared.calls.lock().unwrap().drain(..).collect()
    }

    pub fn activity(&self) -> McpActivity {
        self.shared.activity.lock().unwrap().clone()
    }

    pub fn app_id(&self) -> &str {
        &self.shared.app_id
    }

    pub fn title(&self) -> String {
        self.shared.title.lock().unwrap().clone()
    }

    /// The app's name as Claude Desktop shows it; the discovery file
    /// follows a change.
    pub fn set_title(&mut self, title: &str) -> Result<(), String> {
        if self.discovery.title == title {
            return Ok(());
        }
        *self.shared.title.lock().unwrap() = title.to_string();
        self.discovery.title = title.to_string();
        self.write_discovery()
    }

    /// The tools as Claude Desktop sees them.
    pub fn tools(&self) -> Vec<ToolDef> {
        self.shared.tools()
    }
}

impl Drop for McpHost {
    fn drop(&mut self) {
        // Calls the engine never took: their workers are answered now (a
        // closed channel), so the server's threads can stop.
        self.shared.calls.lock().unwrap().clear();
        // Only our own file: another instance of the app may have taken the
        // name since.
        let ours = std::fs::read_to_string(&self.discovery_path)
            .map(|text| text == self.discovery.serialize_json())
            .unwrap_or(false);
        if ours {
            let _ = std::fs::remove_file(&self.discovery_path);
        }
    }
}
