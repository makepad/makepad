//! A live activity log of the agent for Scope to ingest.
//!
//! The format is the agent-client-protocol update stream Scope already tails
//! for other agents: `<root>/<percent-encoded cwd>/<session>/updates.jsonl`,
//! one `{timestamp, method, params:{sessionId, update, _meta}}` per line, plus
//! `summary.json` (title, cwd) and, for sub-agents, their own session folder
//! and a `subagents/<child>/meta.json` link in the parent's. Point Scope's
//! agent-session root at `<session_dir>/activity` (or at the folder named by
//! `MAKEPAD_CODE_ACTIVITY_DIR`).

use super::event::*;
use makepad_strict_json::{self as json, Value};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn percent_encode(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

struct Pending {
    lane: LaneId,
    name: String,
    args: Vec<(String, String)>,
}

pub struct ScopeLog {
    project_dir: PathBuf,
    cwd: String,
    main_id: String,
    files: HashMap<LaneId, (String, File)>,
    pending: HashMap<ToolId, Pending>,
    /// Streamed text not yet written, per lane (one chunk line per message).
    text: HashMap<LaneId, String>,
    children: HashMap<LaneId, String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl ScopeLog {
    /// `base` is the activity folder; `None` when logging is off.
    pub fn base_for(session_dir: Option<&Path>) -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("MAKEPAD_CODE_ACTIVITY_DIR").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        session_dir.map(|d| d.join("activity"))
    }

    pub fn new(base: &Path, cwd: &Path, session_id: &str, title: &str) -> ScopeLog {
        let cwd = cwd.display().to_string();
        let mut log = ScopeLog {
            project_dir: base.join(percent_encode(&cwd)),
            cwd,
            main_id: session_id.to_string(),
            files: HashMap::new(),
            pending: HashMap::new(),
            text: HashMap::new(),
            children: HashMap::new(),
        };
        log.open(0, session_id, title);
        log
    }

    fn open(&mut self, lane: LaneId, id: &str, title: &str) {
        let dir = self.project_dir.join(id);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let summary = json::obj(vec![
            ("session_summary", json::s(title)),
            ("info", json::obj(vec![("cwd", json::s(self.cwd.clone()))])),
        ]);
        let _ = std::fs::write(dir.join("summary.json"), summary.to_json());
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(dir.join("updates.jsonl")) {
            self.files.insert(lane, (id.to_string(), file));
        }
    }

    pub fn set_title(&mut self, title: &str) {
        if let Some((id, _)) = self.files.get(&0) {
            let id = id.clone();
            let summary = json::obj(vec![
                ("session_summary", json::s(title)),
                ("info", json::obj(vec![("cwd", json::s(self.cwd.clone()))])),
            ]);
            let _ = std::fs::write(self.project_dir.join(id).join("summary.json"), summary.to_json());
        }
    }

    fn write(&mut self, lane: LaneId, update: Value) {
        let Some((id, file)) = self.files.get_mut(&lane) else { return };
        let ms = now_ms();
        let line = json::obj(vec![
            ("timestamp", Value::F64(ms as f64 / 1000.0)),
            ("method", json::s("session/update")),
            (
                "params",
                json::obj(vec![
                    ("sessionId", json::s(id.clone())),
                    ("update", update),
                    ("_meta", json::obj(vec![("agentTimestampMs", Value::Int(ms as i64))])),
                ]),
            ),
        ]);
        let mut text = line.to_json();
        text.push('\n');
        let _ = file.write_all(text.as_bytes());
    }

    fn flush_text(&mut self, lane: LaneId) {
        if let Some(text) = self.text.remove(&lane).filter(|t| !t.trim().is_empty()) {
            self.write(
                lane,
                json::obj(vec![
                    ("sessionUpdate", json::s("agent_message_chunk")),
                    ("content", json::obj(vec![("text", json::s(text))])),
                ]),
            );
        }
    }

    fn flush_all(&mut self) {
        let lanes: Vec<LaneId> = self.text.keys().copied().collect();
        for l in lanes {
            self.flush_text(l);
        }
    }

    /// A user message (the prompt) of the main session.
    pub fn user(&mut self, text: &str) {
        self.flush_all();
        self.write(
            0,
            json::obj(vec![
                ("sessionUpdate", json::s("user_message_chunk")),
                ("content", json::obj(vec![("text", json::s(text))])),
            ]),
        );
    }

    /// Feed every event the engine emits.
    pub fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Delta { lane, text } => {
                self.text.entry(*lane).or_default().push_str(text);
            }
            AgentEvent::ToolStarted(call) => {
                self.flush_text(call.lane);
                let (name, input) = grok_input(&call.name, &call.args, &self.children);
                self.pending.insert(
                    call.id,
                    Pending { lane: call.lane, name: call.name.clone(), args: call.args.clone() },
                );
                if call.name != "todo" {
                    self.write(
                        call.lane,
                        json::obj(vec![
                            ("sessionUpdate", json::s("tool_call")),
                            ("toolCallId", json::s(format!("t{}", call.id))),
                            ("title", json::s(name)),
                            ("rawInput", input),
                        ]),
                    );
                }
            }
            AgentEvent::ToolFinished { id, ok, summary, output, .. } => {
                let Some(p) = self.pending.remove(id) else { return };
                if p.name == "todo" {
                    return;
                }
                let raw = grok_output(&p.name, &p.args, summary, output, &self.children);
                self.write(
                    p.lane,
                    json::obj(vec![
                        ("sessionUpdate", json::s("tool_call_update")),
                        ("toolCallId", json::s(format!("t{id}"))),
                        ("status", json::s(if *ok { "completed" } else { "failed" })),
                        ("rawOutput", raw),
                    ]),
                );
            }
            AgentEvent::Todos(list) => {
                self.flush_text(0);
                let entries = list
                    .iter()
                    .map(|t| {
                        json::obj(vec![
                            ("content", json::s(t.text.clone())),
                            (
                                "status",
                                json::s(match t.state {
                                    TodoState::Pending => "pending",
                                    TodoState::Active => "in_progress",
                                    TodoState::Done => "completed",
                                }),
                            ),
                        ])
                    })
                    .collect();
                self.write(0, json::obj(vec![("sessionUpdate", json::s("plan")), ("entries", Value::Arr(entries))]));
            }
            AgentEvent::LaneStarted { lane, parent, title } => {
                let child = format!("{}-lane{lane}", self.main_id);
                self.children.insert(*lane, child.clone());
                self.open(*lane, &child, title);
                let parent_id = self.files.get(parent).map(|f| f.0.clone()).unwrap_or_else(|| self.main_id.clone());
                let meta_dir = self.project_dir.join(&parent_id).join("subagents").join(&child);
                if std::fs::create_dir_all(&meta_dir).is_ok() {
                    let meta = json::obj(vec![
                        ("child_session_id", json::s(child.clone())),
                        ("parent_session_id", json::s(parent_id)),
                    ]);
                    let _ = std::fs::write(meta_dir.join("meta.json"), meta.to_json());
                }
                self.write(
                    *lane,
                    json::obj(vec![
                        ("sessionUpdate", json::s("user_message_chunk")),
                        ("content", json::obj(vec![("text", json::s(title.clone()))])),
                    ]),
                );
            }
            AgentEvent::LaneFinished { lane, .. } => self.flush_text(*lane),
            AgentEvent::Idle => self.flush_all(),
            _ => {}
        }
    }
}

fn arg<'a>(args: &'a [(String, String)], key: &str) -> &'a str {
    args.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("")
}

fn int(args: &[(String, String)], key: &str) -> Option<i64> {
    arg(args, key).trim().parse().ok()
}

/// Our tool call in the vocabulary the activity reader understands.
fn grok_input(name: &str, args: &[(String, String)], children: &HashMap<LaneId, String>) -> (String, Value) {
    let path = if arg(args, "path").is_empty() { arg(args, "file_path") } else { arg(args, "path") };
    let _ = children;
    match name {
        "read_file" => {
            let mut pairs = vec![("target_file", json::s(path))];
            if let Some(start) = int(args, "start_line") {
                pairs.push(("offset", Value::Int(start)));
                if let Some(end) = int(args, "end_line") {
                    pairs.push(("limit", Value::Int((end - start + 1).max(1))));
                }
            }
            ("read_file".into(), json::obj(pairs))
        }
        "write_file" => (
            "write".into(),
            json::obj(vec![("file_path", json::s(path)), ("content", json::s(arg(args, "content")))]),
        ),
        "edit_file" => (
            "search_replace".into(),
            json::obj(vec![
                ("file_path", json::s(path)),
                ("old_string", json::s(arg(args, "old_string"))),
                ("new_string", json::s(arg(args, "new_string"))),
            ]),
        ),
        "grep" => (
            "grep".into(),
            json::obj(vec![("pattern", json::s(arg(args, "pattern"))), ("path", json::s(path))]),
        ),
        "run_command" => (
            "run_terminal_command".into(),
            json::obj(vec![("command", json::s(arg(args, "command")))]),
        ),
        "spawn_agent" => (
            "spawn_subagent".into(),
            json::obj(vec![("description", json::s(arg(args, "task")))]),
        ),
        other => (
            other.to_string(),
            Value::Obj(args.iter().map(|(k, v)| (k.clone(), json::s(v.clone()))).collect()),
        ),
    }
}

fn grok_output(
    name: &str,
    args: &[(String, String)],
    summary: &str,
    output: &str,
    children: &HashMap<LaneId, String>,
) -> Value {
    match name {
        "grep" => {
            let mut seen: Vec<&str> = Vec::new();
            for line in output.lines() {
                if let Some(path) = line.split(':').next() {
                    if line.matches(':').count() >= 2 && !seen.contains(&path) {
                        seen.push(path);
                    }
                }
            }
            json::obj(vec![(
                "file_matches",
                Value::Arr(seen.into_iter().map(|p| json::obj(vec![("path", json::s(p))])).collect()),
            )])
        }
        "read_file" => {
            // "path:a-b of N"
            let total = summary.rsplit(" of ").next().and_then(|n| n.trim().parse::<i64>().ok());
            match total {
                Some(n) if arg(args, "start_line").is_empty() => {
                    json::obj(vec![("FileContent", json::obj(vec![("total_lines", Value::Int(n))]))])
                }
                _ => Value::Null,
            }
        }
        "spawn_agent" => {
            // The most recently started child.
            let id = children.iter().max_by_key(|(l, _)| **l).map(|(_, id)| id.clone()).unwrap_or_default();
            json::obj(vec![("subagent_id", json::s(id))])
        }
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_tailable_stream() {
        let base = std::env::temp_dir().join(format!("code-scopelog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mut log = ScopeLog::new(&base, Path::new("/work/proj"), "s1", "fix the thing");
        log.user("fix the thing");
        log.observe(&AgentEvent::Delta { lane: 0, text: "Looking.".into() });
        log.observe(&AgentEvent::ToolStarted(ToolCall {
            id: 1,
            lane: 0,
            name: "grep".into(),
            title: "grep x".into(),
            args: vec![("pattern".into(), "x".into())],
        }));
        log.observe(&AgentEvent::ToolFinished {
            id: 1,
            ok: true,
            summary: "1 matches".into(),
            output: "src/a.rs:3:let x".into(),
            diff: None,
        });
        log.observe(&AgentEvent::LaneStarted { lane: 1, parent: 0, title: "check b".into() });
        log.observe(&AgentEvent::Idle);
        let dir = base.join("%2Fwork%2Fproj").join("s1");
        let lines = std::fs::read_to_string(dir.join("updates.jsonl")).unwrap();
        let kinds: Vec<String> = lines
            .lines()
            .map(|l| {
                let v = json::parse_depth(l.as_bytes(), 16).unwrap();
                v.get("params").unwrap().get("update").unwrap().get("sessionUpdate").unwrap().as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(kinds, ["user_message_chunk", "agent_message_chunk", "tool_call", "tool_call_update"]);
        assert!(lines.contains("file_matches"));
        assert!(dir.join("summary.json").exists());
        let meta = std::fs::read_to_string(dir.join("subagents/s1-lane1/meta.json")).unwrap();
        assert!(meta.contains("parent_session_id"));
        assert!(base.join("%2Fwork%2Fproj/s1-lane1/updates.jsonl").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
