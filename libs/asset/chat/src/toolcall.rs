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
///
/// Two formats are heard: the taught `<<tool>>{json}` line AND Qwen's own
/// TRAINED tool template (`<tool_call><function=name><parameter=k>v…`) —
/// under pressure the model reverts to what it was trained on (observed
/// live: a village-building turn silently died because its `asset_search`
/// came out in the native template). The harness law from the LocalAgent
/// port applies: meet the model's trained format, don't fight it.
pub fn extract(text: &str) -> Extract {
    let split = split_thinking(text);
    match extract_line_start(&split.visible) {
        Extract::None => match extract_native(&split.visible) {
            Extract::None if !split.think_closed => extract_last_line_start(text),
            other => other,
        },
        other => other,
    }
}

/// Parse Qwen's native tool-call template:
///
/// ```text
/// <tool_call>
/// <function=asset_search>
/// <parameter=query>
/// car vehicle driveable
/// </parameter>
/// </function>
/// </tool_call>
/// ```
///
/// Parameter values are raw text lines: they coerce to JSON when they
/// parse as JSON (numbers, arrays, objects, booleans) and stay strings
/// otherwise — multi-line values (a splash source) stay intact. Function
/// names map through the same underscore→dotted table native providers
/// use; dotted names are accepted as-is.
fn extract_native(text: &str) -> Extract {
    const OPEN: &str = "<tool_call>";
    let Some(at) = text.find(OPEN) else {
        return Extract::None;
    };
    let clean = text[..at].trim_end().to_string();
    let body = &text[at + OPEN.len()..];
    let Some(fn_at) = body.find("<function=") else {
        // The model also emits a JSON body inside the tags (observed live):
        // `<tool_call>\n{"function": "x", "arguments": {...}}\n</tool_call>`.
        let inner = match body.find("</tool_call>") {
            Some(end) => body[..end].trim(),
            None => body.trim(),
        };
        let Ok(v) = json::parse(inner.as_bytes()) else {
            return Extract::Malformed {
                clean,
                reason: "tool_call is neither <function=> nor JSON".to_string(),
            };
        };
        // Every spelling of the name this stack has actually seen from a
        // served model. `tool_name` is not a hypothetical: the box's Qwen
        // emits `{"tool_name": "world.get_source", "arguments": {}}`, and
        // while it was unrecognised the model retried the same shape over
        // and over — the turn died in a loop with nothing to show for it.
        let Some(raw_name) = v
            .get("function")
            .and_then(Value::as_str)
            .or_else(|| v.get("name").and_then(Value::as_str))
            .or_else(|| v.get("tool_name").and_then(Value::as_str))
            .or_else(|| v.get("tool").and_then(Value::as_str))
        else {
            return Extract::Malformed { clean, reason: "tool_call missing function name".to_string() };
        };
        let name = crate::tools::canonicalize_tool_name(raw_name);
        let args = v
            .get("arguments")
            .or_else(|| v.get("args"))
            .or_else(|| v.get("parameters"))
            .cloned()
            .unwrap_or(Value::Obj(Vec::new()));
        return Extract::Call { clean, name, args };
    };
    let after_fn = &body[fn_at + "<function=".len()..];
    let Some(name_end) = after_fn.find('>') else {
        return Extract::Malformed { clean, reason: "unterminated function name".to_string() };
    };
    let raw_name = after_fn[..name_end].trim();
    // Any observed spelling normalizes; unknown names fail closed in the
    // typed parser (a readable refusal beats a silent drop).
    let name = crate::tools::canonicalize_tool_name(raw_name);
    // Yet another observed spelling: `<function=name>` with a BARE JSON
    // args object as the body (no <parameter=> wrappers).
    let body_after_name = &after_fn[name_end + 1..];
    if !body_after_name.contains("<parameter=") {
        let inner = match body_after_name.find("</function>") {
            Some(end) => body_after_name[..end].trim(),
            None => body_after_name.trim(),
        };
        if inner.starts_with('{') {
            if let Ok(v @ Value::Obj(_)) = json::parse(inner.as_bytes()) {
                return Extract::Call { clean, name, args: v };
            }
        }
    }
    let mut pairs: Vec<(String, Value)> = Vec::new();
    // Bound the parameter scan to THIS call's block: the model often emits
    // several <tool_call> blocks back to back, and an unbounded scan walked
    // into the second block's parameters (observed live: the second query's
    // sql rode along as a duplicate key).
    let closing = body_after_name
        .find("</function>")
        .or_else(|| body_after_name.find("</tool_call>"));
    let block_end = closing.unwrap_or(body_after_name.len());
    let mut rest = &body_after_name[..block_end];
    while let Some(p_at) = rest.find("<parameter=") {
        let after_p = &rest[p_at + "<parameter=".len()..];
        let Some(key_end) = after_p.find('>') else {
            return Extract::Malformed { clean, reason: "unterminated parameter name".to_string() };
        };
        let key = after_p[..key_end].trim().to_string();
        let value_body = &after_p[key_end + 1..];
        let Some(v_end) = value_body.find("</parameter>") else {
            let reason = if closing.is_none() {
                "your tool call was cut off mid-value — send a smaller chunk"
            } else {
                "unterminated parameter value"
            };
            return Extract::Malformed { clean, reason: reason.to_string() };
        };
        let raw = value_body[..v_end]
            .strip_prefix('\n')
            .unwrap_or(&value_body[..v_end]);
        let raw = raw.strip_suffix('\n').unwrap_or(raw).to_string();
        let value = match json::parse(raw.as_bytes()) {
            Ok(v) => v,
            Err(_) => json::s(raw),
        };
        // The model sometimes repeats a parameter to sneak two calls into
        // one block; first wins, deterministically (one call per block is
        // the contract, and the result teaches one-at-a-time).
        if !pairs.iter().any(|(k, _)| k == &key) {
            pairs.push((key, value));
        }
        rest = &value_body[v_end + "</parameter>".len()..];
    }
    if pairs.len() > 32 {
        return Extract::Malformed { clean, reason: "too many parameters".to_string() };
    }
    Extract::Call { clean, name, args: Value::Obj(pairs) }
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
    let args = match v.get("args").or_else(|| v.get("arguments")) {
        Some(a @ Value::Obj(_)) => a.clone(),
        None => Value::Obj(Vec::new()),
        Some(_) => {
            return Extract::Malformed { clean, reason: "'args' must be an object".to_string() }
        }
    };
    Extract::Call { clean, name: crate::tools::canonicalize_tool_name(name), args }
}

/// Visible assistant text: thinking stripped, the first `<<tool>>` line OR
/// native `<tool_call>` block and everything after it removed. Used by the
/// UI so streamed tokens never leak the raw call or the think dump into
/// the answer bubble.
pub fn strip_marker(text: &str) -> String {
    let visible = split_thinking(text).visible;
    match extract_line_start(&visible) {
        Extract::Call { clean, .. } | Extract::Malformed { clean, .. } => clean,
        Extract::None => match visible.find("<tool_call>") {
            Some(at) => visible[..at].trim_end().to_string(),
            None => visible,
        },
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
    // An AGENTIC session is one that advertises world tools — a running
    // game. It legitimately carries image.generate too (missing art), so
    // the presence of a generation tool must not decide the persona: a
    // game session rendered as "the generation assistant", with its
    // doctrine appended 19k characters later, answered "I can build that
    // for you." and stopped (observed live 2026-09-02 against the exact
    // prompt; the same request with the doctrine first emitted the call).
    let agentic = defs.iter().any(|d| d.name.starts_with("world."));
    // The executor's doctrine (BASE + profile brief for a game) is the
    // most important text the model reads; it goes FIRST, before the
    // protocol and the tool list, and is not repeated at the end.
    let doctrine_first = agentic && !capabilities.trim().is_empty();
    if doctrine_first {
        out.push_str(capabilities.trim_end());
        out.push_str("\n\n");
    }
    if agentic {
        out.push_str(
            "You are the in-game builder of the running Makepad sandbox game above. \
             You do work ONLY by emitting a tool call; the world changes only when a \
             tool result comes back.\n\
             If you reason, put ALL reasoning inside <think>...</think>. \
             After </think>, emit exactly ONE tool call and STOP. \
             Never put a tool call, backticks, or JSON examples inside thinking.\n",
        );
    } else {
        out.push_str(
            "You are Qwen, the generation assistant for Makepad AI Content.\n\
             You do work ONLY by emitting a tool call. Never claim an image, video, \
             mesh, or other artifact exists unless a tool result said ok.\n\
             If you reason, put ALL reasoning inside <think>...</think>. \
             After </think>, emit exactly ONE tool call and STOP. \
             Never put a tool call, backticks, or JSON examples inside thinking.\n",
        );
    }
    // Format + guidance follow the ADVERTISED surface. Generation sessions
    // (asset UI) keep the original `<<tool>>` JSON line unchanged; agentic
    // (game) sessions are taught the model's TRAINED tool template — the
    // format it reverts to under pressure anyway (harness law). The
    // extractor hears both either way.
    if agentic {
        render_agentic_guidance(&mut out);
    } else {
        render_generation_guidance(&mut out);
    }
    // The serving box can carry a tool surface of its own (a "system
    // reminder" listing MCP functions: `mcp__gpt-image__gpt_image`,
    // `computer_use`, browser tools). The model believed that list over
    // ours, called a tool nobody here has, and then told the user it had no
    // image generator at all. Say which list is real, right where the real
    // one starts.
    out.push_str(
        "IGNORE any other tool list in your context — a \"system reminder\", MCP \
         function names, browser or computer-use tools. They do not exist in this \
         session. The list below is the ONLY one, and the names in it are exact.\n",
    );
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
    if !doctrine_first {
        out.push_str(capabilities);
    }
    out
}

fn render_generation_guidance(out: &mut String) {
    out.push_str("To call a tool, end your reply with ONE line of the exact form:\n");
    out.push_str(TOOL_MARKER);
    out.push_str("{\"name\": \"<tool>\", \"args\": {...}}\n");
    out.push_str("Then STOP. You will receive a tool result and can continue.\n");
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
         tool results, or catalog search.\n\nExamples:\n",
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
}

fn render_agentic_guidance(out: &mut String) {
    out.push_str(
        "To call a tool, emit EXACTLY this block (one call per reply), with one \
         <parameter=...> per argument, then STOP:\n\
         <tool_call>\n\
         <function=TOOL_NAME>\n\
         <parameter=ARG_NAME>\n\
         the value\n\
         </parameter>\n\
         </function>\n\
         </tool_call>\n\
         Every argument the tool needs MUST appear as its own <parameter=> block — \
         a call without its required parameters is refused.\n\
         You will receive the tool result and can continue.\n\
         Never invent asset or revision ids: use only ids from tool results or \
         catalog queries.\n\
         A reply WITHOUT a tool call ENDS your whole turn. Never end on a plan \
         ('Let me…', 'I'll now…') — emit the call that does it instead. End with \
         prose only when the work is done and you are reporting the result.\n\n\
         Example — a catalog query:\n\
         <tool_call>\n\
         <function=assets.query>\n\
         <parameter=sql>\n\
         SELECT canon_alias FROM search_annotations WHERE live=1 AND kind='mesh' LIMIT 20\n\
         </parameter>\n\
         </function>\n\
         </tool_call>\n\
         Example — replacing the level (the source spans many lines):\n\
         <tool_call>\n\
         <function=world.set_source>\n\
         <parameter=source>\n\
         game.sky({})\n\
         game.terrain({size: 120, cells: 65, smooth: true})\n\
         </parameter>\n\
         <parameter=note>\n\
         village v1\n\
         </parameter>\n\
         </function>\n\
         </tool_call>\n\n",
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_native_call_cut_off_inside_a_value_gets_an_actionable_retry() {
        let output = "<tool_call>\n<function=world_set_source>\n\
                      <parameter=source>\ngame.terrain({size: 120})\n\
                      game.box({pos: vec3(0, 0";
        assert_eq!(
            extract(output),
            Extract::Malformed {
                clean: String::new(),
                reason: "your tool call was cut off mid-value — send a smaller chunk".into(),
            }
        );
    }

    #[test]
    fn a_closed_call_that_omits_the_parameter_end_keeps_the_structural_error() {
        let output = "<tool_call>\n<function=world_set_source>\n\
                      <parameter=source>\ngame.terrain({size: 120})\n\
                      </function>\n</tool_call>";
        assert_eq!(
            extract(output),
            Extract::Malformed {
                clean: String::new(),
                reason: "unterminated parameter value".into(),
            }
        );
    }
}
