pub const AI_TASK_EVENT_PREFIX: &str = "TASK EVENT:";
pub const AI_WAITING_MESSAGE_PREFIX: &str = "WAITING:";
pub const AI_TERMINAL_OBSERVATION_PREFIX: &str = "TERMINAL OBSERVATION:";

pub fn parse_json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let start = json.find(&needle)? + needle.len();
    let mut out = String::new();
    let mut escaped = false;
    for ch in json[start..].chars() {
        if escaped {
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

pub fn parse_json_bool_field(json: &str, field: &str) -> Option<bool> {
    let true_needle = format!("\"{}\":true", field);
    if json.contains(&true_needle) {
        return Some(true);
    }
    let false_needle = format!("\"{}\":false", field);
    if json.contains(&false_needle) {
        return Some(false);
    }
    None
}
