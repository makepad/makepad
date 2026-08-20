//! The provider-agnostic text tool-call contract.
//!
//! Neither provider path offers native structured tool calling end to end
//! (the Claude CLI's custom tools ride MCP; the fleet chat contract is
//! text), so both share one deliberately simple convention: to call a tool
//! the model emits, alone on its own line,
//!
//! ```text
//! <<tool>>{"name": "catalog_search", "args": {"query": "neon", "limit": 5}}
//! ```
//!
//! and stops. The session engine extracts the FIRST such line, treats the
//! text before it as the assistant's visible message, discards anything
//! after it (the model was told to stop), and feeds a typed result back as
//! the next turn's tool message. A malformed line is a typed refusal — the
//! model sees why and can retry — never a silent pass-through to the user.
//!
//! When a native function-calling lane appears (Qwen tool template, MCP),
//! it slots in behind [`crate::provider::ChatProvider`] without touching
//! this wire.

use crate::tools::ToolDef;
use crate::wire::{AttachmentBinding, MAX_TOOL_JSON_BYTES};
use makepad_asset_client::json::{self, Value};

/// The line marker. Chosen to be improbable in prose and trivial to scan.
pub const TOOL_MARKER: &str = "<<tool>>";

#[derive(Clone, Debug, PartialEq)]
pub enum Extract {
    /// No tool call: the whole text is the assistant's message.
    None,
    /// A well-formed call: (visible text before the marker, name, args).
    Call { clean: String, name: String, args: Value },
    /// A marker line that does not parse; the reason is model-facing.
    Malformed { clean: String, reason: String },
}

/// Qwen thinking block vs visible assistant text.
///
/// Thinking models wrap reasoning in `<think>…</think>`. Some dumps omit the
/// open tag and only emit `</think>` before the real answer. Draft tool
/// lines inside the think block are not executed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitText {
    pub thinking: String,
    pub visible: String,
    pub think_closed: bool,
}

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// Split a streamed or finished assistant turn into thinking + visible.
pub fn split_thinking(text: &str) -> SplitText {
    if let Some(open_at) = text.find(THINK_OPEN) {
        let after_open = open_at + THINK_OPEN.len();
        if let Some(rel) = text[after_open..].find(THINK_CLOSE) {
            return SplitText {
                thinking: text[after_open..after_open + rel].trim().to_string(),
                visible: text[after_open + rel + THINK_CLOSE.len()..].trim_start().to_string(),
                think_closed: true,
            };
        }
        return SplitText {
            thinking: text[after_open..].trim().to_string(),
            visible: String::new(),
            think_closed: false,
        };
    }
    if let Some(close_at) = text.find(THINK_CLOSE) {
        return SplitText {
            thinking: text[..close_at].trim().to_string(),
            visible: text[close_at + THINK_CLOSE.len()..].trim_start().to_string(),
            think_closed: true,
        };
    }
    SplitText {
        thinking: String::new(),
        visible: text.to_string(),
        think_closed: true,
    }
}

/// Scan assistant text for a real tool-call line.
///
/// Mid-line mentions (backticks, prose) are skipped. A closed think block
/// is stripped first so a draft `<<tool>>` inside reasoning cannot hide or
/// preempt the real call after `</think>`.
pub fn extract(text: &str) -> Extract {
    let split = split_thinking(text);
    match extract_line_start(&split.visible) {
        Extract::None if !split.think_closed => extract_last_line_start(text),
        other => other,
    }
}

fn extract_line_start(text: &str) -> Extract {
    extract_matching_line_start(text, false)
}

fn extract_last_line_start(text: &str) -> Extract {
    extract_matching_line_start(text, true)
}

fn extract_matching_line_start(text: &str, last: bool) -> Extract {
    let mut search = 0;
    let mut found = Extract::None;
    while let Some(rel) = text[search..].find(TOOL_MARKER) {
        let pos = search + rel;
        search = pos + TOOL_MARKER.len();
        if pos > 0 && !text[..pos].ends_with('\n') {
            continue;
        }
        let clean = text[..pos].trim_end().to_string();
        let rest = &text[pos + TOOL_MARKER.len()..];
        let line = rest.lines().next().unwrap_or("").trim();
        let parsed = parse_tool_line(clean, line);
        if !last {
            return parsed;
        }
        found = parsed;
    }
    found
}

fn parse_tool_line(clean: String, line: &str) -> Extract {
    if line.len() > MAX_TOOL_JSON_BYTES {
        return Extract::Malformed { clean, reason: "tool call too large".to_string() };
    }
    let payload = first_json_object(line).unwrap_or(line);
    let v = match json::parse(payload.as_bytes()) {
        Ok(v) => v,
        Err(e) => {
            return Extract::Malformed { clean, reason: format!("tool call is not valid JSON: {e}") }
        }
    };
    let Some(name) = v.get("name").and_then(Value::as_str) else {
        return Extract::Malformed { clean, reason: "tool call missing 'name'".to_string() };
    };
    let args = match v.get("args") {
        Some(a @ Value::Obj(_)) => a.clone(),
        None => Value::Obj(Vec::new()),
        Some(_) => {
            return Extract::Malformed { clean, reason: "'args' must be an object".to_string() }
        }
    };
    Extract::Call { clean, name: name.to_string(), args }
}

/// Visible assistant text: thinking stripped, first line-start `<<tool>>`
/// and everything after it removed. Used by the UI so streamed tokens never
/// leak the raw call or the think dump into the answer bubble.
pub fn strip_marker(text: &str) -> String {
    let visible = split_thinking(text).visible;
    match extract_line_start(&visible) {
        Extract::Call { clean, .. } | Extract::Malformed { clean, .. } => clean,
        Extract::None => visible,
    }
}

fn first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &c) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The system-prompt fragment that teaches the protocol and lists the
/// allowlisted tools. `capabilities` is the dispatcher's honest, live
/// capability text (advertised profiles, or why none are available).
pub fn render_system(defs: &[ToolDef], capabilities: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "You are Qwen, the generation assistant for Makepad AI Content.\n\
         You do work ONLY by emitting a tool call. Never claim an image, video, \
         mesh, or other artifact exists unless a tool result said ok.\n\
         If you reason, put ALL reasoning inside <think>...</think>. \
         After </think>, emit exactly ONE tool line and STOP. \
         Never put a tool line, backticks, or JSON examples inside thinking.\n\
         To call a tool, end your reply with ONE line of the exact form:\n",
    );
    out.push_str(TOOL_MARKER);
    out.push_str("{\"name\": \"<tool>\", \"args\": {...}}\n");
    out.push_str("Then STOP. You will receive a tool result and can continue.\n");
    // Guidance and examples follow the ADVERTISED surface: teaching a
    // generation-tool routing table to a session that has no generation
    // tools both misleads the model and wastes its (local, small) context.
    let generation = defs.iter().any(|d| d.name == "image.generate");
    if generation {
        out.push_str(
            "Extract a complete prompt from casual speech \
             (\"hey make me an image of a rusty trawler at dawn\" → \
             prompt \"rusty fishing trawler at dawn, misty harbor, cinematic lighting\").\n\
             Pick the tool that matches the content type:\n\
             image → image.generate; video/clip/movie → video.generate; \
             sfx/sound effect → audio.generate; spoken words → speech.generate; \
             song/music → music.generate; 3D model/GLB → mesh.generate; \
             splat/environment/world → world.generate; \
             playable character/avatar → character.generate.\n\
             Image follow-ons use image.generate then=mesh|video|world|character|matte|depth.\n\
             Generation defaults (model, width, height, steps, then) persist on this session.\n\
             When the user says change the default model/resolution/steps, call defaults.set.\n\
             When they ask what the defaults are, call defaults.get.\n\
             When they ask what models, sizes, or backends exist, call fleet.introspect.\n\
             Never invent asset or revision ids: use only ids from bound inputs, \
             tool results, or catalog search.\n\n\
             Examples:\n",
        );
        out.push_str(TOOL_MARKER);
        out.push_str(
            "{\"name\":\"image.generate\",\"args\":{\"prompt\":\"rusty fishing trawler at dawn, misty harbor\"}}\n",
        );
        out.push_str(TOOL_MARKER);
        out.push_str(
            "{\"name\":\"video.generate\",\"args\":{\"prompt\":\"trawler cutting through fog at dawn\"}}\n",
        );
        out.push_str(TOOL_MARKER);
        out.push_str(
            "{\"name\":\"audio.generate\",\"args\":{\"prompt\":\"heavy steel hatch slam\"}}\n",
        );
        out.push_str(TOOL_MARKER);
        out.push_str(
            "{\"name\":\"mesh.generate\",\"args\":{\"prompt\":\"low-poly sci-fi crate, studio lighting\"}}\n\n",
        );
    } else {
        out.push_str(
            "Never invent asset or revision ids: use only ids from tool results \
             or catalog queries.\n\nExamples:\n",
        );
        out.push_str(TOOL_MARKER);
        out.push_str(
            "{\"name\":\"assets.query\",\"args\":{\"sql\":\"SELECT canon_alias, kind FROM search_annotations WHERE live=1 AND kind='mesh' LIMIT 20\"}}\n",
        );
        out.push_str(TOOL_MARKER);
        out.push_str(
            "{\"name\":\"world.set_source\",\"args\":{\"source\":\"game.sky({})\\n...complete level...\"}}\n\n",
        );
    }
    out.push_str("Tools:\n");
    for d in defs {
        out.push_str("- ");
        out.push_str(d.name);
        out.push_str(": ");
        out.push_str(d.description);
        out.push_str(" args: ");
        out.push_str(d.args_doc);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(capabilities);
    out
}

/// The per-turn fragment naming the typed inputs the user bound (attachment
/// chips). These are the ONLY revisions transform calls may consume, plus
/// whatever earlier tool results returned.
pub fn render_attachments(attachments: &[AttachmentBinding]) -> String {
    if attachments.is_empty() {
        return String::new();
    }
    let mut out = String::from("Bound input revisions for this turn (role: revision):\n");
    for a in attachments {
        out.push_str("- ");
        out.push_str(&a.role);
        out.push_str(": ");
        out.push_str(&a.revision.to_string());
        out.push('\n');
    }
    out
}
