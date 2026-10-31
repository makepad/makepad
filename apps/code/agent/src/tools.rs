//! The tools the model can call, their schemas, and the approval flow.
//! File paths go through the sandbox; commands run in the project root.

use super::diff;
use super::engine::{Engine, Reply};
use super::event::*;
use super::risk;
use super::engine::Background;
use super::output::{compact, Patterns, COMPACT_CHARS};
use super::runs::{RunManager, ShellExit};
use super::sandbox::Sandbox;
use makepad_ai_hub::local_llm::ToolSpec;
use makepad_regex::{ParseOptions, Regex};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

/// After a failure pattern: return once the output pauses this long...
const YIELD_QUIET: Duration = Duration::from_millis(300);
/// ...or this many lines followed it, or this long passed.
const YIELD_TAIL_LINES: usize = 40;
const YIELD_TAIL_MAX: Duration = Duration::from_secs(2);

/// How run_command's wait ended.
enum RunEnd {
    Exited(ShellExit),
    Interrupted,
    /// A failure pattern and the lines after it.
    Failure,
    /// yield_ms passed with the command still running.
    Waited,
}

/// Largest result text handed to the model (~2k tokens).
pub const RESULT_CHARS: usize = 7000;
const READ_DEFAULT_LINES: usize = 300;
const LIST_LIMIT: usize = 200;
const GREP_LIMIT: usize = 80;
const SKIP_DIRS: [&str; 5] = [".git", "target", "node_modules", ".claude", "wt.noindex"];

pub fn specs(sub: bool, read_only: bool) -> Vec<ToolSpec> {
    let t = |name: &str, desc: &str, params: &str| ToolSpec::new(name, desc, params);
    let all = vec![
        t("read_file", "Read a file with line numbers. Long files: pass start_line/end_line.",
          r#"{"type":"object","properties":{"path":{"type":"string"},"start_line":{"type":"integer"},"end_line":{"type":"integer"}},"required":["path"]}"#),
        t("list_dir", "List a directory (dirs end with /).",
          r#"{"type":"object","properties":{"path":{"type":"string","description":"default: project root"}}}"#),
        t("glob", "Find files by glob pattern, e.g. src/**/*.rs.",
          r#"{"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}"#),
        t("grep", "Search file contents with a regex; prints path:line:text.",
          r#"{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","description":"file or dir, default root"},"glob":{"type":"string","description":"only files matching, e.g. *.rs"},"ignore_case":{"type":"boolean"}},"required":["pattern"]}"#),
        t("edit_file", "Replace exact text in a file. old_string must match exactly once (add surrounding lines to make it unique) unless replace_all is true.",
          r#"{"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"]}"#),
        t("write_file", "Create a file or overwrite it with the full content.",
          r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}"#),
        t("run_command", "Run a shell command in the project root (build, test, etc.). Run it unpiped. Returns at exit with the code, or early: when a failure shows (error[E, error:, panicked at, FAILED, ...) or after yield_ms, the command keeps running as run N (run_output N, run_stop N). Output comes compact: errors, failing tests, summary.",
          r#"{"type":"object","properties":{"command":{"type":"string"},"yield_ms":{"type":"integer","description":"return after this if still running (default 10000)"},"early_yield_patterns":{"type":"array","items":{"type":"string"},"description":"more failure texts; /regex/ allowed"},"timeout_secs":{"type":"integer","description":"stop the command after this (default 600)"}},"required":["command"]}"#),
        t("run_output", "New output of a background run since the last call (compact), and whether it is still running or exited.",
          r#"{"type":"object","properties":{"id":{"type":"integer"},"since":{"type":"integer","description":"line number to start from (0 = all)"}},"required":["id"]}"#),
        t("run_stop", "Stop a background run (its whole process group).",
          r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#),
        t("git_status", "Short git status of the project.", r#"{"type":"object","properties":{}}"#),
        t("git_diff", "Git diff of uncommitted changes (optionally one path, or staged).",
          r#"{"type":"object","properties":{"path":{"type":"string"},"staged":{"type":"boolean"}}}"#),
        t("git_log", "Recent commits, one line each.",
          r#"{"type":"object","properties":{"count":{"type":"integer"}}}"#),
        t("todo", "Set the plan. items: one per line, '[ ] todo', '[>] doing', '[x] done'.",
          r#"{"type":"object","properties":{"items":{"type":"string"}},"required":["items"]}"#),
        t("spawn_agent", "Run a helper agent on one self-contained task with a fresh context; it returns a summary. Use for searches or investigations that would flood your context. toolset: read-only (default) or edit.",
          r#"{"type":"object","properties":{"task":{"type":"string"},"toolset":{"type":"string","enum":["read-only","edit"]}},"required":["task"]}"#),
        t("ask_user", "Ask the user a question and wait for the answer.",
          r#"{"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}"#),
    ];
    // Sub-agents cannot spawn further agents; the running apps are the main agent's (remote.rs).
    let all: Vec<ToolSpec> = if sub { all.into_iter().filter(|s| s.name != "spawn_agent").collect() } else { all.into_iter().chain(super::remote::specs()).chain(super::remote::attached_specs()).collect() };
    if read_only {
        all.into_iter().filter(|s| !matches!(s.name.as_str(), "edit_file" | "write_file" | "run_command" | "run_stop" | "start_app" | "build_app" | "run_app" | "stop_app")).collect()
    } else {
        all
    }
}

fn arg<'a>(args: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| args.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()))
}

fn flag(args: &[(String, String)], key: &str) -> bool {
    arg(args, &[key]).is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
}

fn num(args: &[(String, String)], key: &str) -> Option<usize> {
    arg(args, &[key]).and_then(|v| v.trim().parse().ok())
}

/// A list argument: a JSON array of strings, else one item per line.
fn list(args: &[(String, String)], key: &str) -> Vec<String> {
    let Some(v) = arg(args, &[key]).map(str::trim).filter(|v| !v.is_empty()) else { return Vec::new() };
    if let Some(inner) = v.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        if let Ok(makepad_strict_json::Value::Arr(items)) = makepad_strict_json::parse(v.as_bytes()) {
            return items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect();
        }
        // Not quite JSON (a regex's `\d` left unescaped, say): split by hand.
        return inner.split("\",").map(|i| i.trim().trim_matches('"').to_string()).filter(|i| !i.is_empty()).collect();
    }
    v.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Keep the head and the tail of `text` when it is longer than `max` chars.
pub fn truncate_middle(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let half = max / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(total - half).collect();
    // Prefer to cut on line boundaries.
    let head = head.rfind('\n').map(|i| &head[..=i]).unwrap_or(&head);
    let tail = tail.find('\n').map(|i| &tail[i + 1..]).unwrap_or(&tail);
    let omitted = total - head.chars().count() - tail.chars().count();
    format!("{head}\n[... {omitted} characters omitted ...]\n{tail}")
}

/// `*`, `**` and `?` globbing over `/`-separated relative paths.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn m(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') => {
                if p.get(1) == Some(&b'*') {
                    let rest = if p.get(2) == Some(&b'/') { &p[3..] } else { &p[2..] };
                    (0..=s.len()).any(|i| m(rest, &s[i..]))
                } else {
                    let rest = &p[1..];
                    for i in 0..=s.len() {
                        if m(rest, &s[i..]) {
                            return true;
                        }
                        if i < s.len() && s[i] == b'/' {
                            break;
                        }
                    }
                    false
                }
            }
            Some(b'?') => !s.is_empty() && s[0] != b'/' && m(&p[1..], &s[1..]),
            Some(c) => !s.is_empty() && s[0] == *c && m(&p[1..], &s[1..]),
        }
    }
    // A pattern without '/' matches the file name anywhere.
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return m(pattern.as_bytes(), name.as_bytes());
    }
    m(pattern.as_bytes(), path.as_bytes())
}

fn walk(sb: &Sandbox, dir: &Path, out: &mut Vec<std::path::PathBuf>, limit: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= limit {
            return;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(sb, &e.path(), out, limit);
        } else {
            out.push(e.path());
        }
    }
}

/// Numbered lines `start..=end`, cut at whole lines once `budget` chars are
/// used. Returns (text, total lines, last line shown).
fn numbered(text: &str, start: usize, end: usize, budget: usize) -> (String, usize, usize) {
    let total = text.lines().count();
    let mut out = String::new();
    let mut last = start.saturating_sub(1);
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if n < start {
            continue;
        }
        if n > end {
            break;
        }
        let shown: String = if line.chars().count() > 400 { line.chars().take(400).collect::<String>() + " [line cut]" } else { line.to_string() };
        let row = format!("{n:>5}\t{shown}\n");
        // Always show at least one line; never cut one in half.
        if !out.is_empty() && out.len() + row.len() > budget {
            break;
        }
        out.push_str(&row);
        last = n;
    }
    (out, total, last)
}

pub(super) enum Verdict {
    Yes,
    No,
    Hunks(Vec<usize>),
}

pub(super) struct Outcome {
    pub ok: bool,
    pub text: String,
    pub summary: String,
    pub diff: Option<Diff>,
}

impl Outcome {
    fn ok(text: impl Into<String>, summary: impl Into<String>) -> Outcome {
        Outcome { ok: true, text: text.into(), summary: summary.into(), diff: None }
    }
    fn err(text: impl Into<String>) -> Outcome {
        let text = text.into();
        Outcome { ok: false, summary: text.lines().next().unwrap_or("").to_string(), text, diff: None }
    }
}

impl Engine {
    /// Put an edit back as it was before the tool ran. A created file is removed.
    pub(super) fn undo_edit(&mut self, id: ToolId) {
        let Some((path, before)) = self.undo.remove(&id) else {
            self.emit(AgentEvent::Error("nothing to undo for that edit".into()));
            return;
        };
        let res = match &before {
            Some(text) => std::fs::write(&path, text),
            None => std::fs::remove_file(&path),
        };
        match res {
            Ok(()) => {
                self.created.remove(&path);
                let rel = Sandbox::new(self.root()).rel(&path);
                self.emit(AgentEvent::FileTouched { path: rel, how: Touch::Edited });
            }
            Err(e) => self.emit(AgentEvent::Error(format!("undo failed: {e}"))),
        }
    }

    fn allow_file(&self) -> Option<PathBuf> {
        let dir = self.cfg.session_dir.as_ref()?.parent()?.join("allow");
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.root().to_string_lossy().bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        Some(dir.join(format!("{h:016x}.txt")))
    }

    pub(super) fn load_repo_allow(&mut self) {
        if let Some(text) = self.allow_file().and_then(|f| std::fs::read_to_string(f).ok()) {
            self.repo_allow = text.lines().filter(|l| !l.is_empty()).map(|l| l.to_string()).collect();
        }
    }

    fn remember_for_repo(&mut self, key: String) {
        if let Some(file) = self.allow_file() {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let mut all: Vec<&String> = self.repo_allow.iter().collect();
            all.push(&key);
            let body: Vec<String> = all.iter().map(|k| k.replace('\n', " ")).collect();
            let _ = std::fs::write(file, body.join("\n") + "\n");
        }
        self.repo_allow.insert(key);
    }

    fn sandbox(&self) -> Sandbox {
        Sandbox::new(self.root())
    }

    pub(super) fn verdict(&mut self, id: ToolId, prompt: String, key: String, trusted: bool) -> Verdict {
        match self.mode {
            ApprovalMode::Auto => return Verdict::Yes,
            ApprovalMode::Allowlist if trusted => return Verdict::Yes,
            _ => {}
        }
        if self.allow.contains(&key) || self.repo_allow.contains(&key) {
            return Verdict::Yes;
        }
        self.emit(AgentEvent::ApprovalNeeded { id, prompt });
        match self.await_reply(id) {
            Some(Reply::Decision(Decision::Allow)) => Verdict::Yes,
            Some(Reply::Decision(Decision::AllowAlways)) => {
                self.allow.insert(key);
                Verdict::Yes
            }
            Some(Reply::Decision(Decision::AllowRepo)) => {
                self.remember_for_repo(key);
                Verdict::Yes
            }
            Some(Reply::Decision(Decision::Hunks(h))) => Verdict::Hunks(h),
            _ => Verdict::No,
        }
    }

    /// Run one tool call from the model. Emits the tool's events and returns
    /// the (text, is_error) pair the model gets back.
    pub(super) fn run_tool(&mut self, lane: LaneId, name: &str, args: &[(String, String)]) -> (String, bool) {
        let id = self.fresh_tool_id();
        let title = title_of(name, args);
        self.emit(AgentEvent::ToolStarted(ToolCall {
            id,
            lane,
            name: name.to_string(),
            title,
            args: args.to_vec(),
        }));
        let outcome = self.dispatch(id, lane, name, args);
        let text = truncate_middle(&outcome.text, RESULT_CHARS);
        self.emit(AgentEvent::ToolFinished {
            id,
            ok: outcome.ok,
            summary: outcome.summary,
            output: text.clone(),
            diff: outcome.diff,
        });
        (text, !outcome.ok)
    }

    fn dispatch(&mut self, id: ToolId, lane: LaneId, name: &str, args: &[(String, String)]) -> Outcome {
        let sb = self.sandbox();
        match name {
            "read_file" => self.read_file(&sb, args),
            "list_dir" => self.list_dir(&sb, args),
            "glob" => self.glob(&sb, args),
            "grep" => self.grep(&sb, args),
            "edit_file" => self.edit_file(id, &sb, args),
            "write_file" => self.write_file(id, &sb, args),
            "run_command" => self.run_command(id, args),
            "run_output" => self.run_output(args),
            "run_stop" => self.run_stop(id, args),
            "git_status" => self.git(vec!["status".into(), "--short".into(), "--branch".into()]),
            "git_diff" => {
                let mut a = vec!["diff".to_string()];
                if flag(args, "staged") {
                    a.push("--staged".into());
                }
                if let Some(p) = arg(args, &["path"]).filter(|p| !p.is_empty()) {
                    match sb.resolve(p) {
                        Ok(abs) => {
                            a.push("--".into());
                            a.push(sb.rel(&abs));
                        }
                        Err(e) => return Outcome::err(e),
                    }
                }
                self.git(a)
            }
            "git_log" => {
                let n = num(args, "count").unwrap_or(10).clamp(1, 50).to_string();
                self.git(vec!["log".into(), "--oneline".into(), "-n".into(), n])
            }
            "todo" => self.todo(args),
            "spawn_agent" => {
                let task = arg(args, &["task"]).unwrap_or("").trim().to_string();
                if task.is_empty() {
                    return Outcome::err("missing 'task'");
                }
                let read_only = !matches!(arg(args, &["toolset"]), Some("edit"));
                let (ok, text) = self.run_sub_agent(lane, &task, read_only);
                let summary = text.lines().next().unwrap_or("").chars().take(100).collect::<String>();
                Outcome { ok, text, summary, diff: None }
            }
            n if super::remote::is_tool(n) => self.remote_tool(id, n, args),
            "ask_user" => {
                let q = arg(args, &["question"]).unwrap_or("").to_string();
                self.emit(AgentEvent::Question { id, text: q });
                match self.await_reply(id) {
                    Some(Reply::Answer(a)) => Outcome::ok(a.clone(), format!("answered: {}", a.lines().next().unwrap_or(""))),
                    _ => Outcome::err("the user did not answer"),
                }
            }
            other => Outcome::err(format!("unknown tool '{other}'")),
        }
    }

    fn read_file(&mut self, sb: &Sandbox, args: &[(String, String)]) -> Outcome {
        let Some(path) = arg(args, &["path", "file_path"]) else { return Outcome::err("missing 'path'") };
        let abs = match sb.resolve(path) {
            Ok(p) => p,
            Err(e) => return Outcome::err(e),
        };
        let bytes = match std::fs::read(&abs) {
            Ok(b) => b,
            Err(e) => return Outcome::err(format!("cannot read {path}: {e}")),
        };
        let Ok(text) = String::from_utf8(bytes) else { return Outcome::err(format!("{path} is not UTF-8 text")) };
        let rel = sb.rel(&abs);
        let start = num(args, "start_line").unwrap_or(1).max(1);
        let end = num(args, "end_line").unwrap_or(start + READ_DEFAULT_LINES - 1).max(start);
        let (body, total, shown_end) = numbered(&text, start, end, RESULT_CHARS - 300);
        if start > total.max(1) {
            return Outcome::err(format!("{rel} has only {total} lines"));
        }
        let mut out = format!("{rel} ({total} lines)\n{body}");
        if shown_end < total {
            out.push_str(&format!("[lines {}-{total} not shown: call read_file with start_line={}]\n", shown_end + 1, shown_end + 1));
        }
        self.emit(AgentEvent::FileTouched { path: rel.clone(), how: Touch::Read });
        Outcome::ok(out, format!("{rel}:{start}-{shown_end} of {total}"))
    }

    fn list_dir(&mut self, sb: &Sandbox, args: &[(String, String)]) -> Outcome {
        let path = arg(args, &["path"]).filter(|p| !p.is_empty()).unwrap_or(".");
        let abs = match sb.resolve(path) {
            Ok(p) => p,
            Err(e) => return Outcome::err(e),
        };
        let rd = match std::fs::read_dir(&abs) {
            Ok(r) => r,
            Err(e) => return Outcome::err(format!("cannot list {path}: {e}")),
        };
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) { format!("{n}/") } else { n }
            })
            .collect();
        names.sort();
        let total = names.len();
        names.truncate(LIST_LIMIT);
        let mut out = names.join("\n");
        if total > LIST_LIMIT {
            out.push_str(&format!("\n[{} more]", total - LIST_LIMIT));
        }
        Outcome::ok(out, format!("{total} entries in {}", sb.rel(&abs)))
    }

    fn glob(&mut self, sb: &Sandbox, args: &[(String, String)]) -> Outcome {
        let Some(pattern) = arg(args, &["pattern"]) else { return Outcome::err("missing 'pattern'") };
        let mut files = Vec::new();
        walk(sb, sb.root(), &mut files, 50_000);
        let mut hits: Vec<String> = files.iter().map(|f| sb.rel(f)).filter(|r| glob_match(pattern, r)).collect();
        let total = hits.len();
        hits.truncate(LIST_LIMIT);
        let mut out = hits.join("\n");
        if total > LIST_LIMIT {
            out.push_str(&format!("\n[{} more]", total - LIST_LIMIT));
        }
        if total == 0 {
            out = "no files match".into();
        }
        self.emit(AgentEvent::FileTouched { path: pattern.to_string(), how: Touch::Searched });
        Outcome::ok(out, format!("{total} files"))
    }

    fn grep(&mut self, sb: &Sandbox, args: &[(String, String)]) -> Outcome {
        let Some(pattern) = arg(args, &["pattern"]) else { return Outcome::err("missing 'pattern'") };
        let re = match Regex::new_with_options(
            pattern,
            ParseOptions { ignore_case: flag(args, "ignore_case"), ..ParseOptions::default() },
        ) {
            Ok(r) => r,
            Err(e) => return Outcome::err(format!("bad regex: {e:?}")),
        };
        let base = match sb.resolve(arg(args, &["path"]).filter(|p| !p.is_empty()).unwrap_or(".")) {
            Ok(p) => p,
            Err(e) => return Outcome::err(e),
        };
        let only = arg(args, &["glob", "include"]).filter(|g| !g.is_empty());
        let mut files = Vec::new();
        if base.is_file() {
            files.push(base.clone());
        } else {
            walk(sb, &base, &mut files, 50_000);
        }
        let mut out = String::new();
        let mut hits = 0usize;
        'files: for f in files {
            let rel = sb.rel(&f);
            if only.is_some_and(|g| !glob_match(g, &rel)) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&f) else { continue };
            if meta.len() > 2_000_000 {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            for (i, line) in text.lines().enumerate() {
                if re.earliest_match(line).is_some() {
                    hits += 1;
                    let shown: String = line.trim().chars().take(200).collect();
                    out.push_str(&format!("{rel}:{}:{shown}\n", i + 1));
                    if hits >= GREP_LIMIT {
                        out.push_str("[stopped at the match limit; narrow the pattern or path]\n");
                        break 'files;
                    }
                }
            }
        }
        if hits == 0 {
            out = "no matches".into();
        }
        self.emit(AgentEvent::FileTouched { path: pattern.to_string(), how: Touch::Searched });
        Outcome::ok(out, format!("{hits} matches"))
    }

    fn edit_file(&mut self, id: ToolId, sb: &Sandbox, args: &[(String, String)]) -> Outcome {
        let Some(path) = arg(args, &["path", "file_path"]) else { return Outcome::err("missing 'path'") };
        let old = arg(args, &["old_string"]).unwrap_or("");
        let new = arg(args, &["new_string"]).unwrap_or("");
        let abs = match sb.resolve(path) {
            Ok(p) => p,
            Err(e) => return Outcome::err(e),
        };
        let before = match std::fs::read_to_string(&abs) {
            Ok(t) => t,
            Err(e) => return Outcome::err(format!("cannot read {path}: {e}")),
        };
        let (after, note) = match edit_text(&before, old, new, flag(args, "replace_all")) {
            Ok(r) => r,
            Err(e) => return Outcome::err(e),
        };
        let mut outcome = self.write_checked(id, sb, &abs, &before, after, Touch::Edited);
        if let (true, Some(note)) = (outcome.ok, note) {
            outcome.text.push_str(&format!(" ({note})"));
        }
        outcome
    }

    fn write_file(&mut self, id: ToolId, sb: &Sandbox, args: &[(String, String)]) -> Outcome {
        let Some(path) = arg(args, &["path", "file_path"]) else { return Outcome::err("missing 'path'") };
        let content = arg(args, &["content"]).unwrap_or("");
        let abs = match sb.resolve(path) {
            Ok(p) => p,
            Err(e) => return Outcome::err(e),
        };
        let existed = abs.exists();
        let before = if existed { std::fs::read_to_string(&abs).unwrap_or_default() } else { String::new() };
        let mut after = content.to_string();
        if !after.ends_with('\n') {
            after.push('\n');
        }
        let how = if existed { Touch::Edited } else { Touch::Created };
        self.write_checked(id, sb, &abs, &before, after, how)
    }

    /// Show the diff, ask when the mode wants it, write what was approved.
    fn write_checked(&mut self, id: ToolId, sb: &Sandbox, abs: &Path, before: &str, after: String, how: Touch) -> Outcome {
        let rel = sb.rel(abs);
        let d = diff::diff(&rel, before, &after);
        if d.hunks.is_empty() {
            return Outcome::ok("no change: the file already has this content", format!("{rel} unchanged"));
        }
        let (add, del) = diff::counts(&d);
        let prompt = format!("{} {rel} (+{add} -{del})", if how == Touch::Created { "create" } else { "edit" });
        let final_text = match self.verdict(id, prompt, format!("edit:{rel}"), true) {
            Verdict::Yes => after,
            Verdict::No => return Outcome::err("the user denied this edit"),
            Verdict::Hunks(h) => {
                if h.is_empty() {
                    return Outcome::err("the user rejected every hunk of this edit");
                }
                diff::apply_selected(before, &after, &h)
            }
        };
        if let Some(parent) = abs.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Kept so the edit can be undone after the fact.
        self.undo.insert(id, (abs.to_path_buf(), if how == Touch::Created { None } else { Some(before.to_string()) }));
        if how == Touch::Created {
            self.created.insert(abs.to_path_buf());
        }
        if let Err(e) = std::fs::write(abs, &final_text) {
            return Outcome::err(format!("cannot write {rel}: {e}"));
        }
        let applied = diff::diff(&rel, before, &final_text);
        let (a, r) = diff::counts(&applied);
        self.emit(AgentEvent::FileTouched { path: rel.clone(), how });
        let partial = if applied.hunks.len() < d.hunks.len() {
            format!(" (user applied {} of {} hunks)", applied.hunks.len(), d.hunks.len())
        } else {
            String::new()
        };
        Outcome {
            ok: true,
            text: format!("{} {rel}: +{a} -{r}{partial}", if how == Touch::Created { "created" } else { "edited" }),
            summary: format!("{rel} +{a} -{r}"),
            diff: Some(applied),
        }
    }

    fn git(&mut self, a: Vec<String>) -> Outcome {
        match Process::new("git").args(&a).current_dir(self.root()).stdin(Stdio::null()).output() {
            Ok(o) => {
                let mut text = String::from_utf8_lossy(&o.stdout).to_string();
                text.push_str(&String::from_utf8_lossy(&o.stderr));
                if text.trim().is_empty() {
                    text = "(nothing)".into();
                }
                let summary = format!("git {}", a.first().map(String::as_str).unwrap_or(""));
                if o.status.success() { Outcome::ok(text, summary) } else { Outcome::err(text) }
            }
            Err(e) => Outcome::err(format!("cannot run git: {e}")),
        }
    }

    fn todo(&mut self, args: &[(String, String)]) -> Outcome {
        let items = arg(args, &["items", "todos"]).unwrap_or("");
        let list = parse_todos(items);
        let n = list.len();
        self.emit(AgentEvent::Todos(list));
        Outcome::ok("plan updated", format!("{n} items"))
    }

    fn run_command(&mut self, id: ToolId, args: &[(String, String)]) -> Outcome {
        let Some(command) = arg(args, &["command", "cmd"]).map(str::trim).filter(|c| !c.is_empty()) else {
            return Outcome::err("missing 'command'");
        };
        let limit = Duration::from_secs(num(args, "timeout_secs").unwrap_or(600).clamp(1, 3600) as u64);
        let yield_after = Duration::from_millis(num(args, "yield_ms").unwrap_or(10_000).clamp(1000, 120_000) as u64);
        let patterns = match Patterns::new(&list(args, "early_yield_patterns")) {
            Ok(p) => p,
            Err(e) => return Outcome::err(e),
        };
        let (trusted, prompt) = match risk::classify(command, self.root(), &self.created) {
            risk::Risk::Safe => (true, format!("run: {command}")),
            risk::Risk::Ask(why) => (false, format!("run: {command}\n({why})")),
        };
        match self.verdict(id, prompt, format!("cmd:{command}"), trusted) {
            Verdict::Yes | Verdict::Hunks(_) => {}
            Verdict::No => return Outcome::err("the user denied this command"),
        }
        self.emit(AgentEvent::FileTouched { path: command.to_string(), how: Touch::Ran });
        let m = RunManager::global();
        let root = self.root().clone();
        let run = match m.start_shell(command, &root, self.owner, limit) {
            Ok(run) => run,
            Err(e) => return Outcome::err(e),
        };
        let started = Instant::now();
        let mut lines: Vec<String> = Vec::new();
        let mut cursor = 0u64;
        let mut pending = String::new();
        let mut last_flush = Instant::now();
        let mut last_line = Instant::now();
        // The failure seen: its pattern, when, and how many lines came before it.
        let mut hit: Option<(String, f32, usize, Instant)> = None;
        let end = loop {
            std::thread::sleep(Duration::from_millis(25));
            self.pump_commands();
            // The end is read before the lines, so no line is missed.
            let exit = m.shell(run).and_then(|v| v.exit);
            let (new, next) = m.shell_lines(run, cursor).unwrap_or_default();
            cursor = next.max(cursor);
            if !new.is_empty() {
                last_line = Instant::now();
            }
            for line in new {
                if hit.is_none() {
                    if let Some(p) = patterns.find(&line) {
                        hit = Some((p, started.elapsed().as_secs_f32(), lines.len(), Instant::now()));
                    }
                }
                pending.push_str(&line);
                pending.push('\n');
                lines.push(line);
            }
            if !pending.is_empty() && last_flush.elapsed() > Duration::from_millis(100) {
                self.emit(AgentEvent::ToolOutput { id, text: std::mem::take(&mut pending) });
                last_flush = Instant::now();
            }
            if let Some(exit) = exit {
                break RunEnd::Exited(exit);
            }
            if self.interrupted || self.shutdown {
                let _ = m.stop_shell(run, "interrupted");
                m.wait_shell(run, Duration::from_millis(3500));
                let (rest, _) = m.shell_lines(run, cursor).unwrap_or_default();
                for line in rest {
                    pending.push_str(&line);
                    pending.push('\n');
                    lines.push(line);
                }
                break RunEnd::Interrupted;
            }
            match &hit {
                // After a failure, the block that follows it: until the
                // output pauses, 40 more lines, or 2 s.
                Some((_, _, at, when)) => {
                    if lines.len() - at >= YIELD_TAIL_LINES || last_line.elapsed() >= YIELD_QUIET || when.elapsed() >= YIELD_TAIL_MAX {
                        break RunEnd::Failure;
                    }
                }
                None if started.elapsed() >= yield_after => break RunEnd::Waited,
                None => {}
            }
        };
        if !pending.is_empty() {
            self.emit(AgentEvent::ToolOutput { id, text: pending });
        }
        let secs = started.elapsed().as_secs_f32();
        let text = lines.join("\n");
        let more = format!("run_output {run} since=0");
        let body = compact(&text, COMPACT_CHARS - 300, Some(&more));
        match end {
            RunEnd::Exited(exit) => {
                self.ended_seen.insert(run);
                let head = match &exit {
                    ShellExit::Code(c) => format!("exit code {c} after {secs:.1} s"),
                    ShellExit::Stopped(why) if why.starts_with("time limit") => format!("[command timed out after {secs:.0} s]"),
                    other => format!("[command {} after {secs:.1} s]", other.text()),
                };
                let summary = match &exit {
                    ShellExit::Code(c) => format!("exit {c}"),
                    ShellExit::Stopped(why) if why.starts_with("time limit") => "timed out".into(),
                    other => other.text(),
                };
                Outcome { ok: exit.success(), text: format!("{head}\n{body}"), summary, diff: None }
            }
            RunEnd::Interrupted => {
                self.ended_seen.insert(run);
                Outcome { ok: false, text: format!("[command interrupted after {secs:.0} s]\n{body}"), summary: "interrupted".into(), diff: None }
            }
            RunEnd::Failure | RunEnd::Waited => {
                self.read_marks.insert(run, cursor);
                self.background.push(Background { tool: id, run, shown: cursor });
                let (head, summary, pattern, ok) = match hit {
                    Some((p, at, _, _)) => (
                        format!("[early yield: pattern `{p}` matched at {at:.1} s; the command is still running as run {run} -- run_output {run} / run_stop {run}]"),
                        format!("early yield `{p}` · running in background (run {run})"),
                        Some(p),
                        false,
                    ),
                    None => (
                        format!("[still running after {secs:.1} s as run {run} -- run_output {run} for more, run_stop {run} to end it]"),
                        format!("running in background (run {run})"),
                        None,
                        true,
                    ),
                };
                self.emit(AgentEvent::RunBackground { id, run, pattern });
                Outcome { ok, text: format!("{head}\n{body}"), summary, diff: None }
            }
        }
    }

    /// run_output: what a run printed since the last look, and its state.
    fn run_output(&mut self, args: &[(String, String)]) -> Outcome {
        let Some(run) = num(args, "id").or_else(|| num(args, "run")).map(|n| n as u64) else { return Outcome::err("missing 'id' (the run number)") };
        let m = RunManager::global();
        let Some(mut view) = m.shell(run) else { return Outcome::err(format!("no run {run}")) };
        let since = num(args, "since").map(|n| n as u64).unwrap_or_else(|| self.read_marks.get(&run).copied().unwrap_or(0));
        // Nothing new from a running command: give it a moment.
        if view.exit.is_none() && view.lines <= since {
            let end = Instant::now() + Duration::from_secs(2);
            while Instant::now() < end && view.exit.is_none() && view.lines <= since {
                std::thread::sleep(Duration::from_millis(50));
                self.pump_commands();
                view = m.shell(run).unwrap_or(view);
            }
        }
        let (lines, next) = m.shell_lines(run, since).unwrap_or_default();
        self.read_marks.insert(run, next);
        let secs = view.elapsed.as_secs_f32();
        let state = match &view.exit {
            None => format!("running for {secs:.1} s"),
            Some(exit) => {
                self.ended_seen.insert(run);
                format!("{} after {secs:.1} s", exit.text())
            }
        };
        let first = next - lines.len() as u64;
        let head = if lines.is_empty() {
            format!("run {run}: {state}; no new output ({next} lines in all)")
        } else {
            format!("run {run}: {state}; lines {}-{next} of {next}", first + 1)
        };
        let body = compact(&lines.join("\n"), COMPACT_CHARS - 300, Some(&format!("run_output {run} since={first}")));
        let summary = state.clone();
        Outcome { ok: true, text: if body.is_empty() { head } else { format!("{head}\n{body}") }, summary, diff: None }
    }

    /// run_stop: end a run (asks first for one this agent did not start).
    fn run_stop(&mut self, id: ToolId, args: &[(String, String)]) -> Outcome {
        let Some(run) = num(args, "id").or_else(|| num(args, "run")).map(|n| n as u64) else { return Outcome::err("missing 'id' (the run number)") };
        let m = RunManager::global();
        let Some(view) = m.shell(run) else { return Outcome::err(format!("no run {run}")) };
        if let risk::Risk::Ask(why) = risk::run_tool("run_stop", view.owner == self.owner) {
            let prompt = format!("stop run {run}: {}\n({why})", super::runs::cut_command(&view.command, 80));
            if let Verdict::No = self.verdict(id, prompt, format!("run_stop:{run}"), false) {
                return Outcome::err("the user denied this");
            }
        }
        if let Some(exit) = &view.exit {
            self.ended_seen.insert(run);
            return Outcome::ok(format!("run {run} had already ended: {}", exit.text()), exit.text());
        }
        if let Err(e) = m.stop_shell(run, "run_stop") {
            return Outcome::err(e);
        }
        let view = m.wait_shell(run, Duration::from_millis(3500)).unwrap_or(view);
        self.ended_seen.insert(run);
        let how = view.exit.as_ref().map(|e| e.text()).unwrap_or_else(|| "stop sent; still ending".into());
        Outcome::ok(format!("run {run}: {how} after {:.1} s", view.elapsed.as_secs_f32()), how)
    }
}

/// Terminal output as plain lines: escape sequences removed, and a carriage
/// return starts the line over (so progress that redraws itself counts once).
pub fn clean_terminal(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                cur.clear();
            }
            '\n' => lines.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    let mut out = lines.join("\n");
    if raw.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Exactly-once replacement; errors say how many matches there were.
pub fn replace_unique(text: &str, old: &str, new: &str, all: bool) -> Result<String, String> {
    if old.is_empty() {
        return Err("old_string is empty".into());
    }
    let n = text.matches(old).count();
    match (n, all) {
        (0, _) => Err("old_string was not found. Read the file again and copy the text exactly, including indentation.".into()),
        (1, _) => Ok(text.replacen(old, new, 1)),
        (n, true) => {
            let _ = n;
            Ok(text.replace(old, new))
        }
        (n, false) => Err(format!(
            "old_string matches {n} places. Add more surrounding lines to make it unique, or pass replace_all=true."
        )),
    }
}

/// `@path` mentions in a request: each one that names a text file inside the
/// project is appended (numbered, cut to a budget) so the model starts with
/// it instead of spending a round on read_file. Other `@words` stay as typed.
pub fn expand_mentions(root: &Path, text: &str) -> String {
    const PER_FILE: usize = 5000;
    const ALL: usize = 12000;
    let sb = Sandbox::new(root);
    let mut seen: Vec<String> = Vec::new();
    let mut extra = String::new();
    for word in text.split_whitespace() {
        let Some(path) = word.strip_prefix('@') else { continue };
        let path = path.trim_end_matches(|c: char| matches!(c, ',' | '.' | ';' | ':' | ')' | '?' | '!'));
        if path.is_empty() || seen.iter().any(|p| p == path) || extra.len() > ALL {
            continue;
        }
        let Ok(abs) = sb.resolve(path) else { continue };
        let Ok(body) = std::fs::read_to_string(&abs) else { continue };
        seen.push(path.to_string());
        let (shown, total, last) = numbered(&body, 1, usize::MAX, PER_FILE);
        extra.push_str(&format!("\n\n[@{path}: {total} lines]\n{shown}"));
        if last < total {
            extra.push_str(&format!("[lines {}-{total} not shown: read_file start_line={}]\n", last + 1, last + 1));
        }
    }
    if extra.is_empty() { text.to_string() } else { format!("{text}{extra}") }
}

/// `replace_unique`, with two helps for a model that copies text loosely:
/// when the exact text is missing but exactly one block matches line by line
/// ignoring indentation and trailing spaces, that block is replaced (the new
/// text is re-indented the same way) and a note says so; when nothing
/// matches, the error names where the first line of old_string does appear.
pub fn edit_text(text: &str, old: &str, new: &str, all: bool) -> Result<(String, Option<String>), String> {
    match replace_unique(text, old, new, all) {
        Ok(t) => return Ok((t, None)),
        Err(e) if !e.contains("not found") => return Err(e),
        Err(_) => {}
    }
    let file: Vec<&str> = text.split_inclusive('\n').collect();
    let old_lines: Vec<&str> = old.trim_matches('\n').lines().collect();
    let want: Vec<&str> = old_lines.iter().map(|l| l.trim()).collect();
    if want.iter().all(|l| l.is_empty()) {
        return Err("old_string is empty".into());
    }
    let same = |at: usize| (0..want.len()).all(|k| file.get(at + k).is_some_and(|l| l.trim() == want[k]));
    let hits: Vec<usize> = (0..file.len()).filter(|&at| same(at)).collect();
    if hits.len() == 1 {
        let at = hits[0];
        let file_indent = leading_ws(file[at]);
        let old_indent = leading_ws(old_lines[0]);
        let new_lines: Vec<String> = new
            .trim_matches('\n')
            .lines()
            .map(|l| {
                if l.trim().is_empty() {
                    String::new()
                } else if let Some(extra) = file_indent.strip_prefix(old_indent) {
                    format!("{extra}{l}")
                } else if let Some(extra) = old_indent.strip_prefix(file_indent) {
                    l.strip_prefix(extra).unwrap_or(l).to_string()
                } else {
                    l.to_string()
                }
            })
            .collect();
        let last = file[at + want.len() - 1];
        let mut out: String = file[..at].concat();
        for (i, l) in new_lines.iter().enumerate() {
            out.push_str(l);
            if i + 1 < new_lines.len() || last.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push_str(&file[at + want.len()..].concat());
        return Ok((out, Some(format!("old_string matched at line {} ignoring indentation; copy text exactly next time", at + 1))));
    }
    if hits.len() > 1 {
        return Err(format!(
            "old_string matches {} places once indentation is ignored (lines {}). Add more surrounding lines.",
            hits.len(),
            hits.iter().take(4).map(|h| (h + 1).to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    let first = want.iter().find(|l| !l.is_empty()).copied().unwrap_or("");
    let near: Vec<usize> = (0..file.len()).filter(|&i| file[i].contains(first)).take(3).collect();
    Err(if near.is_empty() {
        format!(
            "old_string was not found, not even its first line `{}`. read_file the area again and copy the text exactly.",
            first.chars().take(80).collect::<String>()
        )
    } else {
        format!(
            "old_string was not found, but its first line appears at line {}. read_file that range and copy the following lines exactly (a later line differs).",
            near.iter().map(|n| (n + 1).to_string()).collect::<Vec<_>>().join(", ")
        )
    })
}

fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

pub fn parse_todos(items: &str) -> Vec<Todo> {
    items
        .lines()
        .filter_map(|l| {
            let l = l.trim().trim_start_matches(['-', '*']).trim();
            if l.is_empty() {
                return None;
            }
            let (state, rest) = if let Some(r) = l.strip_prefix("[x]").or_else(|| l.strip_prefix("[X]")) {
                (TodoState::Done, r)
            } else if let Some(r) = l.strip_prefix("[>]").or_else(|| l.strip_prefix("[~]")) {
                (TodoState::Active, r)
            } else if let Some(r) = l.strip_prefix("[ ]") {
                (TodoState::Pending, r)
            } else {
                (TodoState::Pending, l)
            };
            Some(Todo { text: rest.trim().to_string(), state })
        })
        .collect()
}

fn title_of(name: &str, args: &[(String, String)]) -> String {
    let p = arg(args, &["path", "file_path"]).unwrap_or("");
    match name {
        "read_file" => match (num(args, "start_line"), num(args, "end_line")) {
            (Some(a), Some(b)) => format!("read {p}:{a}-{b}"),
            (Some(a), None) => format!("read {p}:{a}-"),
            _ => format!("read {p}"),
        },
        "list_dir" => format!("ls {}", if p.is_empty() { "." } else { p }),
        "glob" => format!("glob {}", arg(args, &["pattern"]).unwrap_or("")),
        "grep" => format!("grep {}", arg(args, &["pattern"]).unwrap_or("")),
        "edit_file" => format!("edit {p}"),
        "write_file" => format!("write {p}"),
        "run_command" => format!("$ {}", arg(args, &["command", "cmd"]).unwrap_or("")),
        "run_output" => format!("output of run {}", arg(args, &["id", "run"]).unwrap_or("")),
        "run_stop" => format!("stop run {}", arg(args, &["id", "run"]).unwrap_or("")),
        "git_status" => "git status".into(),
        "git_diff" => format!("git diff {p}").trim_end().to_string(),
        "git_log" => "git log".into(),
        "todo" => "plan".into(),
        "spawn_agent" => format!("agent: {}", arg(args, &["task"]).unwrap_or("").chars().take(60).collect::<String>()),
        "ask_user" => "ask".into(),
        "start_app" => format!("start {}", arg(args, &["app"]).unwrap_or("")),
        "build_app" => format!("build {}", arg(args, &["app"]).unwrap_or("")),
        "run_app" => format!("run {} {}", arg(args, &["app"]).unwrap_or(""), arg(args, &["mode"]).unwrap_or("native")),
        "stop_app" => format!("stop {}", arg(args, &["app"]).unwrap_or("")),
        "app_logs" => format!("logs {}", arg(args, &["app"]).unwrap_or("")),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_must_match_exactly_once() {
        let t = "a\nb\na\n";
        let e = replace_unique(t, "a", "x", false).unwrap_err();
        assert!(e.contains("matches 2 places"), "{e}");
        assert_eq!(replace_unique(t, "a", "x", true).unwrap(), "x\nb\nx\n");
        assert_eq!(replace_unique(t, "b", "y", false).unwrap(), "a\ny\na\n");
        assert!(replace_unique(t, "zzz", "y", false).unwrap_err().contains("not found"));
        assert!(replace_unique(t, "", "y", false).is_err());
    }

    #[test]
    fn edit_strings_keep_edge_indentation_verbatim() {
        // Arguments reach the tools untouched (`arg` never trims) and the
        // replacement is byte for byte: edge indentation and blank edges stay.
        let args = vec![
            ("old_string".to_string(), "        let a = 1;\n".to_string()),
            ("new_string".to_string(), "        let a = 2;\n\n".to_string()),
        ];
        assert_eq!(arg(&args, &["old_string"]), Some("        let a = 1;\n"));
        let text = "fn f() {\n        let a = 1;\n}\n";
        let out = replace_unique(text, arg(&args, &["old_string"]).unwrap(), arg(&args, &["new_string"]).unwrap(), false).unwrap();
        assert_eq!(out, "fn f() {\n        let a = 2;\n\n}\n");
        // An indented-only needle must not match a differently indented line.
        assert!(replace_unique("let a = 1;\n", "        let a = 1;\n", "x", false).is_err());
    }

    #[test]
    fn loose_edit_matches_ignoring_indentation() {
        let t = "fn f() {\n    if x {\n        a();\n        b();\n    }\n}\n";
        // Wrong indentation on old_string and new_string.
        let (out, note) = edit_text(t, "a();\nb();", "a();\nc();\nb();", false).unwrap();
        assert_eq!(out, "fn f() {\n    if x {\n        a();\n        c();\n        b();\n    }\n}\n");
        assert!(note.unwrap().contains("line 3"));
        // Exact text still wins and carries no note.
        assert_eq!(edit_text(t, "a();", "z();", false).unwrap().1, None);
        // A miss points at the first line.
        let e = edit_text(t, "a();\nnope();", "x", false).unwrap_err();
        assert!(e.contains("first line appears at line 3"), "{e}");
        let e = edit_text(t, "zzz", "x", false).unwrap_err();
        assert!(e.contains("not even its first line"), "{e}");
        // Ambiguous once indentation is ignored.
        let e = edit_text("  a;\n\ta;\nb\n", "    a;\n", "x", false).unwrap_err();
        assert!(e.contains("2 places"), "{e}");
    }

    #[test]
    fn reads_cut_at_whole_lines_and_say_where_to_go_on() {
        let text: String = (1..=100).map(|i| format!("line {i} {}\n", "x".repeat(40))).collect();
        let (body, total, last) = numbered(&text, 1, 300, 1000);
        assert_eq!(total, 100);
        assert!(body.len() <= 1000 && last < 100 && last > 10);
        assert!(body.trim_end().ends_with(&"x".repeat(40)), "no half line");
        let (_, _, last) = numbered(&text, 90, 300, 100_000);
        assert_eq!(last, 100);
    }

    #[test]
    fn mentions_attach_project_files_only() {
        let base = std::env::temp_dir().join(format!("code-mention-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        let out = expand_mentions(&base, "fix @src/a.rs, not @nope.rs or @../etc/passwd or me@example.com");
        assert!(out.starts_with("fix @src/a.rs, not"));
        assert!(out.contains("[@src/a.rs: 2 lines]") && out.contains("    2\tfn b() {}"));
        assert_eq!(out.matches("[@").count(), 1);
        assert_eq!(expand_mentions(&base, "plain"), "plain");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn truncation_keeps_head_and_tail() {
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let out = truncate_middle(&text, 400);
        assert!(out.starts_with("line 0\n"));
        assert!(out.trim_end().ends_with("line 999"));
        assert!(out.contains("characters omitted"));
        assert!(out.len() < 600);
        assert_eq!(truncate_middle("short", 400), "short");
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.rs", "src/a/b.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b.rs"));
        assert!(glob_match("src/**/*.rs", "src/b.rs"));
        assert!(!glob_match("src/*.rs", "src/a/b.rs"));
        assert!(glob_match("Cargo.toml", "apps/x/Cargo.toml"));
    }

    #[test]
    fn terminal_progress_collapses() {
        let raw = "   Compiling a (1/3)\r   Compiling b (2/3)\r\x1b[32m   Compiling c (3/3)\x1b[0m\nFinished\n";
        assert_eq!(clean_terminal(raw), "   Compiling c (3/3)\nFinished\n");
        assert_eq!(clean_terminal("a\r\nb\n"), "a\nb\n");
    }

    use std::sync::mpsc::{channel, Receiver};

    fn engine() -> (Engine, Receiver<AgentEvent>) {
        let (tx, rx) = channel();
        let cfg = AgentConfig { root: std::env::temp_dir(), context_tokens: 32768, model: None, approval: ApprovalMode::Auto, session_dir: None };
        (Engine::tools_only(cfg, tx, std::sync::Arc::new(|| {})), rx)
    }

    fn call(eng: &mut Engine, name: &str, args: &[(&str, &str)]) -> (String, bool) {
        let args: Vec<(String, String)> = args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        eng.run_tool(0, name, &args)
    }

    /// The run number in a result header (`... as run 7 ...`).
    fn run_of(text: &str) -> u64 {
        let at = text.find("as run ").unwrap() + 7;
        text[at..].split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_failure_yields_early_and_the_run_goes_on() {
        let (mut eng, rx) = engine();
        let started = Instant::now();
        let (text, is_error) = call(&mut eng, "run_command", &[(
            "command",
            "echo a; echo b; sleep 0.3; echo 'error[E0308]: mismatched types'; echo '  --> src/x.rs:1:1'; sleep 2; echo late; exit 3",
        )]);
        let took = started.elapsed().as_secs_f32();
        assert!(took < 1.5, "returned after {took} s");
        assert!(is_error);
        assert!(text.starts_with("[early yield: pattern `error[E` matched at 0."), "{text}");
        assert!(text.contains("the command is still running as run "), "{text}");
        assert!(text.contains("error[E0308]: mismatched types\n  --> src/x.rs:1:1"), "{text}");
        let run = run_of(&text);
        let events: Vec<AgentEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, AgentEvent::RunBackground { run: r, pattern: Some(p), .. } if *r == run && p == "error[E")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { .. })));
        // run_output from the mark: the line printed later, then the exit.
        let (out, _) = call(&mut eng, "run_output", &[("id", &run.to_string())]);
        assert!(out.contains("late"), "{out}");
        assert!(!out.contains("mismatched"), "{out}");
        RunManager::global().wait_shell(run, Duration::from_secs(5));
        let (out, _) = call(&mut eng, "run_output", &[("id", &run.to_string())]);
        assert!(out.starts_with(&format!("run {run}: exit 3 after")), "{out}");
        assert!(out.contains("no new output"), "{out}");
        // since=0 reads it all again.
        let (out, _) = call(&mut eng, "run_output", &[("id", &run.to_string()), ("since", "0")]);
        assert!(out.contains("lines 1-5 of 5") && out.contains("a\nb\n"), "{out}");
        // The card hears the end; the model was told already (run_output), so no note.
        eng.pump_background_now();
        let events: Vec<AgentEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, AgentEvent::RunEnded { run: r, exit, ok: false, .. } if *r == run && exit == "exit 3")), "{events:?}");
        assert_eq!(eng.take_run_notes(), "");
    }

    #[cfg(unix)]
    #[test]
    fn a_quiet_run_yields_at_its_time_and_ends_with_a_note() {
        let (mut eng, rx) = engine();
        let started = Instant::now();
        let (text, is_error) = call(&mut eng, "run_command", &[("command", "echo working; sleep 1.6; echo done; exit 2"), ("yield_ms", "1000")]);
        let took = started.elapsed().as_secs_f32();
        assert!((0.9..1.5).contains(&took), "returned after {took} s");
        assert!(!is_error);
        assert!(text.starts_with("[still running after 1.0 s as run "), "{text}");
        assert!(text.contains("working"), "{text}");
        let run = run_of(&text);
        RunManager::global().wait_shell(run, Duration::from_secs(5));
        eng.pump_background_now();
        let events: Vec<AgentEvent> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolOutput { text, .. } if text.contains("done"))), "{events:?}");
        assert!(events.iter().any(|e| matches!(e, AgentEvent::RunEnded { run: r, ok: false, summary, .. } if *r == run && summary == "done")), "{events:?}");
        let notes = eng.take_run_notes();
        assert!(notes.starts_with(&format!("[background run {run} exited with code 2 after 1.")), "{notes}");
        assert!(notes.trim_end().ends_with(": done]"), "{notes}");
        // Taken once.
        assert_eq!(eng.take_run_notes(), "");
    }

    #[cfg(unix)]
    #[test]
    fn a_quick_command_returns_at_its_exit_as_before() {
        let (mut eng, _rx) = engine();
        let (text, is_error) = call(&mut eng, "run_command", &[("command", "echo one; echo two 1>&2; exit 0")]);
        assert!(!is_error);
        assert!(text.starts_with("exit code 0 after 0."), "{text}");
        assert!(text.contains("one") && text.contains("two"), "{text}");
        let (text, is_error) = call(&mut eng, "run_command", &[("command", "exit 4")]);
        assert!(is_error && text.starts_with("exit code 4"), "{text}");
        // A custom pattern, as a regex.
        let (text, _) = call(&mut eng, "run_command", &[("command", "echo 'worker 3 crashed'; sleep 5"), ("early_yield_patterns", "[\"/worker \\\\d+ crashed/\"]")]);
        assert!(text.starts_with("[early yield: pattern `/worker \\d+ crashed/` matched at"), "{text}");
        assert_eq!(list(&[("p".into(), "[\"/a \\d/\", \"b\"]".into())], "p"), ["/a \\d/", "b"]);
        assert_eq!(list(&[("p".into(), "x\ny".into())], "p"), ["x", "y"]);
        let run = run_of(&text);
        let (text, is_error) = call(&mut eng, "run_stop", &[("id", &run.to_string())]);
        assert!(!is_error && text.contains("stopped (run_stop)"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn run_stop_ends_the_whole_process_group() {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let (mut eng, _rx) = engine();
        let file = std::env::temp_dir().join(format!("code-yield-grandchild-{}", std::process::id()));
        let command = format!("sleep 30 & echo $! > {}; echo started; wait", file.display());
        let (text, _) = call(&mut eng, "run_command", &[("command", &command), ("yield_ms", "1000")]);
        let run = run_of(&text);
        let grandchild: i32 = std::fs::read_to_string(&file).unwrap().trim().parse().unwrap();
        assert_eq!(unsafe { kill(grandchild, 0) }, 0, "the grandchild runs");
        let (text, is_error) = call(&mut eng, "run_stop", &[("id", &run.to_string())]);
        assert!(!is_error, "{text}");
        let end = Instant::now() + Duration::from_secs(5);
        while unsafe { kill(grandchild, 0) } == 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(unsafe { kill(grandchild, 0) }, 0, "the grandchild was stopped with its group");
        let _ = std::fs::remove_file(&file);
        // A stranger's run asks first; with nobody to answer, nothing stops.
        let (mut other, _rx2) = engine();
        other.mode = ApprovalMode::Ask;
        other.interrupted = true; // no front end to answer the approval
        let (mut eng2, _rx3) = engine();
        let (text, _) = call(&mut eng2, "run_command", &[("command", "sleep 20"), ("yield_ms", "1000")]);
        let run = run_of(&text);
        let (text, is_error) = call(&mut other, "run_stop", &[("id", &run.to_string())]);
        assert!(is_error && text.contains("denied"), "{text}");
        assert!(RunManager::global().shell(run).unwrap().exit.is_none());
        // The owner's session end stops it.
        eng2.end_runs("the session ended");
        let view = RunManager::global().wait_shell(run, Duration::from_secs(5)).unwrap();
        assert_eq!(view.exit, Some(ShellExit::Stopped("the session ended".into())));
    }

    #[test]
    fn todos_parse() {
        let t = parse_todos("- [x] a\n[>] b\n[ ] c\nplain");
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].state, TodoState::Done);
        assert_eq!(t[1].state, TodoState::Active);
        assert_eq!(t[3].text, "plain");
    }
}
