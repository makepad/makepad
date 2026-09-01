//! The `codex` CLI as a chat provider: `codex exec --json`, one process per
//! turn, `codex exec … resume <thread>` across turns. Codex has no "no
//! tools" switch, so it runs `--sandbox read-only` in an empty scratch
//! directory with user config and rules ignored: nothing to read, nothing
//! to write, no shell environment inherited by anything it runs. The
//! system prompt rides as `developer_instructions` config, which stays
//! authoritative on resumed turns (a prefix in the first user message would
//! not). No streaming deltas exist in this protocol: the reply lands as one
//! `agent_message` item; reasoning items are forwarded as a think block.

use crate::chat_wire::{ProviderAvailability, ProviderKind};
use crate::providers::claude::build_prompt_only;
use crate::providers::cli::{
    categorize_cli_error, cli_command, find_cli, toml_basic_string, turn_dir, CliTurn,
};
use crate::providers::provider::{ChatProvider, ProviderEvent, TurnInput};
use makepad_strict_json::{self as json, Value};
use std::path::PathBuf;

#[derive(Default)]
pub struct ParseState {
    pub collected: String,
    /// The CLI's thread id, for `resume`.
    pub session_id: Option<String>,
}

pub struct CodexCliChatProvider {
    cli: Option<PathBuf>,
    model: Option<String>,
    resume: Option<String>,
    turn: Option<(CliTurn, ParseState)>,
}

impl CodexCliChatProvider {
    pub fn new(model: Option<String>) -> CodexCliChatProvider {
        CodexCliChatProvider { cli: find_codex(), model, resume: None, turn: None }
    }
}

/// `CODEX_CLI_PATH`, else `codex` on `$PATH` or in the usual dirs.
pub fn find_codex() -> Option<PathBuf> {
    find_cli("CODEX_CLI_PATH", "codex", &["~/.codex/bin/codex"])
}

/// argv after the executable, pure for testing. Exec-level policy flags
/// must precede `resume`: the CLI rejects `--sandbox` after it and, worse,
/// omitting it lets a resumed thread inherit a broader policy. The prompt
/// is NOT here — the final `-` positional makes `codex exec` (and `codex
/// exec resume`) read it from stdin (verified live, codex 0.149), so chat
/// content never shows in a process listing.
pub fn build_args(
    model: &Option<String>,
    resume: &Option<String>,
    system: &str,
    cwd: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "exec".into(),
        "--json".into(),
        "--color".into(),
        "never".into(),
        "--sandbox".into(),
        "read-only".into(),
        "--skip-git-repo-check".into(),
        "--ignore-user-config".into(),
        "--ignore-rules".into(),
        "--config".into(),
        "shell_environment_policy.inherit=none".into(),
        "-C".into(),
        cwd.to_string(),
    ];
    if !system.is_empty() {
        args.push("--config".into());
        args.push(format!("developer_instructions={}", toml_basic_string(system)));
    }
    if let Some(m) = model {
        args.push("-m".into());
        args.push(m.clone());
    }
    if let Some(r) = resume {
        args.push("resume".into());
        args.push(r.clone());
    }
    args.push("-".into());
    args
}

/// One `codex exec --json` line → events; `done` when the turn ended.
pub fn parse_line(v: &Value, state: &mut ParseState) -> (Vec<ProviderEvent>, bool) {
    let mut events = Vec::new();
    match v.get("type").and_then(Value::as_str) {
        Some("thread.started") => {
            if let Some(id) = v.get("thread_id").and_then(Value::as_str) {
                state.session_id = Some(id.to_string());
            }
        }
        Some("item.completed") => {
            let item = v.get("item");
            let text = item.and_then(|i| i.get("text")).and_then(Value::as_str).unwrap_or("");
            match item.and_then(|i| i.get("type")).and_then(Value::as_str) {
                Some("agent_message") if !text.is_empty() => {
                    if !state.collected.is_empty() {
                        state.collected.push('\n');
                        events.push(ProviderEvent::Delta("\n".to_string()));
                    }
                    state.collected.push_str(text);
                    events.push(ProviderEvent::Delta(text.to_string()));
                }
                Some("reasoning") if !text.is_empty() => {
                    events.push(ProviderEvent::Delta(format!("<think>{text}</think>\n")));
                }
                Some("error") => {
                    let message = item
                        .and_then(|i| i.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("Codex reported an error");
                    events.push(ProviderEvent::Error(message.to_string()));
                }
                _ => {}
            }
        }
        Some("turn.completed") => {
            events.push(ProviderEvent::Done { text: state.collected.clone() });
            return (events, true);
        }
        Some("turn.failed") | Some("error") => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .or_else(|| v.get("message").and_then(Value::as_str))
                .unwrap_or("Codex turn failed");
            events.push(ProviderEvent::Error(message.to_string()));
            return (events, true);
        }
        _ => {}
    }
    (events, false)
}

impl ChatProvider for CodexCliChatProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::CodexCli
    }

    fn availability(&mut self) -> ProviderAvailability {
        match &self.cli {
            Some(p) => ProviderAvailability::Available {
                model: self.model.clone().unwrap_or_else(|| "codex".to_string()),
                detail: p.display().to_string(),
            },
            None => ProviderAvailability::Unavailable {
                reason: "codex CLI not found on this host (set CODEX_CLI_PATH or install codex)".to_string(),
            },
        }
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        if self.turn.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        let Some(cli) = self.cli.clone() else {
            return Err("codex CLI not found".to_string());
        };
        let prompt = build_prompt_only(input, self.resume.is_some());
        let cwd = turn_dir("codex");
        let args = build_args(&self.model, &self.resume, &input.system_with_dynamic(), &cwd.to_string_lossy());
        let mut command = cli_command(&cli, &cwd);
        command.args(&args);
        let turn = CliTurn::spawn(command, Some(prompt), "codex", Some(cwd))?;
        self.turn = Some((turn, ParseState::default()));
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        let Some((cli, parse)) = self.turn.as_mut() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let drained = cli.drain();
        for line in drained.lines {
            if cli.finished {
                break;
            }
            if let Ok(v) = json::parse(line.as_bytes()) {
                let (mut evs, done) = parse_line(&v, parse);
                // Vendor error text → fixed public category; the raw
                // message goes to the server log only.
                for ev in &mut evs {
                    if let ProviderEvent::Error(raw) = ev {
                        let public = categorize_cli_error("codex", raw, false);
                        *ev = ProviderEvent::Error(public);
                    }
                }
                events.append(&mut evs);
                if done {
                    cli.finished = true;
                }
            }
        }
        if drained.exited && !cli.finished {
            events.push(ProviderEvent::Error(cli.exit_error("codex")));
            cli.finished = true;
        }
        if cli.finished {
            if let Some((cli, mut parse)) = self.turn.take() {
                if let Some(sid) = parse.session_id.take() {
                    self.resume = Some(sid);
                }
                cli.wait();
            }
        }
        events
    }

    fn cancel(&mut self) {
        if let Some((cli, _)) = self.turn.take() {
            cli.kill_group();
        }
    }
}

impl Drop for CodexCliChatProvider {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_flags_precede_resume_and_the_prompt_is_never_in_argv() {
        let args = build_args(&Some("o4".into()), &Some("t1".into()), "be terse", "/tmp/c");
        let resume_at = args.iter().position(|a| a == "resume").unwrap();
        let sandbox_at = args.iter().position(|a| a == "--sandbox").unwrap();
        assert!(sandbox_at < resume_at);
        assert_eq!(args[resume_at + 1], "t1");
        // The `-` positional makes codex read the prompt from STDIN — chat
        // content must never be visible in a process listing.
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(args.iter().any(|a| a == "developer_instructions=\"be terse\""));
        assert!(args.windows(2).any(|w| w[0] == "-C" && w[1] == "/tmp/c"));
        assert!(args.windows(2).any(|w| w[0] == "-m" && w[1] == "o4"));
        let bare = build_args(&None, &None, "", "/tmp/c");
        assert_eq!(bare.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn the_live_event_shapes_parse() {
        let mut state = ParseState::default();
        let mut all = Vec::new();
        let mut ended = false;
        for line in [
            r#"{"type":"thread.started","thread_id":"01a0-thread"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"i0","type":"reasoning","text":"plan"}}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"hello from codex"}}"#,
            r#"{"type":"turn.completed","usage":{"output_tokens":8}}"#,
        ] {
            let (events, done) = parse_line(&json::parse(line.as_bytes()).unwrap(), &mut state);
            all.extend(events);
            ended |= done;
        }
        assert!(ended);
        assert_eq!(state.session_id.as_deref(), Some("01a0-thread"));
        let text: String = all
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::Delta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "<think>plan</think>\nhello from codex");
        assert!(matches!(all.last(), Some(ProviderEvent::Done { text }) if text == "hello from codex"));
        let (events, done) = parse_line(
            &json::parse(br#"{"type":"turn.failed","error":{"message":"quota"}}"#).unwrap(),
            &mut ParseState::default(),
        );
        assert!(done);
        assert!(matches!(&events[..], [ProviderEvent::Error(m)] if m == "quota"));
    }
}
