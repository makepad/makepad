//! Recover space in a proxy transcript while retaining the calls and findings still in use.
use crate::chat_wire::{ChatMessage, ChatRole};
use crate::local_llm::{contract_calls, parse_tool_call};
use makepad_strict_json::{parse_depth, Value};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

const REFUSED: &str = "[trimmed refused tool call";
const REPLACED: &str = "[trimmed superseded tool call";
const REFUSED_END: &str = " — payload omitted to fit context; see the error below]";
const REPLACED_END: &str = " — replaced by a later call; payload omitted to fit context]";
const ELIDED: &str = "[an earlier tool result";

fn is_elided_result(text: &str) -> bool {
    text.strip_prefix("ERROR: ").unwrap_or(text).starts_with(ELIDED)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckKind { None, Findings, Summary }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallSupersession { Live, Refused, Replaced }

enum TurnItem<'a> {
    Block { span: Range<usize>, body: &'a str },
    Trimmed { consumed_results: usize, superseded: bool },
}

#[derive(Clone, Debug)]
struct CallRecord {
    assistant_idx: usize,
    span: Option<Range<usize>>,
    block_call_count: usize,
    result_idx: Option<usize>,
    tool: String,
    action: Option<String>,
    slot: Option<String>,
    label: Option<String>,
    targets: Vec<String>,
    has_write_payload: bool,
    replaces_whole: bool,
    refused: bool,
    is_read: bool,
    canonical_read_key: Option<String>,
    supersession: CallSupersession,
    deferred_refusal: bool,
}

fn placeholder(text: &str) -> Option<(TurnItem<'_>, usize)> {
    let superseded = text.starts_with(REPLACED);
    let rest = text.strip_prefix(if superseded { REPLACED } else { REFUSED })?;
    let (count, desc) = if let Some(desc) = rest.strip_prefix(": ") {
        (1, desc)
    } else {
        let digits = rest.strip_prefix(" (")?;
        let n = digits.bytes().take_while(u8::is_ascii_digit).count();
        (digits[..n].parse().ok()?, digits[n..].strip_prefix(" calls): ")?)
    };
    let end = desc.find(['[', ']', '<', '>', '\r', '\n'])?;
    if !desc[..=end].ends_with(if superseded { REPLACED_END } else { REFUSED_END }) {
        return None;
    }
    Some((TurnItem::Trimmed { consumed_results: count, superseded }, text.len() - desc.len() + end + 1))
}

fn extract_turn_items(text: &str) -> Vec<TurnItem<'_>> {
    let mut items = Vec::new();
    let mut pos = 0;
    while let Some(offset) = text[pos..].find(['<', '[']) {
        let start = pos + offset;
        let rest = &text[start..];
        if let Some(thought) = rest.strip_prefix("<think>") {
            let Some(end) = thought.find("</think>") else { break };
            pos = start + "<think>".len() + end + "</think>".len();
        } else if let Some(body) = rest.strip_prefix("<tool_call>") {
            let Some(end) = body.find("</tool_call>") else { break };
            pos = start + "<tool_call>".len() + end + "</tool_call>".len();
            items.push(TurnItem::Block { span: start..pos, body: &body[..end] });
        } else if let Some((item, len)) = placeholder(rest) {
            items.push(item);
            pos = start + len;
        } else {
            pos = start + 1;
        }
    }
    items
}

type Args = Vec<(String, String)>;

fn parse_call_meta(tool: String, args: Args, assistant_idx: usize, span: Option<Range<usize>>, block_call_count: usize, result_idx: Option<usize>, refused: bool) -> CallRecord {
    let mut effective: BTreeMap<_, _> = args.into_iter().collect();
    let action = effective.get("action").map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    if let Some(Value::Obj(fields)) = effective.get("args").and_then(|s| parse_depth(s.as_bytes(), 32).ok()) {
        effective.remove("args");
        for (key, value) in fields {
            effective.insert(key, match value { Value::Str(s) => s, v => v.to_json() });
        }
    }
    let arg = |k: &str| effective.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
    let slot = arg("slot").map(str::to_owned);
    let label = arg("label").map(str::to_owned);
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    let mut target = |s: &str| {
        let s = s.trim();
        if !s.is_empty() && seen.insert(s.to_owned()) { targets.push(s.to_owned()); }
    };
    for key in ["doc", "path", "script", "file"] {
        if let Some(s) = arg(key) { target(s); }
    }
    let mut whole_edits = false;
    if let Some(Value::Arr(edits)) = arg("edits").and_then(|s| parse_depth(s.as_bytes(), 32).ok()) {
        whole_edits = !edits.is_empty();
        for edit in edits {
            whole_edits &= matches!(edit.get("op").and_then(Value::as_str), Some("create" | "replace" | "remove"));
            for key in ["doc", "path", "file"] {
                if let Some(s) = edit.get(key).and_then(Value::as_str) { target(s); }
            }
        }
    }
    let body = ["text", "content", "source", "body", "code"].iter().any(|k| effective.contains_key(*k));
    let patch = ["edits", "patch", "old_string", "old"].iter().any(|k| effective.contains_key(*k));
    let has_write_payload = slot.is_some() || ["edits", "text", "content", "source", "patch", "old_string", "new_string", "body", "code"].iter().any(|k| arg(k).is_some());
    let replaces_whole = slot.is_some() || (targets.len() == 1 && body && !patch) || whole_edits;
    let is_read = !has_write_payload && !refused && !tool.is_empty();
    let canonical_read_key = is_read.then(|| {
        // JSON quoting keeps a separator embedded in a value distinct from another argument.
        let args = Value::Obj(effective.into_iter().map(|(k, v)| (k, Value::Str(v))).collect());
        Value::Arr(vec![Value::Str(tool.clone()), Value::Str(action.clone().unwrap_or_default()), args]).to_json()
    });
    CallRecord {
        assistant_idx, span, block_call_count, result_idx, tool, action, slot, label, targets,
        has_write_payload, replaces_whole, refused, is_read, canonical_read_key,
        supersession: if refused { CallSupersession::Refused } else { CallSupersession::Live },
        deferred_refusal: false,
    }
}

fn clean(value: &str) -> String {
    value.split(|c: char| c.is_ascii_whitespace() || matches!(c, '<' | '>' | '[' | ']'))
        .filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}

impl CallRecord {
    fn descriptor(&self) -> String {
        let mut name = if self.tool.is_empty() { "unreadable tool call".into() } else { clean(&self.tool) };
        if let Some(action) = &self.action { name.push(' '); name.push_str(&clean(action)); }
        let mut details = Vec::new();
        if let Some(slot) = &self.slot {
            details.push(format!("slot \"{}\"", clean(slot)));
        } else if !self.targets.is_empty() {
            let mut targets = self.targets.iter().take(3).map(|s| clean(s)).collect::<Vec<_>>().join(", ");
            if self.targets.len() > 3 { targets.push_str(", ..."); }
            details.push(format!("target \"{targets}\""));
        }
        if let Some(label) = &self.label {
            details.push(format!("\"{}\"", clean(label).chars().take(64).collect::<String>()));
        }
        if !details.is_empty() { name.push_str(&format!(" ({})", details.join(", "))); }
        name
    }

    fn trim_note(&self) -> String {
        let desc = self.descriptor();
        if self.refused {
            let count = if self.block_call_count == 1 { String::new() } else { format!(" ({} calls)", self.block_call_count) };
            format!("{REFUSED}{count}: {desc}{REFUSED_END}")
        } else {
            format!("{REPLACED}: {desc}{REPLACED_END}")
        }
    }
}

#[derive(Default)]
struct ToolResult {
    call: Option<usize>,
    direct_stale: bool,
    broad_stale: bool,
    resolved_error: bool,
    protected: bool,
    recent: bool,
}

fn collect_calls(history: &[ChatMessage], results: &mut [ToolResult]) -> Vec<CallRecord> {
    let mut records = Vec::new();
    for (a, msg) in history.iter().enumerate().filter(|(_, m)| m.role == ChatRole::Assistant) {
        let first = a + 1;
        let m = history[first..].iter().take_while(|m| m.role == ChatRole::Tool).count();
        let mut total_parsed = 0usize;
        let items: Vec<_> = extract_turn_items(&msg.text).into_iter().map(|item| {
            let calls = match &item {
                TurnItem::Trimmed { consumed_results, .. } => {
                    total_parsed = total_parsed.saturating_add(*consumed_results);
                    Vec::new()
                }
                TurnItem::Block { body, .. } => {
                    let calls = parse_tool_call(body).ok().map(|c| vec![c]).or_else(|| contract_calls(body)).unwrap_or_default();
                    total_parsed = total_parsed.saturating_add(calls.len());
                    calls
                }
            };
            (item, calls)
        }).collect();
        let mut cursor = 0;
        for (item, calls) in items {
            let span = match item {
                TurnItem::Trimmed { consumed_results, superseded } => {
                    let count = consumed_results.min(m - cursor);
                    if superseded {
                        for i in first + cursor..first + cursor + count {
                            results[i].direct_stale = !history[i].text.starts_with("ERROR: ");
                        }
                    }
                    cursor += count;
                    continue;
                }
                TurnItem::Block { span, .. } => span,
            };
            let k = calls.len();
            if k == 0 {
                let rewrite = total_parsed == 0 && m > 0 && history[first].text.starts_with("ERROR: ");
                let idx = rewrite.then_some(first);
                records.push(parse_call_meta(String::new(), Vec::new(), a, rewrite.then_some(span), 0, idx, rewrite));
                continue;
            }
            let offset = first + cursor;
            let all_refused = k <= m - cursor && history[offset..offset + k].iter().all(|m| m.text.starts_with("ERROR: "));
            for (j, (name, args)) in calls.into_iter().enumerate() {
                let idx = (cursor + j < m).then_some(offset + j);
                let refused = idx.is_some_and(|i| history[i].text.starts_with("ERROR: "));
                if let Some(i) = idx { results[i].call = Some(records.len()); }
                let trimmable = j == 0 && (k == 1 || all_refused);
                records.push(parse_call_meta(name, args, a, trimmable.then(|| span.clone()), if trimmable { k } else { 1 }, idx, refused));
            }
            cursor += k.min(m - cursor);
        }
    }
    records
}

fn classify_calls(records: &mut [CallRecord], results: &mut [ToolResult], last_assistant: Option<usize>) {
    // Keys borrow the records' immutable metadata; decisions are applied after both scans.
    let mut decisions: Vec<_> = records.iter().map(|r| (r.supersession, false)).collect();
    let mut slots = HashSet::new();
    let mut targets = HashSet::new();
    for (i, r) in records.iter().enumerate() {
        let slot = r.slot.as_ref().map(|s| (&r.tool, &r.action, s));
        if r.refused {
            decisions[i].1 = Some(r.assistant_idx) == last_assistant
                && !slot.is_some_and(|s| slots.contains(&s))
                && !r.targets.iter().any(|t| targets.contains(t));
        } else {
            if let Some(s) = slot { slots.insert(s); }
            targets.extend(r.targets.iter());
        }
    }
    slots.clear();
    targets.clear();
    let mut whole_targets = HashSet::new();
    let mut written_targets = HashSet::new();
    let mut later_reads = HashSet::new();
    let mut later_write = false;
    for (i, r) in records.iter().enumerate().rev() {
        let slot = r.slot.as_ref().map(|s| (&r.tool, &r.action, s));
        if r.refused {
            if let Some(idx) = r.result_idx {
                results[idx].resolved_error = slot.is_some_and(|s| slots.contains(&s)) || r.targets.iter().any(|t| targets.contains(t));
            }
            continue;
        }
        let single = match r.targets.as_slice() { [t] => Some((&r.tool, &r.action, t)), _ => None };
        let replaced = slot.is_some_and(|s| slots.contains(&s)) || single.is_some_and(|t| whole_targets.contains(&t));
        if replaced {
            decisions[i].0 = CallSupersession::Replaced;
        } else {
            if let Some(s) = slot { slots.insert(s); }
            if let Some(t) = single.filter(|_| r.replaces_whole) { whole_targets.insert(t); }
        }
        if let Some(idx) = r.result_idx {
            results[idx].direct_stale = replaced || (r.is_read && (r.targets.iter().any(|t| written_targets.contains(t))
                || r.canonical_read_key.as_ref().is_some_and(|k| later_reads.contains(k))));
            results[idx].broad_stale = r.is_read && r.targets.is_empty() && later_write;
        }
        if let Some(k) = &r.canonical_read_key { later_reads.insert(k); }
        if r.has_write_payload {
            later_write = true;
            written_targets.extend(r.targets.iter());
        }
        targets.extend(r.targets.iter());
    }
    for (i, r) in records.iter_mut().enumerate() {
        r.supersession = decisions[i].0;
        r.deferred_refusal = if r.block_call_count > 1 {
            decisions[i..i + r.block_call_count].iter().any(|d| d.1)
        } else { decisions[i].1 };
    }
}

fn diagnostic_keys(text: &str) -> (bool, bool) {
    let text = if text.starts_with("[The application set changed.") {
        text.split_once("</tools>").map_or("", |(_, body)| body)
    } else { text };
    let bytes = text.trim_ascii_start().as_bytes();
    if bytes.first() != Some(&b'{') { return (false, false); }
    let (mut findings, mut summary) = (false, false);
    let mut depth = 1usize;
    let mut i = 1;
    while i < bytes.len() && depth > 0 {
        let key = match bytes[i] {
            b'{' | b'[' => { depth += 1; i += 1; continue; }
            b'}' | b']' => { depth -= 1; i += 1; continue; }
            b'"' => {
                let start = i + 1;
                i = start;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                if i >= bytes.len() { break; }
                i += 1;
                &bytes[start..i - 1]
            }
            c if c.is_ascii_alphanumeric() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
                &bytes[start..i]
            }
            _ => { i += 1; continue; }
        };
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) { i += 1; }
        if depth == 1 && bytes.get(i) == Some(&b':') {
            let kind = match key {
                b"findings" | b"diagnostics" | b"check" => CheckKind::Findings,
                b"checks" | b"failures" => CheckKind::Summary,
                _ => CheckKind::None,
            };
            findings |= kind == CheckKind::Findings;
            summary |= kind == CheckKind::Summary;
        }
    }
    (findings, summary)
}

fn protect_results(history: &[ChatMessage], results: &mut [ToolResult]) {
    let (mut findings, mut summary, mut error) = (false, false, false);
    let mut recent = 0;
    for (msg, result) in history.iter().zip(results).rev().filter(|(m, _)| m.role == ChatRole::Tool) {
        if msg.text.starts_with("ERROR: ") {
            if !error && !result.resolved_error && !is_elided_result(&msg.text) {
                result.protected = true;
                error = true;
            }
        } else if !findings || !summary {
            let (f, s) = diagnostic_keys(&msg.text);
            result.protected = (f && !findings) || (s && !summary);
            findings |= f;
            summary |= s;
        }
        if !result.direct_stale && recent < 2 {
            result.recent = true;
            recent += 1;
        }
    }
}

struct Budget {
    remaining: usize,
    edits: Vec<(usize, Range<usize>, String)>,
}

impl Budget {
    fn trim_calls(&mut self, records: &[CallRecord], deferred: bool) {
        for r in records {
            if self.remaining == 0 { break; }
            let Some(span) = &r.span else { continue };
            if r.supersession == CallSupersession::Live || r.deferred_refusal != deferred { continue; }
            let note = r.trim_note();
            if span.len() > note.len() * 2 {
                self.remaining = self.remaining.saturating_sub((span.len() - note.len()) / 3);
                self.edits.push((r.assistant_idx, span.clone(), note));
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ResultTier { Direct, Broad, General }

/// Replace the oldest eligible payloads in priority order, reporting each original message once.
pub(crate) fn shorten_history(history: &mut [ChatMessage], need: usize, max: usize, mut replaced: impl FnMut(usize, String)) -> bool {
    let target = need.saturating_sub(max / 5 * 4 + max % 5 * 4 / 5);
    if target == 0 { return false; }
    let mut results: Vec<_> = (0..history.len()).map(|_| ToolResult::default()).collect();
    let mut records = collect_calls(history, &mut results);
    classify_calls(&mut records, &mut results, history.iter().rposition(|m| m.role == ChatRole::Assistant));
    protect_results(history, &mut results);
    let mut budget = Budget { remaining: target, edits: Vec::new() };
    budget.trim_calls(&records, false);
    for tier in [ResultTier::Direct, ResultTier::Broad, ResultTier::General] {
        for (i, (msg, result)) in history.iter_mut().zip(&results).enumerate() {
            if budget.remaining == 0 { break; }
            if msg.role != ChatRole::Tool || result.protected || is_elided_result(&msg.text) { continue; }
            let is_error = msg.text.starts_with("ERROR: ");
            let eligible = match tier {
                ResultTier::Direct => result.direct_stale && !is_error,
                ResultTier::Broad => result.broad_stale && !is_error,
                ResultTier::General => !result.recent,
            };
            if !eligible { continue; }
            let desc = result.call.map(|n| format!(" ({})", records[n].descriptor())).unwrap_or_default();
            let reason = match tier {
                ResultTier::Direct | ResultTier::Broad => "because a later call superseded it",
                ResultTier::General => "to keep the conversation within the model's context",
            };
            let note = if is_error {
                format!("ERROR: {ELIDED}{desc}, elided {reason}]")
            } else {
                format!("{ELIDED}{desc}, elided {reason}; call the tool again if you need it]")
            };
            if msg.text.len() > note.len() * 2 {
                budget.remaining = budget.remaining.saturating_sub((msg.text.len() - note.len()) / 3);
                replaced(i, std::mem::take(&mut msg.text));
                msg.text = note;
            }
        }
    }
    budget.trim_calls(&records, true);
    // Work backwards from the original offsets, copying even a many-call turn only once.
    budget.edits.sort_unstable_by_key(|(i, span, _)| (*i, std::cmp::Reverse(span.start)));
    for group in budget.edits.chunk_by(|a, b| a.0 == b.0) {
        let idx = group[0].0;
        let original = &history[idx].text;
        let mut end = original.len();
        let mut pieces = Vec::new();
        for (_, span, note) in group {
            pieces.push(&original[span.end..end]);
            pieces.push(note.as_str());
            end = span.start;
        }
        pieces.push(&original[..end]);
        pieces.reverse();
        let shortened = pieces.concat();
        replaced(idx, std::mem::take(&mut history[idx].text));
        history[idx].text = shortened;
    }
    budget.remaining < target
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_llm::tool_call_markup;
    use ChatRole::{Assistant, System, Tool, User};

    fn call(tool: &str, args: &[(&str, &str)]) -> String {
        tool_call_markup(tool, &args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>())
    }

    fn edit(slot: &str, text: &str) -> String {
        call("doc.edit", &[("slot", slot), ("text", text)])
    }

    fn pair(history: &mut Vec<ChatMessage>, markup: &str, result: &str) {
        history.push(ChatMessage::new(Assistant, markup));
        history.push(ChatMessage::new(Tool, result));
    }

    fn shorten(history: &mut [ChatMessage], target: usize) -> bool {
        shorten_history(history, 4_000 + target, 5_000, |_, _| {})
    }

    #[test]
    fn refused_and_replaced_proposals_trim_while_newest_proposal_and_checks_stay() {
        let payload = "body".repeat(1024);
        let files = Value::Arr(["a", "b"].map(|doc| makepad_strict_json::obj(vec![
            ("doc", Value::Str(doc.into())), ("op", Value::Str("replace".into())), ("text", Value::Str(payload.clone())),
        ])).into()).to_json();
        let proposal = call("doc.edit", &[("action", "step.propose"), ("args", &makepad_strict_json::obj(vec![
            ("slot", Value::Str("architecture".into())), ("label", Value::Str("Block architecture".into())),
            ("edits", parse_depth(files.as_bytes(), 32).unwrap()),
        ]).to_json())]);
        let findings = format!("{{findings: [\"{payload}\"]}}");
        let checks = format!("{{checks: {{counts: []}}, detail: \"{payload}\"}}");
        let mut h = vec![ChatMessage::new(System, "instructions"), ChatMessage::new(User, "current request")];
        pair(&mut h, &call("doc.read", &[("doc", "spec.txt")]), &payload);
        pair(&mut h, &proposal, &format!("{{checks: {{counts: [\"{payload}\"]}}}}"));
        pair(&mut h, &call("check.list", &[]), &findings);
        pair(&mut h, &call("design.read", &[]), &payload);
        pair(&mut h, &proposal, "ERROR: invalid change");
        pair(&mut h, &proposal, &checks);
        assert!(shorten(&mut h, 100_000));
        assert!(h[4].text.starts_with("[trimmed superseded tool call: doc.edit step.propose (slot \"architecture\", \"Block architecture\")"));
        assert!(h[10].text.starts_with("[trimmed refused tool call: doc.edit step.propose"));
        assert_eq!(h[12].text, proposal);
        assert_eq!((&h[7].text, &h[13].text), (&findings, &checks));
        assert_eq!((h[0].text.as_str(), h[1].text.as_str()), ("instructions", "current request"));
        assert!(h[9].text.contains("a later call superseded it"));
    }

    #[test]
    fn refused_proposal_never_supersedes_earlier_accepted_proposal_for_same_slot() {
        let live = edit("architecture", &"x".repeat(2048));
        let refused = edit("architecture", &"y".repeat(2048));
        let mut h = Vec::new();
        pair(&mut h, &live, "ok");
        pair(&mut h, &refused, "ERROR: invalid");
        assert!(shorten(&mut h, 1000));
        assert_eq!(h[0].text, live);
        assert!(h[2].text.starts_with(REFUSED));
        assert_eq!(h[3].text, "ERROR: invalid");

        // A small refused call survives the span threshold even when its diagnostic is large.
        // After other results arrive, eliding that diagnostic must never turn it into a write.
        let mut history = Vec::new();
        pair(&mut history, &live, "ok");
        pair(&mut history, &edit("architecture", "x"), &format!("ERROR: {}", "bad change ".repeat(256)));
        pair(&mut history, &edit("other", "y"), "ERROR: missing field");
        pair(&mut history, &call("doc.read", &[("doc", "one")]), "one");
        pair(&mut history, &call("doc.read", &[("doc", "two")]), "two");
        let refused_call = history[2].text.clone();
        let error_note = r#"ERROR: [an earlier tool result (doc.edit (slot "architecture")), elided to keep the conversation within the model's context]"#;
        assert!(shorten(&mut history, 50));
        assert_eq!(history[0].text, live);
        assert_eq!(history[2].text, refused_call);
        assert_eq!(history[3].text, error_note);
        assert!(!shorten(&mut history, 50));
        assert!(history[0].text == live, "a refused short write displaced the accepted draft: {}", history[0].text);
        assert_eq!(history[2].text, refused_call);
        assert_eq!(history[3].text, error_note);
    }

    #[test]
    fn distinct_slots_do_not_supersede_each_other() {
        let mut h = Vec::new();
        for slot in ["requirements", "architecture"] { pair(&mut h, &edit(slot, &"x".repeat(1024)), "ok"); }
        let original = h.clone();
        assert!(!shorten(&mut h, 1000));
        assert_eq!(h, original);
    }

    #[test]
    fn incremental_patch_never_supersedes_whole_file_write_but_whole_write_does() {
        let text = "x".repeat(2048);
        let whole = call("doc.text", &[("doc", "a.txt"), ("text", &text)]);
        let patch = call("doc.text", &[("doc", "a.txt"), ("edits", &format!(r#"[{{"old":"{text}","new":"new"}}]"#))]);
        let mut h = Vec::new();
        pair(&mut h, &whole, "ok"); pair(&mut h, &patch, "ok");
        assert!(!shorten(&mut h, 2000));
        let latest = call("doc.text", &[("doc", "a.txt"), ("text", &"y".repeat(2048))]);
        pair(&mut h, &latest, "ok");
        assert!(shorten(&mut h, 2000));
        assert!(h[0].text.starts_with(REPLACED) && h[2].text.starts_with(REPLACED));
        assert_eq!(h[4].text, latest);
    }

    #[test]
    fn stale_targeted_read_and_duplicate_read_elide_before_unmodified_reads() {
        let text = "x".repeat(2048);
        let read = |doc| call("doc.read", &[("doc", doc)]);
        let mut h = Vec::new();
        pair(&mut h, &read("spec.txt"), &text); pair(&mut h, &read("a.txt"), &text);
        pair(&mut h, &call("doc.write", &[("edits", r#"[{"doc":"a.txt","op":"replace"}]"#)]), "ok");
        assert!(shorten(&mut h, 50));
        assert_eq!(h[1].text, text);
        assert!(h[3].text.contains("because a later call"));
        let mut pages = Vec::new();
        pair(&mut pages, &call("doc.read", &[("doc", "a"), ("from", "1"), ("to", "50")]), &text);
        pair(&mut pages, &call("doc.read", &[("doc", "a"), ("from", "51"), ("to", "100")]), &text);
        assert!(!shorten(&mut pages, 50));
        pair(&mut pages, &call("doc.read", &[("to", "50"), ("from", "1"), ("doc", "a")]), &text);
        assert!(shorten(&mut pages, 50));
        assert!(pages[1].text.contains("because a later call"));
        assert_eq!(pages[3].text, text);
    }

    #[test]
    fn newest_check_findings_stay_protected_even_outside_newest_two_results_and_no_false_positive_in_code_reads() {
        let text = "a".repeat(1024);
        for prefix in ["", "[The application set changed.]\n<tools>...</tools>\n"] {
            let findings = format!("{prefix}{{current: true findings: [\"{text}\"]}}");
            let mut h = Vec::new();
            pair(&mut h, &call("check.list", &[]), &findings);
            pair(&mut h, &call("read_file", &[("path", "x.rs")]), &format!(r#"{{doc: "x.rs" text: "let findings: Vec<_> = {text};"}}"#));
            pair(&mut h, &call("read_file", &[("path", "b")]), "b");
            pair(&mut h, &call("read_file", &[("path", "c")]), "c");
            assert!(shorten(&mut h, 1000));
            assert_eq!(h[1].text, findings);
            assert!(h[3].text.starts_with(ELIDED));
        }
    }

    #[test]
    fn superseded_call_payloads_trim_before_tool_results_and_handle_json_contract_and_multi_call_turns() {
        let data = "x".repeat(2048);
        let old = format!(r#"<tool_call>{{"name":"write_file","arguments":{{"path":"a.rs","content":"{data}"}}}}</tool_call>"#);
        let live = edit("other", &data);
        let mut h = vec![ChatMessage::new(Tool, &data), ChatMessage::new(Assistant, format!("before {old} between {live} after")),
            ChatMessage::new(Tool, "ok"), ChatMessage::new(Tool, "ok")];
        pair(&mut h, &call("write_file", &[("path", "a.rs"), ("content", "replacement")]), "ok");
        assert!(shorten(&mut h, 50));
        assert_eq!(h[0].text, data);
        assert_eq!(h[1].text, format!("before {REPLACED}: write_file (target \"a.rs\"){REPLACED_END} between {live} after"));
    }

    #[test]
    fn first_ever_refused_draft_is_deferred_to_last_resort_tier_4() {
        let data = "x".repeat(2048);
        let draft = edit("arch", &data);
        let mut h = Vec::new();
        pair(&mut h, &call("doc.read", &[("doc", "old")]), &data);
        pair(&mut h, &call("doc.read", &[("doc", "other")]), "small");
        pair(&mut h, &draft, "ERROR: retry");
        assert!(shorten(&mut h, 50));
        assert!(h[1].text.starts_with(ELIDED)); assert_eq!(h[4].text, draft);
        assert!(shorten(&mut h, 1000));
        assert!(h[4].text.starts_with(REFUSED)); assert_eq!(h[5].text, "ERROR: retry");
    }

    #[test]
    fn shortening_is_idempotent_on_repeated_passes() {
        let data = "x".repeat(2048);
        let mut h = Vec::new();
        let proposal = call("doc.edit", &[("slot", "a"), ("label", "bad]\n<tool_call>"), ("text", &data)]);
        pair(&mut h, &proposal, &data);
        pair(&mut h, &edit("a", "current"), "ok");
        assert!(shorten(&mut h, 2000));
        assert!(!h[0].text.contains("<tool_call>"));
        let once = h.clone();
        assert!(!shorten(&mut h, 2000)); assert_eq!(h, once);
    }

    #[test]
    fn unreadable_block_beside_parsed_call_does_not_shift_result_pairing() {
        let data = "x".repeat(2048);
        let broken = format!("<tool_call>{{broken {data}}}</tool_call>");
        let valid = edit("arch", &data);
        let mut h = Vec::new();
        pair(&mut h, &valid, "ok");
        pair(&mut h, &format!("{broken}{valid}"), "ERROR: invalid proposal");
        assert!(shorten(&mut h, 50));
        let expected = format!("{broken}{REFUSED}: doc.edit (slot \"arch\"){REFUSED_END}");
        assert_eq!(h[2].text, expected);
        assert!(!shorten(&mut h, 2000)); assert_eq!(h[2].text, expected);

        let mut h = Vec::new();
        pair(&mut h, &format!("{broken}{broken}"), "ERROR: your tool call could not be read");
        h.push(ChatMessage::new(Assistant, "later"));
        let note = format!("{REFUSED} (0 calls): unreadable tool call{REFUSED_END}");
        assert!(shorten(&mut h, 50)); assert_eq!(h[0].text, format!("{note}{broken}"));
        assert!(shorten(&mut h, 2000)); assert_eq!(h[0].text, format!("{note}{note}"));
    }

    #[test]
    fn repeated_shortening_passes_keep_sibling_result_pairing_aligned() {
        let data = "x".repeat(2048);
        let array = format!(r#"<tool_call>[{{"name":"doc.edit","arguments":{{"slot":"a","text":"{data}"}}}},{{"name":"doc.edit","arguments":{{"slot":"b","text":"{data}"}}}}]</tool_call>"#);
        for (refused, count, extra, live) in [(edit("a", &data), "", false, edit("b", &data)), (array, " (2 calls)", true, edit("c", &data))] {
            let mut h = vec![ChatMessage::new(Assistant, format!("{refused}{live}")), ChatMessage::new(Tool, "ERROR: bad a")];
            if extra { h.push(ChatMessage::new(Tool, "ERROR: bad b")); }
            h.extend([ChatMessage::new(Tool, "ok"), ChatMessage::new(Assistant, "later")]);
            assert!(shorten(&mut h, 50));
            let expected = format!("{REFUSED}{count}: doc.edit (slot \"a\"){REFUSED_END}{live}");
            assert_eq!(h[0].text, expected);
            assert!(!shorten(&mut h, 2000)); assert_eq!(h[0].text, expected);
        }
        let other = edit("b", &data);
        let mut h = vec![ChatMessage::new(Assistant, format!("{}{other}", edit("a", &data))), ChatMessage::new(Tool, &data),
            ChatMessage::new(Tool, "small"), ChatMessage::new(Assistant, edit("a", "replacement"))];
        assert!(shorten(&mut h, 50)); assert_eq!(h[1].text, data);
        assert!(shorten(&mut h, 50));
        assert!(h[1].text.starts_with("[an earlier tool result, elided because a later call superseded it"));
        assert!(h[0].text.ends_with(&other)); assert_eq!(h[2].text, "small");
    }
}
