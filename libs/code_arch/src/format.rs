use crate::Plan;
use std::fmt::Write;

/// Canonical v0 TOML: sorted IDs/files, fixed property order, no timestamps.
/// Prose breaks after `.`, `!`, or `?` followed by whitespace. TOML line
/// continuations preserve the original text bytes when inserting a line break.
/// Existing prose newlines and opaque visuals are preserved exactly as values.
/// Comments and author-chosen TOML quoting are not retained.
pub fn format(plan: &Plan) -> String {
    let mut out = String::new();
    writeln!(out, "[arch]\nversion = {}", plan.arch.version).unwrap();
    for (key, value) in [
        ("scope", &plan.arch.scope),
        ("title", &plan.arch.title),
        ("prompt", &plan.arch.prompt),
        ("generator", &plan.arch.generator),
    ] {
        writeln!(out, "{key} = {}", quoted(value)).unwrap();
    }
    out.push_str("\n[source]\nfiles = [\n");
    let mut files: Vec<_> = plan.source.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path).then(a.hash.cmp(&b.hash)));
    for file in files {
        writeln!(
            out,
            "    {{ path = {}, hash = {} }},",
            quoted(&file.path.to_string_lossy()),
            quoted(&file.hash)
        )
        .unwrap();
    }
    writeln!(out, "]\noverview = {}", prose(&plan.overview)).unwrap();
    out.push_str("\n[lanes]\n");
    for (id, lane) in &plan.lanes {
        writeln!(out, "{} = {{ title = {} }}", key(id), quoted(&lane.title)).unwrap();
    }
    out.push_str("\n[nodes]\n");
    // Ordinary node tables let prose remain readable without relying on
    // multiline inline-table syntax (which TOML 1.0 does not support).
    for (id, node) in &plan.nodes {
        writeln!(out, "\n[nodes.{}]", key(id)).unwrap();
        writeln!(
            out,
            "kind = {}\ntitle = {}",
            quoted(node.kind.as_str()),
            quoted(&node.title)
        )
        .unwrap();
        writeln!(
            out,
            "summary = {}\nstory = {}",
            prose(&node.summary),
            prose(&node.story)
        )
        .unwrap();
        if let Some(lane) = &node.lane {
            writeln!(out, "lane = {}", quoted(lane)).unwrap();
        }
        let refs = node
            .refs
            .iter()
            .map(|r| quoted(r))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "refs = [{refs}]").unwrap();
        for (name, value) in [
            ("budget", &node.budget),
            ("child", &node.child),
            ("visual", &node.visual),
        ] {
            if let Some(value) = value {
                writeln!(out, "{name} = {}", quoted(value)).unwrap();
            }
        }
    }
    out.push_str("\n[edges]\n");
    for (id, edge) in &plan.edges {
        write!(
            out,
            "{} = {{ from = {}, to = {}, kind = {}",
            key(id),
            quoted(&edge.from),
            quoted(&edge.to),
            quoted(edge.kind.as_str())
        )
        .unwrap();
        if let Some(label) = &edge.label {
            write!(out, ", label = {}", quoted(label)).unwrap();
        }
        out.push_str(" }\n");
    }
    out
}

fn key(text: &str) -> String {
    if crate::valid_id(text) {
        text.into()
    } else {
        quoted(text)
    }
}

fn escape(out: &mut String, ch: char) {
    match ch {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{08}' => out.push_str("\\b"),
        '\u{0c}' => out.push_str("\\f"),
        ch if ch.is_control() => {
            write!(out, "\\u{:04X}", ch as u32).unwrap();
        }
        ch => out.push(ch),
    }
}

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        escape(&mut out, ch);
    }
    out.push('"');
    out
}

fn prose(text: &str) -> String {
    if !text.contains('\n')
        && !text
            .chars()
            .zip(text.chars().skip(1))
            .any(|(a, b)| matches!(a, '.' | '!' | '?') && b.is_whitespace())
    {
        return quoted(text);
    }
    let mut out = String::from("\"\"\"\n");
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\n' {
            out.push('\n');
        } else {
            escape(&mut out, ch);
        }
        if matches!(ch, '.' | '!' | '?') && chars.peek().is_some_and(|c| c.is_whitespace()) {
            while chars
                .peek()
                .is_some_and(|c| c.is_whitespace() && *c != '\n')
            {
                escape(&mut out, chars.next().unwrap());
            }
            if chars.peek().is_some_and(|c| *c != '\n') {
                out.push_str("\\\n");
            }
        }
    }
    out.push_str("\"\"\"");
    out
}
