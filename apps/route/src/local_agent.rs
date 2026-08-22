//! LocalAgent — the on-device dispatcher (route.md M2).
//!
//! A `makepad_ai::Agent` impl backed by an in-process `LlamaSession`
//! (Qwen3.5-9B on makepad-ggml — pure Rust, no external processes, no C++).
//! The session is `!Send`: it lives on a dedicated worker thread, built
//! there from a factory closure (the converse FilterWorker pattern), with
//! events streamed back over mpsc + `SignalToUI`.
//!
//! The session is append-only across turns — `reset()` would reload all
//! weights from disk, and appending means the system+tools prefix and the
//! whole history stay in KV cache: each turn only prefills its new suffix
//! (this *is* the prefix-KV reuse route.md asks for).
//!
//! Tool calls follow Qwen3.5's own chat template (read from the GGUF): a
//! leading system message declares `<tools>` (one JSON schema per line) and
//! the model replies with
//! `<tool_call>\n<function=name>\n<parameter=key>\nvalue\n</parameter>...`.
//! Tool results return as a user turn of `<tool_response>` blocks.

use makepad_ai::makepad_micro_serde::*;
use makepad_ai::*;
use makepad_ai_llm::{LlamaSession, LlamaSessionConfig};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub const DEFAULT_LOCAL_MODEL: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";
// 32k: the session is append-only (tool prefix + every turn accumulates) and
// the hybrid model's KV is cheap — only 12 of 48 layers are attention
// (~48KB/token → ~1.6GB at 32k); DeltaNet state is context-independent.
const MAX_CONTEXT: u32 = 32768;
const MAX_NEW_TOKENS: usize = 768;
/// Stop generating a turn when fewer tokens than this remain.
const MIN_REMAINING_CONTEXT: usize = 256;

enum WorkerMsg {
    UserTurn(String),
    /// (result, is_error) per completed tool call, in call order.
    ToolResults(Vec<(String, bool)>),
}

enum WorkerEvent {
    Ready { prefill_tokens: usize, secs: f64 },
    LoadFailed(String),
    Delta(String),
    ToolCall { name: String, args_json: String },
    TurnDone { timing: String },
    ContextFull,
}

pub struct LocalAgent {
    to_worker: Option<Sender<WorkerMsg>>,
    from_worker: Receiver<WorkerEvent>,
    cancel: Arc<AtomicBool>,
    session_id: Option<SessionId>,
    prompt_id: Option<PromptId>,
    ready: bool,
    /// Tool results the app still owes us for the current round, in order.
    awaiting_tools: usize,
    tool_results: Vec<(String, String, bool)>,
    next_tool_id: u64,
    /// Timing line of the last completed turn, shared with the app's
    /// status label (the trait object hides concrete accessors).
    timing: Arc<std::sync::Mutex<String>>,
}

impl LocalAgent {
    pub fn new(model_path: String, timing: Arc<std::sync::Mutex<String>>) -> Self {
        let (event_tx, event_rx) = channel();
        let (msg_tx, msg_rx) = channel::<WorkerMsg>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = cancel.clone();
        // The session itself is created on this thread (it is !Send).
        std::thread::spawn(move || {
            worker_main(model_path, msg_rx, event_tx, cancel_worker);
        });
        Self {
            to_worker: Some(msg_tx),
            from_worker: event_rx,
            cancel,
            session_id: None,
            prompt_id: None,
            ready: false,
            awaiting_tools: 0,
            tool_results: Vec::new(),
            next_tool_id: 0,
            timing,
        }
    }

    fn set_timing(&self, text: String) {
        if let Ok(mut timing) = self.timing.lock() {
            *timing = text;
        }
    }
}

impl Agent for LocalAgent {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();
        self.session_id = Some(session_id);
        // Ship the prompt prefix to the worker as the first "message":
        // encode as a UserTurn-free init by prepending before first turn.
        // Simplest: worker builds the prefix lazily from this one-shot cell.
        if let Some(tx) = &self.to_worker {
            let prefix = build_prompt_prefix(
                config.system_prompt.as_deref().unwrap_or(""),
                &config.tools,
            );
            // Reuse ToolResults(vec) with empty vec as "init" would be
            // obscure; send the prefix through a dedicated first UserTurn
            // marker instead: worker treats the first message specially.
            let _ = tx.send(WorkerMsg::UserTurn(format!("\u{0}PREFIX\u{0}{prefix}")));
        }
        session_id
    }

    fn send_prompt(&mut self, _cx: &mut Cx, _session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();
        self.prompt_id = Some(prompt_id);
        self.cancel.store(false, Ordering::Relaxed);
        self.awaiting_tools = 0;
        self.tool_results.clear();
        if let Some(tx) = &self.to_worker {
            let _ = tx.send(WorkerMsg::UserTurn(text.to_string()));
        }
        prompt_id
    }

    fn send_tool_result(
        &mut self,
        _cx: &mut Cx,
        _session_id: SessionId,
        tool_use_id: &str,
        result: &str,
        is_error: bool,
    ) {
        if self.awaiting_tools == 0 {
            return;
        }
        self.tool_results
            .push((tool_use_id.to_string(), result.to_string(), is_error));
        if self.tool_results.len() >= self.awaiting_tools {
            self.awaiting_tools = 0;
            let results: Vec<(String, bool)> = self
                .tool_results
                .drain(..)
                .map(|(_, result, is_error)| (result, is_error))
                .collect();
            if let Some(tx) = &self.to_worker {
                let _ = tx.send(WorkerMsg::ToolResults(results));
            }
        }
    }

    fn cancel_prompt(&mut self, _cx: &mut Cx, _prompt_id: PromptId) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let (Some(session_id), prompt_id) = (self.session_id, self.prompt_id) else {
            return out;
        };
        let prompt_id = prompt_id.unwrap_or_else(PromptId::new);
        while let Ok(event) = self.from_worker.try_recv() {
            match event {
                WorkerEvent::Ready { prefill_tokens, secs } => {
                    self.ready = true;
                    self.set_timing(format!(
                        "local model ready: {prefill_tokens} tok prefix in {secs:.1}s"
                    ));
                    out.push(AgentEvent::SessionReady { session_id });
                }
                WorkerEvent::LoadFailed(error) => {
                    out.push(AgentEvent::SessionError { session_id, error });
                }
                WorkerEvent::Delta(text) => {
                    out.push(AgentEvent::TextDelta { prompt_id, text });
                }
                WorkerEvent::ToolCall { name, args_json } => {
                    self.next_tool_id += 1;
                    self.awaiting_tools += 1;
                    out.push(AgentEvent::ToolRequest {
                        prompt_id,
                        tool_use_id: format!("local_{}", self.next_tool_id),
                        tool_name: name,
                        tool_input: args_json,
                    });
                }
                WorkerEvent::TurnDone { timing } => {
                    self.set_timing(timing);
                    // Tool round in flight: adapter semantics — no
                    // TurnComplete, the tool results drive the next round.
                    if self.awaiting_tools == 0 {
                        out.push(AgentEvent::TurnComplete {
                            prompt_id,
                            stop_reason: StopReason::EndTurn,
                        });
                    }
                }
                WorkerEvent::ContextFull => {
                    out.push(AgentEvent::PromptError {
                        prompt_id,
                        error: "local context window is full — restart the app to reset the conversation".into(),
                    });
                }
            }
        }
        out
    }

    fn is_session_ready(&self, _session_id: SessionId) -> bool {
        self.ready
    }
}

// --- prompt building --------------------------------------------------------

/// The tools system message per Qwen3.5's own chat template, plus the app
/// system prompt, ending ready for the first user turn.
fn build_prompt_prefix(system_prompt: &str, tools: &[ToolDefinition]) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("<|im_start|>system\n");
    if !tools.is_empty() {
        out.push_str("# Tools\n\nYou have access to the following functions:\n\n<tools>\n");
        for tool in tools {
            out.push_str(&format!(
                "{{\"name\":{},\"description\":{},\"parameters\":{}}}\n",
                tool.name.serialize_json(),
                tool.description.serialize_json(),
                tool.parameters
            ));
        }
        out.push_str("</tools>\n\n");
        out.push_str(
            "If you choose to call a function ONLY reply in the following format with NO suffix:\n\n\
             <tool_call>\n<function=example_function_name>\n<parameter=example_parameter_1>\n\
             value_1\n</parameter>\n</function>\n</tool_call>\n\n\
             <IMPORTANT>\nReminder:\n\
             - Function calls MUST follow the specified format: an inner <function=...></function> block must be nested within <tool_call></tool_call> XML tags\n\
             - Required parameters MUST be specified\n\
             - You may provide optional reasoning for your function call in natural language BEFORE the function call, but NOT after\n\
             - If there is no function call available, answer the question like normal\n\
             </IMPORTANT>\n\n",
        );
    }
    out.push_str(system_prompt);
    out.push_str("<|im_end|>\n");
    out
}

fn user_turn(text: &str) -> String {
    format!("<|im_start|>user\n{text}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n")
}

fn tool_response_turn(results: &[(String, bool)]) -> String {
    let mut out = String::from("<|im_start|>user\n");
    for (result, is_error) in results {
        if *is_error {
            out.push_str(&format!("<tool_response>\nERROR: {result}\n</tool_response>\n"));
        } else {
            out.push_str(&format!("<tool_response>\n{result}\n</tool_response>\n"));
        }
    }
    out.push_str("<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
    out
}

// --- worker thread ----------------------------------------------------------

fn worker_main(
    model_path: String,
    msg_rx: Receiver<WorkerMsg>,
    event_tx: Sender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
) {
    let send = |event: WorkerEvent| {
        let _ = event_tx.send(event);
        SignalToUI::set_ui_signal();
    };

    // First message is the prompt prefix (create_session).
    let prefix = match msg_rx.recv() {
        Ok(WorkerMsg::UserTurn(text)) => match text.strip_prefix("\u{0}PREFIX\u{0}") {
            Some(prefix) => prefix.to_string(),
            None => text,
        },
        _ => return,
    };

    let t0 = std::time::Instant::now();
    let mut session = match LlamaSession::load(
        &model_path,
        LlamaSessionConfig {
            max_context: Some(MAX_CONTEXT),
            ..Default::default()
        },
    ) {
        Ok(session) => session,
        Err(error) => {
            send(WorkerEvent::LoadFailed(format!(
                "cannot load {model_path}: {error:?}"
            )));
            return;
        }
    };

    let tokens = match session.vocab().tokenize(&prefix, true, true) {
        Ok(tokens) => tokens,
        Err(error) => {
            send(WorkerEvent::LoadFailed(format!("tokenize: {error:?}")));
            return;
        }
    };
    let prefill_tokens = tokens.len();
    if let Err(error) = session.append_tokens(&tokens) {
        send(WorkerEvent::LoadFailed(format!("prefill: {error:?}")));
        return;
    }
    send(WorkerEvent::Ready {
        prefill_tokens,
        secs: t0.elapsed().as_secs_f64(),
    });

    let im_end = session.vocab().token_id("<|im_end|>");
    let tool_call_open = session.vocab().token_id("<tool_call>");
    let tool_call_close = session.vocab().token_id("</tool_call>");
    let think_open = session.vocab().token_id("<think>");
    let think_close = session.vocab().token_id("</think>");

    while let Ok(msg) = msg_rx.recv() {
        let turn_text = match msg {
            WorkerMsg::UserTurn(text) => user_turn(&text),
            WorkerMsg::ToolResults(results) => tool_response_turn(&results),
        };
        let t_turn = std::time::Instant::now();
        let turn_tokens = match session.vocab().tokenize(&turn_text, true, true) {
            Ok(tokens) => tokens,
            Err(_) => continue,
        };
        if session.remaining_context() < turn_tokens.len() + MIN_REMAINING_CONTEXT {
            send(WorkerEvent::ContextFull);
            continue;
        }
        if session.append_tokens(&turn_tokens).is_err() {
            send(WorkerEvent::ContextFull);
            continue;
        }
        let prefill_secs = t_turn.elapsed().as_secs_f64();

        // Generation loop: stream text deltas; capture <tool_call> bodies;
        // swallow <think> blocks; stop on EOS/limits/cancel.
        let t_gen = std::time::Instant::now();
        let mut decoder = session.vocab().text_decoder();
        let mut generated = 0usize;
        let mut in_tool_call = false;
        let mut in_think = false;
        let mut tool_body = String::new();
        let mut tool_calls: Vec<(String, String)> = Vec::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                // Close the dangling assistant turn so the KV stays valid,
                // and drop any tool calls the cancelled turn produced — an
                // interrupted command must not keep acting.
                if let Some(im_end) = im_end {
                    let _ = session.append_token(im_end);
                }
                tool_calls.clear();
                break;
            }
            if generated >= MAX_NEW_TOKENS
                || session.remaining_context() < MIN_REMAINING_CONTEXT
            {
                if let Some(im_end) = im_end {
                    let _ = session.append_token(im_end);
                }
                break;
            }
            let token = match session.next_greedy_token() {
                Ok(Some(token)) => token,
                _ => break, // EOS (<|im_end|>) or padding — turn over
            };
            generated += 1;
            if Some(token) == tool_call_open {
                in_tool_call = true;
                tool_body.clear();
                continue;
            }
            if Some(token) == tool_call_close {
                if in_tool_call {
                    in_tool_call = false;
                    match parse_tool_call(&tool_body) {
                        Ok(call) => tool_calls.push(call),
                        Err(error) => {
                            // Malformed call: surface as a delta so the
                            // transcript shows what the model tried.
                            send(WorkerEvent::Delta(format!("[bad tool call: {error}]")));
                        }
                    }
                }
                continue;
            }
            if Some(token) == think_open {
                in_think = true;
                continue;
            }
            if Some(token) == think_close {
                in_think = false;
                continue;
            }
            if let Some(text) = decoder.push_token(session.vocab(), token) {
                if in_tool_call {
                    tool_body.push_str(&text);
                } else if !in_think {
                    send(WorkerEvent::Delta(text));
                }
            }
        }
        let gen_secs = t_gen.elapsed().as_secs_f64();
        for (name, args_json) in tool_calls.drain(..) {
            send(WorkerEvent::ToolCall { name, args_json });
        }
        send(WorkerEvent::TurnDone {
            timing: format!(
                "local: prefill {} tok {:.1}s · gen {} tok {:.1}s ({:.1} tok/s) · ctx {}/{}",
                turn_tokens.len(),
                prefill_secs,
                generated,
                gen_secs,
                generated as f64 / gen_secs.max(0.001),
                session.token_count(),
                session.max_context(),
            ),
        });
    }
}

// --- tool-call parsing ------------------------------------------------------

/// Parse the body between `<tool_call>` and `</tool_call>`:
/// `<function=NAME>` + `<parameter=key>\nvalue\n</parameter>`* — into
/// (name, JSON args). Values are coerced: numbers, bools and JSON
/// arrays/objects pass through; everything else becomes a JSON string.
fn parse_tool_call(body: &str) -> Result<(String, String), String> {
    let function_at = body.find("<function=").ok_or("missing <function=")?;
    let rest = &body[function_at + "<function=".len()..];
    let name_end = rest.find(['>', '\n']).ok_or("unterminated function name")?;
    let name = rest[..name_end].trim().to_string();
    if name.is_empty() {
        return Err("empty function name".into());
    }
    let mut args = String::from("{");
    let mut first = true;
    let mut cursor = &rest[name_end..];
    while let Some(param_at) = cursor.find("<parameter=") {
        let param_rest = &cursor[param_at + "<parameter=".len()..];
        let Some(key_end) = param_rest.find('>') else {
            break;
        };
        let key = param_rest[..key_end].trim();
        let value_rest = &param_rest[key_end + 1..];
        let value_end = value_rest.find("</parameter>").unwrap_or(value_rest.len());
        let value = value_rest[..value_end]
            .trim_matches('\n')
            .trim()
            .to_string();
        if !first {
            args.push(',');
        }
        first = false;
        args.push_str(&format!(
            "{}:{}",
            serde_json::to_string(key).unwrap_or_default(),
            coerce_json_value(&value)
        ));
        cursor = &value_rest[value_end..];
    }
    args.push('}');
    Ok((name, args))
}

/// Raw parameter text → JSON value text.
fn coerce_json_value(value: &str) -> String {
    if value == "true" || value == "false" || value == "null" {
        return value.to_string();
    }
    if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        return value.to_string();
    }
    if (value.starts_with('[') && value.ends_with(']'))
        || (value.starts_with('{') && value.ends_with('}'))
    {
        if JsonValue::deserialize_json(value).is_ok() {
            return value.to_string();
        }
    }
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_tool_call() {
        let body = "\n<function=route_plan>\n<parameter=to>\nutrecht\n</parameter>\n<parameter=mode>\ncar\n</parameter>\n</function>\n";
        let (name, args) = parse_tool_call(body).unwrap();
        assert_eq!(name, "route_plan");
        assert_eq!(args, r#"{"to":"utrecht","mode":"car"}"#);
    }

    #[test]
    fn coerces_types() {
        let body = "<function=map_fly_to>\n<parameter=lon>\n4.89\n</parameter>\n<parameter=lat>\n52.37\n</parameter>\n<parameter=zoom>\n14\n</parameter>\n</function>";
        let (name, args) = parse_tool_call(body).unwrap();
        assert_eq!(name, "map_fly_to");
        assert_eq!(args, r#"{"lon":4.89,"lat":52.37,"zoom":14}"#);
    }

    #[test]
    fn coerces_arrays_and_bools() {
        let body = "<function=route_along>\n<parameter=kinds>\n[\"charger\", \"museum\"]\n</parameter>\n<parameter=on>\ntrue\n</parameter>\n</function>";
        let (_, args) = parse_tool_call(body).unwrap();
        assert_eq!(args, r#"{"kinds":["charger", "museum"],"on":true}"#);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_tool_call("no function here").is_err());
    }

    #[test]
    fn prefix_contains_tools_block() {
        let tools = vec![crate::broker::def(
            "geo_search",
            "Search places",
            r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}"#,
        )];
        let prefix = build_prompt_prefix("You are a router.", &tools);
        assert!(prefix.starts_with("<|im_start|>system\n# Tools"));
        assert!(prefix.contains("\"name\":\"geo_search\""));
        assert!(prefix.contains("<tools>\n"));
        assert!(prefix.ends_with("You are a router.<|im_end|>\n"));
    }
}
