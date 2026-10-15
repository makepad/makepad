//! The in-process LLM engine: one local model on a thread of its own.
//!
//! A `makepad_ai_llm::LlamaSession` (pure Rust on makepad-ggml — no external
//! process, nothing over the network) generalized out of files' chat agent
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
//!
//! The engine itself (`worker`) needs the `llm` feature; the types, the
//! prompt format and the tool-call parser here do not, so a featureless
//! hub chat (machine node or LAN fleet only) shares them.

use std::{path::PathBuf, sync::Arc};

#[cfg(feature = "llm")]
mod worker;
#[cfg(feature = "llm")]
pub use worker::LocalLlmSession;
#[cfg(feature = "llm")]
pub(crate) use worker::{worker_main, WorkerMsg};

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

/// Engine limits. The defaults are the ones files shipped with.
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

// --------------------------------------------------------------- the prompt

/// The system turn: the tools block in Qwen's own template, then the caller's
/// instructions, ending ready for the first user turn.
pub fn build_prefix(system_prompt: &str, tools: &[ToolSpec]) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("<|im_start|>system\n");
    out.push_str(&system_text(system_prompt, tools));
    out.push_str("<|im_end|>\n");
    out
}

/// The system turn's TEXT — the tools block and the instructions, without
/// the template's own `<|im_start|>`/`<|im_end|>` wrapping. A node that
/// renders the template itself (the fleet chat box) takes this as its
/// `chat_system`; the in-process prefix wraps it. One rendering of the tool
/// protocol, wherever the model runs.
pub fn system_text(system_prompt: &str, tools: &[ToolSpec]) -> String {
    let mut out = String::with_capacity(4096);
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

// --------------------------------------------------------- tool-call parsing

/// The body between `<tool_call>` and `</tool_call>`: `<function=NAME>` and a
/// run of `<parameter=key>\nvalue\n</parameter>`, into a name and its
/// arguments. Values stay strings.
pub(crate) fn parse_tool_call(body: &str) -> Result<(String, Vec<(String, String)>), String> {
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
        // The template wraps a value in exactly one newline each side; the
        // value itself (indentation, blank edges) is kept byte for byte.
        let raw = &value_rest[..value_end];
        let raw = raw.strip_prefix("\r\n").or_else(|| raw.strip_prefix('\n')).unwrap_or(raw);
        let raw = raw.strip_suffix("\r\n").or_else(|| raw.strip_suffix('\n')).unwrap_or(raw);
        let value = raw.to_string();
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
    fn argument_values_keep_their_edges_exactly() {
        let body = "\n<function=edit_file>\n<parameter=old_string>\n    let x = 1;\n        y\n</parameter>\n<parameter=new_string>\n\n  z \n\n</parameter>\n</function>\n";
        let (_, args) = parse_tool_call(body).unwrap();
        assert_eq!(arg(&args, "old_string"), "    let x = 1;\n        y");
        assert_eq!(arg(&args, "new_string"), "\n  z \n");
    }

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
