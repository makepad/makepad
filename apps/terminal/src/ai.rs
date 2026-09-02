//! The live terminal on the desktop's AI bus.
//!
//! Reads come straight from the emulator at call time. `run` takes the same
//! PTY input path as a paste into the terminal: it is deliberately not a
//! sandbox or a second shell.

use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_strict_json::{self as json, Value};

const DEFAULT_SCROLLBACK_LINES: usize = 200;
const MAX_SCROLLBACK_LINES: usize = 2_000;
const MAX_COMMAND_BYTES: usize = 4 * 1024;

pub struct ScreenState {
    pub rows: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cwd: Option<String>,
}

/// The small seam between the bus and the terminal widget. Implemented by
/// `MpTerm`, so commands and reads use the already-running session.
pub trait TerminalTarget {
    fn visible_screen(&self) -> Option<ScreenState>;
    fn recent_screen(&self, lines: usize) -> Option<ScreenState>;
    fn type_bytes(&mut self, bytes: &[u8]) -> bool;
}

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "terminal",
        "Terminal",
        "The live terminal. Its screen tools read what is visible now and its recent scrollback. run types a line into the live shell exactly as the person would: it runs for real and is not sandboxed; read_screen on the next turn reads the result.",
    )
    .with_tool(ToolDef::new(
        "read_screen",
        "Read the terminal grid that is visible now, with trailing spaces removed from each row, plus cursor and working-directory context.",
        r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "read_scrollback",
        "Read the last requested number of lines from the terminal's accessible scrollback plus screen (default 200, maximum 2000).",
        r#"{"type":"object","properties":{"lines":{"type":"integer","minimum":1,"maximum":2000}},"additionalProperties":false}"#,
        Risk::Read,
    ))
    .with_tool(ToolDef::new(
        "run",
        "Type a command followed by Enter into the live shell. This runs for real, is not sandboxed, and returns immediately; use read_screen on the next turn to see output.",
        r#"{"type":"object","properties":{"command":{"type":"string","maxLength":4096}},"required":["command"],"additionalProperties":false}"#,
        Risk::Act,
    ))
}

/// Answer one terminal call. The match is intentionally closed: no caller can
/// name an operation outside these three tools.
pub fn answer(call: &ServiceCall, target: &mut impl TerminalTarget) -> ToolResult {
    match call.tool.as_str() {
        "read_screen" => {
            if let Err(error) = empty_args(&call.args) {
                return ToolResult::refused(&call.call_id, error);
            }
            match target.visible_screen() {
                Some(state) => screen_result(&call.call_id, state),
                None => ToolResult::unavailable(&call.call_id, "the terminal session is not ready"),
            }
        }
        "read_scrollback" => {
            let fields = match object_args(&call.args, &["lines"]) {
                Ok(fields) => fields,
                Err(error) => return ToolResult::refused(&call.call_id, error),
            };
            let lines = match fields.iter().find(|(key, _)| key == "lines") {
                None => DEFAULT_SCROLLBACK_LINES,
                Some((_, Value::Int(lines))) if *lines >= 1 => {
                    (*lines as usize).min(MAX_SCROLLBACK_LINES)
                }
                Some(_) => {
                    return ToolResult::refused(
                        &call.call_id,
                        "read_scrollback.lines must be an integer from 1 to 2000",
                    )
                }
            };
            match target.recent_screen(lines) {
                Some(state) => screen_result(&call.call_id, state),
                None => ToolResult::unavailable(&call.call_id, "the terminal session is not ready"),
            }
        }
        "run" => {
            let fields = match object_args(&call.args, &["command"]) {
                Ok(fields) => fields,
                Err(error) => return ToolResult::refused(&call.call_id, error),
            };
            let Some(command) = fields
                .iter()
                .find(|(key, _)| key == "command")
                .and_then(|(_, value)| value.as_str())
            else {
                return ToolResult::refused(&call.call_id, "run.command must be a string");
            };
            if let Err(error) = validate_command(command) {
                return ToolResult::refused(&call.call_id, error);
            }
            let mut bytes = command.as_bytes().to_vec();
            bytes.push(b'\n');
            if target.type_bytes(&bytes) {
                ToolResult::ok(&call.call_id, "command typed into the live shell", "typed")
            } else {
                ToolResult::unavailable(&call.call_id, "the terminal session is not ready")
            }
        }
        other => ToolResult::refused(
            &call.call_id,
            format!(
                "terminal has no tool `{other}`; it has read_screen, read_scrollback, run"
            ),
        ),
    }
}

fn screen_result(call_id: &str, state: ScreenState) -> ToolResult {
    let note = match state.cwd {
        Some(cwd) => format!(
            "cursor row {}, column {}; cwd {}",
            state.cursor_row + 1,
            state.cursor_col + 1,
            cwd
        ),
        None => format!(
            "cursor row {}, column {}; cwd unknown",
            state.cursor_row + 1,
            state.cursor_col + 1
        ),
    };
    ToolResult::ok(call_id, render_rows(&state.rows), note)
}

fn render_rows(rows: &[String]) -> String {
    rows.iter()
        .map(|row| row.trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n")
}

fn empty_args(args: &str) -> Result<(), String> {
    object_args(args, &[]).map(|_| ())
}

fn object_args(args: &str, allowed: &[&str]) -> Result<Vec<(String, Value)>, String> {
    let fields = match json::parse(args.as_bytes()) {
        Ok(Value::Obj(fields)) => fields,
        Ok(_) => return Err("tool arguments must be a JSON object".to_string()),
        Err(error) => return Err(format!("invalid tool arguments: {error}")),
    };
    if let Some((key, _)) = fields.iter().find(|(key, _)| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown argument `{key}`"));
    }
    Ok(fields)
}

fn validate_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("run.command must not be empty".to_string());
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(format!(
            "run.command is {} bytes; the maximum is {MAX_COMMAND_BYTES}",
            command.len()
        ));
    }
    if command
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err(
            "run.command contains a control character other than newline or tab".to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::ToolOutcome;

    struct FakeTarget;

    impl TerminalTarget for FakeTarget {
        fn visible_screen(&self) -> Option<ScreenState> {
            None
        }

        fn recent_screen(&self, _lines: usize) -> Option<ScreenState> {
            None
        }

        fn type_bytes(&mut self, _bytes: &[u8]) -> bool {
            true
        }
    }

    fn call(command: &str) -> ServiceCall {
        ServiceCall {
            call_id: "c1".into(),
            tool: "run".into(),
            args: format!(r#"{{"command":"{command}"}}"#),
        }
    }

    #[test]
    fn terminal_manifest_validates() {
        manifest().validate().expect("a valid terminal manifest");
    }

    #[test]
    fn rows_are_trimmed_and_joined() {
        assert_eq!(
            render_rows(&["one   ".into(), "two ".into(), "".into()]),
            "one\ntwo\n"
        );
    }

    #[test]
    fn run_refuses_control_characters() {
        let mut target = FakeTarget;
        let result = answer(&call(r"echo\u001bboom"), &mut target);
        assert_eq!(result.outcome, ToolOutcome::Refused);
        assert!(result.text.contains("control character"));

        assert!(validate_command("printf 'a\\nb'\n\t").is_ok());
    }
}
