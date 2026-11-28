//! The built-in remote commands described in the one tool schema (the AI services' manifest:
//! a service id, tools with a JSON-schema of arguments and a risk). Every app serves them as
//! the services `ui`, `edit`, `logs` and `inspect` of its `app` service, next to its own AI
//! services, so an agent, `help` and an MCP adapter read the same description. A call becomes
//! the `cmd` request it stands for. The JSON is the services' wire form (makepad-ai-services
//! `AppLine`, `ServiceManifest`), written here because the platform sits below that crate.

/// How much a command can break (the services' `Risk`: Read, Act, Destructive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdRisk {
    Read,
    Act,
    Destructive,
}

impl CmdRisk {
    fn wire(self) -> &'static str {
        match self {
            CmdRisk::Read => "Read",
            CmdRisk::Act => "Act",
            CmdRisk::Destructive => "Destructive",
        }
    }
}

/// One remote command.
#[derive(Clone, Copy)]
pub struct Command {
    /// The command words (`ui find`, `cap start`).
    pub words: &'static str,
    /// The usage line `help` prints.
    pub args: &'static str,
    /// What it answers.
    pub answers: &'static str,
    /// The service it belongs to: `ui`, `edit`, `logs` or `inspect`.
    pub service: &'static str,
    pub risk: CmdRisk,
    /// The JSON-schema `properties` of its arguments, without braces.
    pub params: &'static str,
    pub required: &'static [&'static str],
    /// Arguments that are words of the command line, in order (`design <op>`, `gpu snap <name>`).
    pub positional: &'static [&'static str],
}

/// The built-in services: id, label, brief.
pub const SERVICES: &[(&str, &str, &str)] = &[
    ("ui", "Window", "The app's window as a person uses it: read and act on its semantic tree (ui, ui find, ui act, ui type), inject raw mouse, keys and text for input tests, resize, wait until it settled, record or grab frames. Coordinates are window points as ui reports them."),
    ("edit", "Design", "Live design edits: widget data and structural Splash edits (design), the tweaker overlay and hot-patched shader constants."),
    ("logs", "Logs", "The app's log lines and trace topics."),
    ("inspect", "Inspect", "Where the app's GPU memory is."),
];

/// The tool name of a command: its words joined with `_`.
pub fn tool_name(words: &str) -> String {
    words.replace(' ', "_")
}

pub use crate::remote::json_str;

/// Flat argument object for a multi-op command. Each op's guide and the union of its
/// properties come from the same table that validates parameters at runtime.
fn flat_op_schema(operations: &[(&str, &str)]) -> String {
    let mut op_enum = Vec::with_capacity(operations.len());
    let mut op_guide = Vec::with_capacity(operations.len());
    // Keep first-seen order and the original entry when all its value schemas agree.
    let mut props: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for &(op, params) in operations {
        op_enum.push(json_str(op));
        let fields = makepad_remote::params::properties(params);
        if fields.is_empty() {
            op_guide.push(op.to_string());
        } else {
            let names: Vec<&str> = fields.iter().map(|(key, _)| *key).collect();
            op_guide.push(format!("{op}({})", names.join(",")));
        }
        for (key, entry) in fields {
            if key == "op" {
                continue;
            }
            let value = entry
                .strip_prefix('"')
                .and_then(|s| s.strip_prefix(key))
                .and_then(|s| s.strip_prefix('"'))
                .map(str::trim_start)
                .and_then(|s| s.strip_prefix(':'))
                .map(str::trim_start)
                .unwrap_or("{}");
            if let Some((_, _, values)) = props.iter_mut().find(|(k, _, _)| *k == key) {
                if !values.contains(&value) {
                    values.push(value);
                }
            } else {
                props.push((key, entry, vec![value]));
            }
        }
    }
    let mut properties = vec![format!(
        "\"op\":{{\"type\":\"string\",\"enum\":[{}],\"description\":{}}}",
        op_enum.join(","),
        json_str(&op_guide.join("; "))
    )];
    for (key, first_entry, values) in props {
        if values.len() == 1 {
            properties.push(first_entry.to_string());
        } else {
            properties.push(format!("{}:{{\"anyOf\":[{}]}}", json_str(key), values.join(",")));
        }
    }
    format!(
        "{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[\"op\"],\"additionalProperties\":false}}",
        properties.join(",")
    )
}

/// The JSON schema of a command's arguments.
pub fn schema(c: &Command) -> String {
    let operations = match c.words {
        "design" => Some(makepad_remote::params::DESIGN),
        "tweak op" => Some(makepad_remote::params::TWEAK),
        _ => None,
    };
    if let Some(operations) = operations {
        return flat_op_schema(operations);
    }
    let required: Vec<String> = c.required.iter().map(|r| json_str(r)).collect();
    format!("{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[{}]}}", c.params, required.join(","))
}

/// A service's manifest as the services' wire JSON.
pub fn manifest_json(service: &str, commands: &[&Command]) -> String {
    let (label, brief) = SERVICES.iter().find(|s| s.0 == service).map(|s| (s.1, s.2)).unwrap_or((service, ""));
    let tools: Vec<String> = commands
        .iter()
        .filter(|c| c.service == service)
        .map(|c| {
            let description = if c.args.is_empty() { c.answers.to_string() } else { format!("{} (usage: {} {})", c.answers, c.words, c.args) };
            format!(
                "{{\"name\":{},\"description\":{},\"parameters\":{},\"risk\":{{\"{}\":[]}},\"preview\":false}}",
                json_str(&tool_name(c.words)),
                json_str(&description),
                json_str(&schema(c)),
                c.risk.wire()
            )
        })
        .collect();
    format!("{{\"id\":{},\"label\":{},\"brief\":{},\"tools\":[{}],\"topics\":[]}}", json_str(service), json_str(label), json_str(brief), tools.join(","))
}

/// The `Instance` lines of the built-in services for an `app` list.
pub fn instance_lines(commands: &[&Command]) -> Vec<String> {
    SERVICES
        .iter()
        .filter(|(id, _, _)| commands.iter().any(|c| c.service == *id))
        .map(|(id, label, _)| {
            format!(
                "{{\"Instance\":{{\"endpoint\":[{}],\"meta\":{{\"app_id\":{},\"display_name\":{},\"location\":\"built into the app\",\"focus_epoch\":0,\"pid\":0}},\"manifest\":{}}}}}",
                json_str(id),
                json_str(id),
                json_str(label),
                manifest_json(id, commands)
            )
        })
        .collect()
}

/// The `cmd` request line a call stands for: the command's words (positional arguments
/// appended) and the remaining arguments as flat JSON fields.
pub fn request_line(c: &Command, args_json: &str) -> Result<String, String> {
    let body = args_json.trim();
    let inner = body.strip_prefix('{').and_then(|b| b.strip_suffix('}')).ok_or("the arguments must be a JSON object")?.trim();
    let mut words = c.words.to_string();
    let mut rest = inner.to_string();
    for p in c.positional {
        let (value, remaining) = take_string_field(&rest, p).ok_or_else(|| format!("{} needs `{p}`", tool_name(c.words)))?;
        words.push(' ');
        words.push_str(&value);
        rest = remaining;
    }
    let rest = rest.trim().trim_matches(',').trim().to_string();
    Ok(if rest.is_empty() { format!("{{\"cmd\":{}}}", json_str(&words)) } else { format!("{{\"cmd\":{},{rest}}}", json_str(&words)) })
}

/// Takes the string or number field `key` out of the inside of a flat JSON object.
fn take_string_field(inner: &str, key: &str) -> Option<(String, String)> {
    let pat = format!("{}:", json_str(key));
    let at = inner.find(&pat).or_else(|| inner.find(&format!("{} :", json_str(key))))?;
    let after = inner[at..].find(':')? + at + 1;
    let tail = inner[after..].trim_start();
    let start = inner.len() - tail.len();
    let (value, end) = if let Some(s) = tail.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = s.char_indices();
        let mut end = None;
        while let Some((i, ch)) = chars.next() {
            match ch {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        out.push(match n {
                            'n' => '\n',
                            't' => '\t',
                            o => o,
                        });
                    }
                }
                '"' => {
                    end = Some(start + 1 + i + 1);
                    break;
                }
                o => out.push(o),
            }
        }
        (out, end?)
    } else {
        let len = tail.find([',', '}']).unwrap_or(tail.len());
        (tail[..len].trim().to_string(), start + len)
    };
    if value.contains(char::is_whitespace) || value.is_empty() {
        return None;
    }
    let mut remaining = String::new();
    remaining.push_str(inner[..at].trim_end().trim_end_matches(','));
    let after_value = inner[end..].trim_start().trim_start_matches(',');
    if !remaining.trim().is_empty() && !after_value.trim().is_empty() {
        remaining.push(',');
    }
    remaining.push_str(after_value);
    Some((value, remaining))
}

/// An `app` `Result` line.
pub fn result_line(call_id: &str, ok: bool, text: &str) -> String {
    format!(
        "{{\"Result\":[{{\"call_id\":{},\"outcome\":{{\"{}\":[]}},\"text\":{},\"data\":\"\",\"note\":\"\",\"preview\":false,\"disposition\":{{\"Continue\":[]}},\"subscribe\":[],\"images\":[]}}]}}",
        json_str(call_id),
        if ok { "Ok" } else { "Failed" },
        json_str(text)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESIGN: Command = Command { words: "design", args: "<op> ...", answers: "edits", service: "edit", risk: CmdRisk::Act, params: "\"op\":{\"type\":\"string\"}", required: &["op"], positional: &["op"] };
    const CLICK: Command = Command { words: "click", args: "x= y=", answers: "a click", service: "ui", risk: CmdRisk::Act, params: "\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"}", required: &["x", "y"], positional: &[] };

    #[test]
    fn calls_become_cmd_lines() {
        assert_eq!(request_line(&CLICK, r#"{"x":10,"y":20}"#).unwrap(), r#"{"cmd":"click","x":10,"y":20}"#);
        assert_eq!(request_line(&CLICK, "{}").unwrap(), r#"{"cmd":"click"}"#);
        assert_eq!(request_line(&DESIGN, r#"{"path":"a.b","op":"insert","at":2}"#).unwrap(), r#"{"cmd":"design insert","path":"a.b","at":2}"#);
        assert_eq!(request_line(&DESIGN, r#"{"op":"undo"}"#).unwrap(), r#"{"cmd":"design undo"}"#);
        assert!(request_line(&DESIGN, "{}").is_err());
        assert!(request_line(&DESIGN, r#"{"op":"two words"}"#).is_err());
        assert!(request_line(&CLICK, "[1]").is_err());
    }

    #[test]
    fn design_and_tweak_op_emit_flat_provider_safe_schemas() {
        use crate::makepad_micro_serde::{DeJson, JsonValue};
        use makepad_remote::params;

        for (words, operations) in [("design", params::DESIGN), ("tweak op", params::TWEAK)] {
            let schema = schema(&Command { words, ..DESIGN });
            let root = JsonValue::deserialize_json_strict(&schema).unwrap();
            assert_eq!(root.get("type").and_then(JsonValue::as_str), Some("object"), "{words}");
            assert_eq!(root.get("additionalProperties"), Some(&JsonValue::Bool(false)), "{words}");
            assert_eq!(root.get("required"), Some(&JsonValue::Array(vec![JsonValue::String("op".into())])), "{words}");
            for keyword in ["oneOf", "anyOf", "allOf"] {
                assert!(root.get(keyword).is_none(), "{words} has top-level {keyword}");
            }
            let properties = root.get("properties").unwrap().as_object().unwrap();
            let op = &properties["op"];
            assert_eq!(op.get("type").and_then(JsonValue::as_str), Some("string"), "{words}");
            let names: Vec<_> = op.get("enum").unwrap().as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            assert_eq!(names, operations.iter().map(|(name, _)| *name).collect::<Vec<_>>(), "{words}");
            if words == "tweak op" {
                assert!(names.iter().all(|name| !name.starts_with("design_")), "{words} advertises a design alias");
            }
            let mut guide = op.get("description").unwrap().as_str().unwrap().split("; ");
            for &(name, params) in operations {
                let mut keys = Vec::new();
                for (key, _) in params::properties(params) {
                    assert!(properties.contains_key(key), "{words} lacks {key} for {name}");
                    keys.push(key);
                }
                let usage = if keys.is_empty() { name.to_string() } else { format!("{name}({})", keys.join(",")) };
                assert_eq!(guide.next(), Some(usage.as_str()), "{words} guide for {name}");
            }
            assert_eq!(guide.next(), None, "{words} has extra op guidance");
        }

        let conflict = flat_op_schema(&[("a", r#""x":{"type":"integer"}"#), ("b", r#""x":{"type":"string"}"#)]);
        let root = JsonValue::deserialize_json_strict(&conflict).unwrap();
        let expected = JsonValue::deserialize_json_strict(r#"{"anyOf":[{"type":"integer"},{"type":"string"}]}"#).unwrap();
        assert_eq!(root.get("properties").unwrap().get("x"), Some(&expected));
    }

    #[test]
    fn manifests_carry_names_schemas_and_risks() {
        let m = manifest_json("ui", &[&CLICK, &DESIGN]);
        assert!(m.starts_with(r#"{"id":"ui","label":"Window""#), "{m}");
        assert!(m.contains(r#""name":"click""#) && !m.contains("design"));
        assert!(m.contains(r#"\"required\":[\"x\",\"y\"]"#), "{m}");
        assert!(m.contains(r#""risk":{"Act":[]}"#));
        assert_eq!(tool_name("cap start"), "cap_start");
        assert_eq!(instance_lines(&[&CLICK]).len(), 1);
    }
}
