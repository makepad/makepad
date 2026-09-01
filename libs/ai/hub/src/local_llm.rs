//! The in-process LLM engine: one local model on a thread of its own.
//!
//! A `makepad_ai_llm::LlamaSession` (pure Rust on makepad-ggml — no external
//! process, nothing over the network) generalized out of mpfiles' chat agent
//! so every app consumes the same engine through [`crate::hub::AiHub`]. The
//! session is `!Send`: it is built on, and never leaves, one dedicated worker
//! thread; the consumer talks to it over a pair of channels and is woken
//! through an injected [`WakeHook`] (a UI app passes `SignalToUI::set_ui_signal`;
//! a headless caller passes nothing and just polls).
//!
//! The session is append-only across turns. `reset()` would reload every
//! weight from disk, and appending means the system-and-tools prefix and the
//! whole conversation stay in the KV cache: each turn only prefills its own
//! suffix. This is the sticky-lane law of aicore.md §6 in its in-process form.
//!
//! Tool calls follow Qwen's own chat template: a leading system message
//! declares `<tools>` (one JSON schema per line) and the model answers with
//! `<tool_call>\n<function=name>\n<parameter=key>\nvalue\n</parameter>…`. Tool
//! results go back as a user turn of `<tool_response>` blocks. Nothing here
//! parses JSON — the wire between the parser and the tools is a list of
//! (key, value) strings, because the tool pack that reads them knows what it
//! wants, and a number that arrived as text is still a number.
//!
//! The transcript is the CONSUMER'S: this engine holds only the KV cache and
//! the running turn. Losing the engine loses warmth, never the conversation
//! (aicore.md §7).

use makepad_ai_llm::{LlamaSession, LlamaSessionConfig};

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
};

/// Called after every event so a sleeping consumer wakes up. UI apps pass
/// their platform signal; headless callers may pass `None` and poll.
pub type WakeHook = Arc<dyn Fn() + Send + Sync>;

/// One tool, as the model is told about it.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// A JSON-schema object, verbatim.
    pub parameters: String,
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters: parameters.into(),
        }
    }
}

/// Engine limits. The defaults are the ones mpfiles shipped with.
#[derive(Clone, Debug)]
pub struct LocalLlmConfig {
    /// The GGUF to load. Path policy (env overrides, checkout search) is the
    /// app's business; the engine takes the answer.
    pub model: PathBuf,
    /// Append-only window for the session's whole life.
    pub max_context: u32,
    /// Per-turn generation cap.
    pub max_new_tokens: usize,
    /// Stop a turn while this much context is still left, so the next fits.
    pub min_remaining_context: usize,
}

impl LocalLlmConfig {
    pub fn new(model: PathBuf) -> Self {
        Self {
            model,
            max_context: 16384,
            max_new_tokens: 640,
            min_remaining_context: 256,
        }
    }
}

/// What the worker has to say. Everything a chat panel shows comes from here.
#[derive(Debug)]
pub enum ChatEvent {
    /// A named phase of the load, and how far through it is (0..1).
    Loading { phase: String, fraction: f64 },
    /// The weights are resident and the prefix is prefilled.
    Ready { prefill_tokens: usize, secs: f64 },
    /// The model could not be loaded at all. Nothing else will ever arrive.
    Failed(String),
    /// A piece of the answer being written.
    Delta(String),
    /// The model wants a tool run. The consumer owes exactly one result per call.
    ToolCall {
        name: String,
        args: Vec<(String, String)>,
    },
    /// The turn is over. `tool_calls` is how many results the consumer now owes.
    TurnDone {
        tool_calls: usize,
        tokens: usize,
        secs: f64,
        context_used: usize,
        context_max: usize,
    },
    /// The window is full; this session cannot continue.
    ContextFull,
}

pub(crate) enum WorkerMsg {
    UserTurn(String),
    /// One (text, is_error) per tool the last turn asked for, in call order.
    ToolResults(Vec<(String, bool)>),
}

/// A running local-model chat: channels to a dedicated worker thread.
pub struct LocalLlmSession {
    to_worker: Sender<WorkerMsg>,
    from_worker: Receiver<ChatEvent>,
    cancel: Arc<AtomicBool>,
}

impl LocalLlmSession {
    /// Start loading the model. Nothing blocks: the load happens on the worker
    /// and reports itself through [`LocalLlmSession::poll`].
    pub fn start(config: LocalLlmConfig, prefix: String, wake: Option<WakeHook>) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        thread::Builder::new()
            .name("ai-hub-local-llm".into())
            .spawn(move || worker_main(config, prefix, msg_rx, event_tx, worker_cancel, wake, None))
            .expect("spawn local llm worker");
        Self {
            to_worker,
            from_worker,
            cancel,
        }
    }

    pub fn send_user_turn(&self, text: String) {
        self.cancel.store(false, Ordering::Relaxed);
        let _ = self.to_worker.send(WorkerMsg::UserTurn(text));
    }

    pub fn send_tool_results(&self, results: Vec<(String, bool)>) {
        let _ = self.to_worker.send(WorkerMsg::ToolResults(results));
    }

    /// Stop the turn that is running, per token. The worker closes the
    /// dangling assistant turn so the cache stays valid and drops any tool
    /// calls it had collected — an interrupted question must not keep acting.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Vec<ChatEvent> {
        self.from_worker.try_iter().collect()
    }
}

// --------------------------------------------------------------- the prompt

/// The system turn: the tools block in Qwen's own template, then the caller's
/// instructions, ending ready for the first user turn.
pub fn build_prefix(system_prompt: &str, tools: &[ToolSpec]) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("<|im_start|>system\n");
    if !tools.is_empty() {
        out.push_str("# Tools\n\nYou have access to the following functions:\n\n<tools>\n");
        for tool in tools {
            out.push_str("{\"name\":");
            push_json_string(&mut out, &tool.name);
            out.push_str(",\"description\":");
            push_json_string(&mut out, &tool.description);
            out.push_str(",\"parameters\":");
            out.push_str(&tool.parameters);
            out.push_str("}\n");
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

/// A JSON string literal, appended. The only JSON this module writes.
fn push_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn user_turn(text: &str) -> String {
    format!("<|im_start|>user\n{text}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n")
}

fn tool_response_turn(results: &[(String, bool)]) -> String {
    let mut out = String::from("<|im_start|>user\n");
    for (result, is_error) in results {
        out.push_str("<tool_response>\n");
        if *is_error {
            out.push_str("ERROR: ");
        }
        out.push_str(result);
        out.push_str("\n</tool_response>\n");
    }
    out.push_str("<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
    out
}

// --------------------------------------------------------------- the worker

pub(crate) fn worker_main(
    config: LocalLlmConfig,
    prefix: String,
    msg_rx: Receiver<WorkerMsg>,
    event_tx: Sender<ChatEvent>,
    cancel: Arc<AtomicBool>,
    wake: Option<WakeHook>,
    // The held machine election, when this worker won it (aicore §3). The
    // guard publishes load progress so co-located waiters ride one load,
    // and it drops — reopening the election — exactly when this worker ends.
    mut residency: Option<crate::machine::ResidencyGuard>,
) {
    let wake_consumer = move || {
        if let Some(wake) = &wake {
            wake();
        }
    };
    let send = |event: ChatEvent| {
        let _ = event_tx.send(event);
        wake_consumer();
    };

    let started = std::time::Instant::now();
    let load = {
        let event_tx = event_tx.clone();
        let wake_consumer = wake_consumer.clone();
        let residency = &mut residency;
        let mut progress = move |phase: &str, fraction: f64| {
            if let Some(guard) = residency.as_mut() {
                let _ = guard.publish(crate::machine::ResidencyState::Loading { fraction });
            }
            let _ = event_tx.send(ChatEvent::Loading {
                phase: phase.to_string(),
                fraction,
            });
            wake_consumer();
        };
        LlamaSession::load_with_progress(
            &config.model,
            LlamaSessionConfig {
                max_context: Some(config.max_context),
                ..Default::default()
            },
            &mut progress,
        )
    };
    let mut session = match load {
        Ok(session) => session,
        Err(error) => {
            send(ChatEvent::Failed(format!(
                "could not load {}: {error:?}",
                config.model.display()
            )));
            return;
        }
    };

    let tokens = match session.vocab().tokenize(&prefix, true, true) {
        Ok(tokens) => tokens,
        Err(error) => return send(ChatEvent::Failed(format!("tokenize: {error:?}"))),
    };
    let prefill_tokens = tokens.len();
    if let Err(error) = session.append_tokens(&tokens) {
        return send(ChatEvent::Failed(format!("prefill: {error:?}")));
    }
    if let Some(guard) = residency.as_mut() {
        // Resident but not serving a port: co-located claimants see the
        // election held and fall back per the documented soft failure.
        let _ = guard.publish(crate::machine::ResidencyState::Ready { port: 0 });
    }
    send(ChatEvent::Ready {
        prefill_tokens,
        secs: started.elapsed().as_secs_f64(),
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
        let turn_tokens = match session.vocab().tokenize(&turn_text, true, true) {
            Ok(tokens) => tokens,
            Err(_) => continue,
        };
        if session.remaining_context() < turn_tokens.len() + config.min_remaining_context
            || session.append_tokens(&turn_tokens).is_err()
        {
            send(ChatEvent::ContextFull);
            continue;
        }

        // Stream the answer; capture <tool_call> bodies; swallow <think>.
        let generating = std::time::Instant::now();
        let mut decoder = session.vocab().text_decoder();
        let mut generated = 0usize;
        let mut in_tool_call = false;
        let mut in_think = false;
        let mut tool_body = String::new();
        let mut tool_calls: Vec<(String, Vec<(String, String)>)> = Vec::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                // Close the dangling assistant turn so the cache stays valid,
                // and drop the tool calls: an interrupted question must not
                // keep looking at things.
                if let Some(im_end) = im_end {
                    let _ = session.append_token(im_end);
                }
                tool_calls.clear();
                break;
            }
            if generated >= config.max_new_tokens
                || session.remaining_context() < config.min_remaining_context
            {
                if let Some(im_end) = im_end {
                    let _ = session.append_token(im_end);
                }
                break;
            }
            let token = match session.next_greedy_token() {
                Ok(Some(token)) => token,
                // End of turn, or nothing left to say.
                _ => break,
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
                        // Malformed: show what it tried rather than hanging.
                        Err(error) => send(ChatEvent::Delta(format!("[bad tool call: {error}]"))),
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
                    send(ChatEvent::Delta(text));
                }
            }
        }
        let secs = generating.elapsed().as_secs_f64();
        let tool_call_count = tool_calls.len();
        for (name, args) in tool_calls {
            send(ChatEvent::ToolCall { name, args });
        }
        send(ChatEvent::TurnDone {
            tool_calls: tool_call_count,
            tokens: generated,
            secs,
            context_used: session.token_count(),
            context_max: session.max_context(),
        });
    }
}

// --------------------------------------------------------- tool-call parsing

/// The body between `<tool_call>` and `</tool_call>`: `<function=NAME>` and a
/// run of `<parameter=key>\nvalue\n</parameter>`, into a name and its
/// arguments. Values stay strings.
fn parse_tool_call(body: &str) -> Result<(String, Vec<(String, String)>), String> {
    let function_at = body.find("<function=").ok_or("missing <function=")?;
    let rest = &body[function_at + "<function=".len()..];
    let name_end = rest.find(['>', '\n']).ok_or("unterminated function name")?;
    let name = rest[..name_end].trim().to_string();
    if name.is_empty() {
        return Err("empty function name".into());
    }
    let mut args = Vec::new();
    let mut cursor = &rest[name_end..];
    while let Some(param_at) = cursor.find("<parameter=") {
        let param_rest = &cursor[param_at + "<parameter=".len()..];
        let Some(key_end) = param_rest.find('>') else {
            break;
        };
        let key = param_rest[..key_end].trim().to_string();
        let value_rest = &param_rest[key_end + 1..];
        let value_end = value_rest.find("</parameter>").unwrap_or(value_rest.len());
        let value = value_rest[..value_end].trim_matches('\n').trim().to_string();
        args.push((key, value));
        cursor = &value_rest[value_end..];
    }
    Ok((name, args))
}

/// One argument by name, or the empty string.
pub fn arg<'a>(args: &'a [(String, String)], key: &str) -> &'a str {
    args.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_typical_call() {
        let body =
            "\n<function=list_dir>\n<parameter=path>\n~/Documents\n</parameter>\n</function>\n";
        let (name, args) = parse_tool_call(body).unwrap();
        assert_eq!(name, "list_dir");
        assert_eq!(arg(&args, "path"), "~/Documents");
        assert_eq!(arg(&args, "missing"), "");
    }

    #[test]
    fn parses_several_parameters_in_order() {
        let body = "<function=read_file>\n<parameter=path>\n/tmp/a.txt\n</parameter>\n\
                    <parameter=max_bytes>\n4096\n</parameter>\n</function>";
        let (name, args) = parse_tool_call(body).unwrap();
        assert_eq!(name, "read_file");
        assert_eq!(
            args,
            vec![
                ("path".to_string(), "/tmp/a.txt".to_string()),
                ("max_bytes".to_string(), "4096".to_string()),
            ]
        );
    }

    #[test]
    fn rejects_a_body_with_no_function() {
        assert!(parse_tool_call("just some prose").is_err());
    }

    #[test]
    fn the_prefix_carries_the_tools_block() {
        let tools = [ToolSpec::new(
            "list_dir",
            "List a folder's entries.",
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
        )];
        let prefix = build_prefix("You are a file assistant.", &tools);
        assert!(prefix.starts_with("<|im_start|>system\n# Tools"));
        assert!(prefix.contains("\"name\":\"list_dir\""));
        assert!(prefix.contains("<tools>\n"));
        assert!(prefix.ends_with("You are a file assistant.<|im_end|>\n"));
    }

    #[test]
    fn json_strings_are_escaped() {
        let mut out = String::new();
        push_json_string(&mut out, "a \"quoted\" \\ path\nnewline");
        assert_eq!(out, "\"a \\\"quoted\\\" \\\\ path\\nnewline\"");
    }
}
