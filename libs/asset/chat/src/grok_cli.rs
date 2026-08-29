//! The `grok` CLI as a chat provider — the same headless contract as
//! Claude Code (single-turn prompt via `--prompt-file`,
//! Anthropic-Messages-format NDJSON with partial
//! deltas, `--resume <session>`), so it shares that parser
//! ([`crate::claude::parse_stream_line`]). Chat-only: every built-in tool
//! the CLI advertises is disallowed, permission mode `dontAsk` denies
//! anything that slips through, and `--max-turns 1` bounds the process to
//! the one reply. No key passes through us: the CLI is logged in on the
//! broker host or the provider is `Unavailable`.

use crate::claude::{build_prompt_only, poll_messages_turn, ParseState};
use crate::cli::{cli_command, find_cli, turn_dir, CliTurn};
use crate::provider::{ChatProvider, ProviderEvent, TurnInput};
use crate::wire::{ProviderAvailability, ProviderKind};
use std::path::PathBuf;

/// Every built-in tool grok 1.0.5 listed in its `init` line. `--tools ""`
/// and `--tools none` do NOT strip them (observed live); `--disallowed-tools`
/// does, bar four process-management tools it keeps regardless — which
/// `--max-turns 1` + `dontAsk` render inert.
pub const GROK_BUILTIN_TOOLS: &str = "run_terminal_command,read_file,search_replace,list_dir,grep,\
todo_write,scheduler_create,scheduler_delete,scheduler_list,monitor,search_tool,use_tool,workflow,\
enter_plan_mode,exit_plan_mode,ask_user_question,web_search,web_fetch,image_gen,image_edit,\
image_to_video,reference_to_video,write,kill_command_or_subagent,get_command_or_subagent_output,\
spawn_subagent";

pub struct GrokCliChatProvider {
    cli: Option<PathBuf>,
    model: Option<String>,
    resume: Option<String>,
    turn: Option<(CliTurn, ParseState)>,
}

impl GrokCliChatProvider {
    pub fn new(model: Option<String>) -> GrokCliChatProvider {
        GrokCliChatProvider { cli: find_grok(), model, resume: None, turn: None }
    }
}

/// `GROK_CLI_PATH`, else `grok` on `$PATH` or in its installer's dir.
pub fn find_grok() -> Option<PathBuf> {
    find_cli("GROK_CLI_PATH", "grok", &["~/.grok/bin/grok"])
}

/// argv after the executable, pure for testing. The prompt is NOT here —
/// grok has no stdin prompt mode, but `--prompt-file` (single-turn, like
/// `-p`; verified live, grok 1.0.5) reads it from a file inside the 0700
/// per-turn dir, so chat content never shows in a process listing.
pub fn build_args(
    model: &Option<String>,
    resume: &Option<String>,
    system: &str,
    prompt_file: &str,
    cwd: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--prompt-file".into(),
        prompt_file.to_string(),
        "--output-format".into(),
        "streaming-messages-json".into(),
        "--include-partial-messages".into(),
        "--permission-mode".into(),
        "dontAsk".into(),
        "--max-turns".into(),
        "1".into(),
        "--disallowed-tools".into(),
        GROK_BUILTIN_TOOLS.into(),
        "--cwd".into(),
        cwd.to_string(),
    ];
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.clone());
    }
    if let Some(r) = resume {
        args.push("--resume".into());
        args.push(r.clone());
    }
    if !system.is_empty() {
        args.push("--system-prompt-override".into());
        args.push(system.to_string());
    }
    args
}

impl ChatProvider for GrokCliChatProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GrokCli
    }

    fn availability(&mut self) -> ProviderAvailability {
        match &self.cli {
            Some(p) => ProviderAvailability::Available {
                model: self.model.clone().unwrap_or_else(|| "grok-cli".to_string()),
                detail: p.display().to_string(),
            },
            None => ProviderAvailability::Unavailable {
                reason: "grok CLI not found on this host (set GROK_CLI_PATH or install grok)".to_string(),
            },
        }
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        if self.turn.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        let Some(cli) = self.cli.clone() else {
            return Err("grok CLI not found".to_string());
        };
        let prompt = build_prompt_only(input, self.resume.is_some());
        let cwd = turn_dir("grok");
        let prompt_file = cwd.join("prompt.txt");
        if let Err(e) = std::fs::write(&prompt_file, prompt.as_bytes()) {
            let _ = std::fs::remove_dir_all(&cwd);
            return Err(format!("failed to stage grok prompt: {e}"));
        }
        let args = build_args(
            &self.model,
            &self.resume,
            &input.system,
            &prompt_file.to_string_lossy(),
            &cwd.to_string_lossy(),
        );
        let mut command = cli_command(&cli, &cwd);
        command.args(&args);
        let turn = CliTurn::spawn(command, None, "grok", Some(cwd))?;
        self.turn = Some((turn, ParseState::default()));
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        poll_messages_turn(&mut self.turn, &mut self.resume, "grok")
    }

    fn cancel(&mut self) {
        if let Some((cli, _)) = self.turn.take() {
            cli.kill_group();
        }
    }
}

impl Drop for GrokCliChatProvider {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::parse_stream_line;

    #[test]
    fn argv_is_headless_single_turn_and_toolless() {
        let args = build_args(&None, &Some("s9".into()), "sys", "/tmp/x/prompt.txt", "/tmp/x");
        // The prompt itself is never in argv — only the path of its file
        // inside the private per-turn dir.
        assert_eq!(&args[..2], &["--prompt-file".to_string(), "/tmp/x/prompt.txt".to_string()]);
        assert!(!args.iter().any(|a| a == "-p"));
        assert!(args.windows(2).any(|w| w[0] == "--max-turns" && w[1] == "1"));
        assert!(args.windows(2).any(|w| w[0] == "--permission-mode" && w[1] == "dontAsk"));
        assert!(args.windows(2).any(|w| w[0] == "--disallowed-tools" && w[1].contains("run_terminal_command")));
        assert!(args.windows(2).any(|w| w[0] == "--resume" && w[1] == "s9"));
        assert!(args.windows(2).any(|w| w[0] == "--system-prompt-override" && w[1] == "sys"));
        assert!(args.windows(2).any(|w| w[0] == "--cwd" && w[1] == "/tmp/x"));
    }

    #[test]
    fn grok_speaks_the_same_stream_as_claude() {
        // The live grok 1.0.5 result line, trimmed.
        let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"hello from grok","session_id":"01a0"}"#;
        let mut state = ParseState::default();
        let (events, done) =
            parse_stream_line(&makepad_asset_client::json::parse(line.as_bytes()).unwrap(), &mut state);
        assert!(done);
        assert!(matches!(&events[..], [ProviderEvent::Done { text }] if text == "hello from grok"));
        assert_eq!(state.session_id.as_deref(), Some("01a0"));
    }
}
