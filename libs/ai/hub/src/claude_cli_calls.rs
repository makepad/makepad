//! Tool calls between a hub client's text protocol and Claude.
//!
//! Hub clients describe tools in their system prompt and read tool calls back
//! out of the reply text in the hub's standard (Qwen-trained) markup:
//!
//! ```text
//! <tool_call>
//! <function=NAME>
//! <parameter=KEY>
//! value
//! </parameter>
//! </function>
//! </tool_call>
//! ```
//!
//! Claude (run with no tools) writes its own spellings of that, so the
//! claude-cli backend gives it ONE output contract instead ([`CONTRACT`]: a
//! JSON object inside `<tool_call>` tags), shows earlier calls of the
//! conversation in that same form ([`history_to_contract`]), and turns the
//! reply back into the standard markup ([`to_app_format`]). The parser also
//! hears the spellings Claude falls back to (`<invoke name=…>` with
//! `<parameter name=…>`, a `<function=…>` block, a fenced JSON call), so a
//! drift still arrives as a call. Nothing here runs a tool: it only rewrites
//! text the caller then parses and executes.

use makepad_strict_json::{self as json, Value};

/// Added to the system prompt when the app's prompt defines tool calls.
pub const CONTRACT: &str = "TOOL CALLS. The application's instructions below describe their own tool-call markup; \
do not write that markup. Write every tool call as one JSON object inside <tool_call> tags, exactly like this:\n\
<tool_call>\n\
{\"name\": \"TOOL_NAME\", \"arguments\": {\"ARG_NAME\": \"value\", \"OTHER_ARG\": 3}}\n\
</tool_call>\n\
Use the tool and argument names the application lists, and JSON values of the right type; a long or multi-line \
text (a script source) is one JSON string with \\n escapes. Put any prose before the first block and nothing after \
the last; the relay turns each block into the application's own format, and earlier calls in the conversation \
are shown in this same JSON form.";

/// Whether an app's system prompt defines textual tool calls.
pub fn app_has_tools(system: &str) -> bool {
    system.contains("<tool_call>")
}

/// One tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    /// In order; values from markup are strings, from JSON keep their type.
    pub args: Vec<(String, Value)>,
}

const MARKERS: [&str; 5] = ["<tool_call>", "<function_calls>", "<invoke", "<function=", "```"];

fn earliest_marker(text: &str, from: usize) -> Option<(usize, &'static str)> {
    MARKERS.iter().filter_map(|m| text[from..].find(m).map(|at| (from + at, *m))).min_by_key(|(at, _)| *at)
}

/// How much of a reply still being written can be shown: everything before
/// anything that may start a tool call (or a partial marker at the end).
/// The final, converted reply always extends this prefix.
pub fn stream_safe_len(text: &str) -> usize {
    if let Some((at, _)) = earliest_marker(text, 0) {
        return at;
    }
    for (at, c) in text.char_indices().rev().take(16) {
        if matches!(c, '<' | '`') && MARKERS.iter().any(|m| m.starts_with(&text[at..])) {
            return at;
        }
    }
    text.len()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.chars().any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '='))
}

/// The value of a markup parameter: one newline trimmed at each end.
fn markup_value(raw: &str) -> Value {
    let raw = raw.strip_prefix('\n').unwrap_or(raw);
    Value::Str(raw.strip_suffix('\n').unwrap_or(raw).to_string())
}

/// `<parameter=KEY>v</parameter>` and `<parameter name="KEY">v</parameter>`.
fn parse_params(mut s: &str) -> Option<Vec<(String, Value)>> {
    let mut args: Vec<(String, Value)> = Vec::new();
    while let Some(at) = s.find("<parameter") {
        let rest = &s[at + "<parameter".len()..];
        let (key, after) = if let Some(rest) = rest.strip_prefix('=') {
            let end = rest.find('>')?;
            (rest[..end].trim().trim_matches('"').to_string(), &rest[end + 1..])
        } else {
            let name_at = rest.find("name=\"")?;
            let rest = &rest[name_at + "name=\"".len()..];
            let end = rest.find('"')?;
            let close = rest[end..].find('>')?;
            (rest[..end].to_string(), &rest[end + close + 1..])
        };
        let end = after.find("</parameter>")?;
        if key.is_empty() {
            return None;
        }
        if !args.iter().any(|(k, _)| *k == key) {
            args.push((key, markup_value(&after[..end])));
        }
        s = &after[end + "</parameter>".len()..];
    }
    Some(args)
}

/// `<function=NAME>` params.
fn parse_function(s: &str) -> Option<Call> {
    let rest = &s[s.find("<function=")? + "<function=".len()..];
    let end = rest.find('>')?;
    let name = rest[..end].trim().trim_matches('"').to_string();
    valid_name(&name).then_some(())?;
    let body = &rest[end + 1..];
    let body = &body[..body.find("</function>").unwrap_or(body.len())];
    if !body.contains("<parameter") {
        // A bare JSON args object as the body.
        let inner = body.trim();
        if inner.starts_with('{') {
            if let Some(Value::Obj(args)) = parse_json(inner) {
                return Some(Call { name, args });
            }
        }
    }
    Some(Call { name, args: parse_params(body)? })
}

/// `<invoke name="NAME">` params, up to its end.
fn parse_invoke(s: &str) -> Option<Call> {
    let rest = &s[s.find("<invoke")? + "<invoke".len()..];
    let rest = &rest[rest.find("name=\"")? + "name=\"".len()..];
    let end = rest.find('"')?;
    let name = rest[..end].to_string();
    valid_name(&name).then_some(())?;
    let body = &rest[end..];
    let body = &body[body.find('>')? + 1..];
    Some(Call { name, args: parse_params(body)? })
}

fn parse_json(text: &str) -> Option<Value> {
    json::parse_depth(text.as_bytes(), 64).ok()
}

/// A JSON call object: `{"name", "arguments"}` and the spellings models use.
fn json_call(v: &Value) -> Option<Call> {
    let mut v = v;
    if let Some(inner @ Value::Obj(_)) = v.get("function") {
        v = inner;
    }
    let name = ["name", "function", "tool_name", "tool"].iter().find_map(|k| v.get(k).and_then(Value::as_str))?.to_string();
    valid_name(&name).then_some(())?;
    let args = match ["arguments", "args", "parameters", "input"].iter().find_map(|k| v.get(k)) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Obj(args)) => args.clone(),
        Some(Value::Str(text)) => match parse_json(text)? {
            Value::Obj(args) => args,
            _ => return None,
        },
        Some(_) => return None,
    };
    Some(Call { name, args })
}

/// JSON calls in a block body: one object, or an array of them; a code
/// fence around it is allowed.
fn json_calls(body: &str) -> Option<Vec<Call>> {
    let mut inner = body.trim();
    if let Some(rest) = inner.strip_prefix("```") {
        inner = rest.split_once('\n').map_or("", |(_, r)| r);
        inner = inner.trim_end().strip_suffix("```").unwrap_or(inner).trim();
    }
    match parse_json(inner)? {
        Value::Arr(items) => items.iter().map(json_call).collect(),
        v => Some(vec![json_call(&v)?]),
    }
}

/// Every call in a markup body.
fn body_calls(body: &str) -> Option<Vec<Call>> {
    if body.contains("<invoke") {
        let mut calls = Vec::new();
        let mut rest = body;
        while let Some(at) = rest.find("<invoke") {
            let piece = &rest[at..];
            let end = piece.find("</invoke>").map_or(piece.len(), |e| e + "</invoke>".len());
            calls.push(parse_invoke(&piece[..end])?);
            rest = &piece[end..];
        }
        Some(calls)
    } else if body.contains("<function=") {
        Some(vec![parse_function(body)?])
    } else {
        json_calls(body)
    }
}

fn block_end(text: &str, from: usize, close: &str) -> usize {
    text[from..].find(close).map_or(text.len(), |e| from + e + close.len())
}

/// The calls in a reply and where the first one starts. `None` when the
/// reply holds no call; `Err` when a call block cannot be read (the text is
/// then relayed as written, for the caller to refuse).
pub fn parse_calls(text: &str) -> Result<Option<(usize, Vec<Call>)>, ()> {
    let mut calls = Vec::new();
    // Per call: an `<invoke>` with nothing inside (Claude sometimes writes
    // such a stub before the real call).
    let mut stubs = Vec::new();
    let mut first = None;
    let mut pos = 0;
    while let Some((at, marker)) = earliest_marker(text, pos) {
        let (end, found) = match marker {
            "<tool_call>" => {
                let end = block_end(text, at, "</tool_call>");
                let body = &text[at + marker.len()..end];
                (end, body_calls(body.strip_suffix("</tool_call>").unwrap_or(body)).ok_or(())?)
            }
            "<function_calls>" => {
                let end = block_end(text, at, "</function_calls>");
                (end, body_calls(&text[at..end]).ok_or(())?)
            }
            "<invoke" => {
                let end = ["</invoke>", "</function>", "</tool_call>"]
                    .iter()
                    .filter_map(|c| text[at..].find(c).map(|e| at + e + c.len()))
                    .min()
                    .unwrap_or(text.len());
                (end, vec![parse_invoke(&text[at..end]).ok_or(())?])
            }
            "<function=" => {
                let end = block_end(text, at, "</function>");
                (end, vec![parse_function(&text[at..end]).ok_or(())?])
            }
            _ => {
                // A fence: a call only if it holds a JSON call; prose code
                // stays prose.
                let end = text[at + 3..].find("```").map_or(text.len(), |e| at + 3 + e + 3);
                match json_calls(&text[at..end]) {
                    Some(found) => (end, found),
                    None => {
                        pos = end;
                        continue;
                    }
                }
            }
        };
        if !found.is_empty() {
            let stub = matches!(marker, "<invoke" | "<function_calls>")
                && found.iter().all(|c| c.args.is_empty())
                && text[at..end].split('>').skip(1).all(|piece| piece.split('<').next().unwrap_or("").trim().is_empty());
            stubs.extend(std::iter::repeat(stub).take(found.len()));
            calls.extend(found);
        }
        if first.is_none() && !calls.is_empty() {
            first = Some(at);
        }
        // An unclosed tool_call runs to the end; the closers of a mixed
        // spelling (`</function></tool_call>`) are skipped with it.
        let mut next = end;
        for close in ["</function>", "</tool_call>", "</function_calls>"] {
            let rest = text[next..].trim_start();
            if let Some(after) = rest.strip_prefix(close) {
                next = text.len() - after.len();
            }
        }
        pos = next;
    }
    if stubs.iter().any(|stub| !stub) {
        let mut keep = stubs.iter().map(|stub| !stub);
        calls.retain(|_| keep.next().unwrap_or(true));
    }
    Ok(first.map(|at| (at, calls)))
}

fn standard_value(v: &Value) -> String {
    match v {
        Value::Str(s) if !s.starts_with('"') => s.clone(),
        other => other.to_json(),
    }
}

/// The calls in the hub's standard markup.
pub fn render_standard(calls: &[Call]) -> String {
    let mut out = String::new();
    for (i, call) in calls.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("<tool_call>\n<function=");
        out.push_str(&call.name);
        out.push_str(">\n");
        for (key, value) in &call.args {
            out.push_str("<parameter=");
            out.push_str(key);
            out.push_str(">\n");
            out.push_str(&standard_value(value));
            out.push_str("\n</parameter>\n");
        }
        out.push_str("</function>\n</tool_call>");
    }
    out
}

/// Claude's reply as the app reads it: the prose before the first call
/// exactly as written, then every call in the standard markup. A reply
/// without calls, or with one that cannot be read, is returned unchanged.
pub fn to_app_format(text: &str) -> String {
    match parse_calls(text) {
        Ok(Some((at, calls))) => format!("{}{}", &text[..at], render_standard(&calls)),
        _ => text.to_string(),
    }
}

/// A markup value back as JSON: a string that holds JSON of another type
/// (a number, an object) is shown as that.
fn contract_value(v: &Value) -> Value {
    match v {
        Value::Str(s) => match parse_json(s.trim()) {
            Some(Value::Str(_)) | None => v.clone(),
            Some(parsed) => parsed,
        },
        other => other.clone(),
    }
}

/// An earlier assistant turn with its calls in the [`CONTRACT`] form.
pub fn history_to_contract(text: &str) -> String {
    let Ok(Some((at, calls))) = parse_calls(text) else { return text.to_string() };
    let mut out = text[..at].to_string();
    for (i, call) in calls.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let args = Value::Obj(call.args.iter().map(|(k, v)| (k.clone(), contract_value(v))).collect());
        let object = json::obj(vec![("name", json::s(call.name.clone())), ("arguments", args)]);
        out.push_str("<tool_call>\n");
        out.push_str(&object.to_json());
        out.push_str("\n</tool_call>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &[(&str, Value)]) -> Call {
        Call { name: name.into(), args: args.iter().map(|(k, v)| (k.to_string(), v.clone())).collect() }
    }

    #[test]
    fn the_contract_form_becomes_the_standard_markup() {
        let reply = "Building it.\n<tool_call>\n{\"name\": \"world.api\", \"arguments\": {\"op\": \"spawn\", \"count\": 3, \"source\": \"a\\nb\"}}\n</tool_call>";
        let out = to_app_format(reply);
        assert_eq!(
            out,
            "Building it.\n<tool_call>\n<function=world.api>\n<parameter=op>\nspawn\n</parameter>\n<parameter=count>\n3\n</parameter>\n<parameter=source>\na\nb\n</parameter>\n</function>\n</tool_call>"
        );
        let (name, args) = crate::local_llm::parse_tool_call(&out["Building it.\n<tool_call>".len()..]).unwrap();
        assert_eq!(name, "world.api");
        assert_eq!(args[2], ("source".to_string(), "a\nb".to_string()));
    }

    #[test]
    fn claudes_fallback_spellings_are_heard() {
        let want = vec![call("world.api", &[("op", Value::Str("spawn".into())), ("what", Value::Str("crate".into()))])];
        for reply in [
            // The mixed form seen live.
            "<invoke name=\"world.api\">\n<parameter name=\"op\">spawn</parameter>\n<parameter name=\"what\">crate</parameter>\n</function>\n</tool_call>",
            "<function_calls>\n<invoke name=\"world.api\">\n<parameter name=\"op\">spawn</parameter>\n<parameter name=\"what\">crate</parameter>\n</invoke>\n</function_calls>",
            "<tool_call>\n<function=world.api>\n<parameter=op>\nspawn\n</parameter>\n<parameter=what>\ncrate\n</parameter>\n</function>\n</tool_call>",
            "<tool_call>\n```json\n{\"name\":\"world.api\",\"arguments\":{\"op\":\"spawn\",\"what\":\"crate\"}}\n```\n</tool_call>",
            "```json\n{\"name\":\"world.api\",\"arguments\":{\"op\":\"spawn\",\"what\":\"crate\"}}\n```",
            "<tool_call>{\"function\": {\"name\": \"world.api\", \"arguments\": \"{\\\"op\\\":\\\"spawn\\\",\\\"what\\\":\\\"crate\\\"}\"}}</tool_call>",
        ] {
            let (at, calls) = parse_calls(reply).unwrap().unwrap_or_else(|| panic!("no call in {reply}"));
            assert_eq!(at, 0, "{reply}");
            assert_eq!(calls, want, "{reply}");
        }
    }

    #[test]
    fn prose_and_code_stay_text() {
        let prose = "Here is a crate:\n```\nlet x = 1;\n```\nDone.";
        assert_eq!(parse_calls(prose), Ok(None));
        assert_eq!(to_app_format(prose), prose);
        let broken = "<tool_call>\n{\"name\": \"world.api\", \"arguments\": {\n</tool_call>";
        assert_eq!(parse_calls(broken), Err(()));
        assert_eq!(to_app_format(broken), broken);
    }

    #[test]
    fn empty_invoke_stubs_before_a_real_call_are_dropped() {
        let reply = "\n<invoke name=\"model_publish\">\n</invoke>\n\n<invoke name=\"x\">\n</invoke>\nRetrying.\n<tool_call>\n{\"name\":\"model_publish\",\"arguments\":{\"document\":\"d\"}}\n</tool_call>";
        let (at, calls) = parse_calls(reply).unwrap().unwrap();
        assert_eq!(at, 1);
        assert_eq!(calls, vec![call("model_publish", &[("document", Value::Str("d".into()))])]);
        // Alone, an argument-less invoke is a call.
        let (_, calls) = parse_calls("<invoke name=\"world.list\"></invoke>").unwrap().unwrap();
        assert_eq!(calls, vec![call("world.list", &[])]);
    }

    #[test]
    fn several_calls_keep_their_order() {
        let reply = "a\n<tool_call>{\"name\":\"x.one\",\"arguments\":{}}</tool_call>\n<tool_call>{\"name\":\"x.two\",\"arguments\":{\"k\":[1,2]}}</tool_call>";
        let out = to_app_format(reply);
        assert_eq!(out, "a\n<tool_call>\n<function=x.one>\n</function>\n</tool_call>\n<tool_call>\n<function=x.two>\n<parameter=k>\n[1,2]\n</parameter>\n</function>\n</tool_call>");
    }

    #[test]
    fn the_streamed_prefix_is_always_a_prefix_of_the_final_text() {
        let reply = "Sure thing.\n<tool_call>\n{\"name\": \"world.api\", \"arguments\": {\"op\": \"x\"}}\n</tool_call>";
        let fin = to_app_format(reply);
        for cut in 0..=reply.len() {
            if !reply.is_char_boundary(cut) {
                continue;
            }
            let shown = &reply[..stream_safe_len(&reply[..cut])];
            assert!(fin.starts_with(shown), "{cut}: {shown:?}");
            assert!(!shown.contains('<'), "{shown:?}");
        }
        assert_eq!(stream_safe_len("hello <tool"), "hello ".len());
        assert_eq!(stream_safe_len("a < b"), "a < b".len());
    }

    #[test]
    fn history_calls_are_shown_in_the_contract_form() {
        let turn = "On it.\n<tool_call>\n<function=world.set_source>\n<parameter=source>\ngame.sky({})\n</parameter>\n<parameter=size>\n120\n</parameter>\n</function>\n</tool_call>";
        assert_eq!(
            history_to_contract(turn),
            "On it.\n<tool_call>\n{\"name\":\"world.set_source\",\"arguments\":{\"source\":\"game.sky({})\",\"size\":120}}\n</tool_call>"
        );
        assert_eq!(history_to_contract("plain"), "plain");
    }

    #[test]
    fn tools_are_detected_by_the_call_markup() {
        assert!(app_has_tools("emit <tool_call> blocks"));
        assert!(!app_has_tools("just chat"));
    }
}
