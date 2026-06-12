#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiTerminalAnalysis {
    pub mode: &'static str,
    pub is_codex: bool,
    pub summary: String,
    pub codex_status: Option<String>,
}

pub fn terminal_mode_and_summary(title: &str, visible_text: &str) -> AiTerminalAnalysis {
    let lines: Vec<String> = visible_text.lines().map(|line| line.to_string()).collect();
    let lowered = format!("{}\n{}", title, visible_text).to_lowercase();
    let codex_status = detect_codex_status_line(&lines);
    let codex_prompt_visible = lines
        .iter()
        .rev()
        .take(6)
        .any(|line| is_codex_prompt_line(line));
    let strong_codex_prompt_visible = lines
        .iter()
        .rev()
        .take(6)
        .any(|line| is_strong_codex_prompt_line(line));
    let codex_prompt_has_draft = lines
        .iter()
        .rev()
        .take(6)
        .any(|line| is_codex_prompt_line(line) && codex_prompt_has_draft(line));
    let is_codex = lowered.contains("codex")
        || lowered.contains("apply_patch")
        || lowered.contains("exec_command")
        || lowered.contains("functions.exec_command")
        || lowered.contains("esc to interrupt")
        || lowered.contains("left \u{00b7}")
        || lowered.contains("gpt-5")
        || codex_status.is_some()
        || strong_codex_prompt_visible;
    let codex_status = if is_codex { codex_status } else { None };
    let needs_attention = lowered.contains("permission denied")
        || lowered.contains("sandbox")
        || lowered.contains("panic")
        || lowered.contains("error:")
        || lowered.contains("failed")
        || lowered.contains("blocked")
        || lowered.contains("approve")
        || lowered.contains("how would you like to proceed");
    let awaiting_input = lowered.contains("waiting for user")
        || lowered.contains("request user input")
        || lowered.contains("press enter")
        || lowered.contains("press return")
        || lowered.contains("continue?")
        || lowered.contains("type 'continue'")
        || lowered.contains("type \"continue\"");
    let working = lowered.contains("apply_patch")
        || lowered.contains("exec_command")
        || lowered.contains("searching")
        || lowered.contains("reading")
        || lowered.contains("building")
        || lowered.contains("testing")
        || lowered.contains("running")
        || lowered.contains("patching")
        || codex_status.is_some();

    let mode = if needs_attention {
        "needs-attention"
    } else if working {
        "working"
    } else if is_codex && codex_prompt_has_draft {
        "awaiting-input"
    } else if awaiting_input {
        "awaiting-input"
    } else if is_codex && codex_prompt_visible && codex_status.is_none() {
        "done"
    } else if visible_text.trim().is_empty() {
        "starting"
    } else {
        "idle"
    };

    AiTerminalAnalysis {
        mode,
        is_codex,
        summary: terminal_summary_line(&lines, is_codex, codex_status.as_deref()),
        codex_status,
    }
}

pub fn terminal_summary_line(
    lines: &[String],
    is_codex: bool,
    codex_status: Option<&str>,
) -> String {
    lines
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| {
            !line.is_empty()
                && Some(*line) != codex_status
                && !(is_codex
                    && (is_codex_prompt_line(line)
                        || line.contains("esc to interrupt")
                        || line.contains("100% left")
                        || line.contains("left \u{00b7}")))
        })
        .map(|line| truncate_inline(line, 140))
        .unwrap_or_else(|| "No visible output yet".to_string())
}

pub fn truncate_terminal_excerpt(text: &str, max_chars: usize, max_lines: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let start = lines.len().saturating_sub(max_lines);
    let excerpt = lines[start..].join("\n");
    if excerpt.chars().count() <= max_chars {
        return excerpt;
    }
    let tail: String = excerpt
        .chars()
        .rev()
        .take(max_chars.saturating_sub(3))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{}", tail)
}

pub fn terminal_blocked_reason(mode: &str) -> Option<&'static str> {
    match mode {
        "awaiting-input" => Some("Tracked terminal is awaiting input"),
        "needs-attention" => Some("Tracked terminal needs attention"),
        _ => None,
    }
}

pub fn is_codex_prompt_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('\u{203a}')
        || trimmed.starts_with('>')
        || trimmed.contains("Enter a prompt...")
}

pub fn is_strong_codex_prompt_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('\u{203a}') || trimmed.contains("Enter a prompt...")
}

pub fn codex_prompt_has_draft(line: &str) -> bool {
    let trimmed = line.trim_start();
    let rest = if let Some(rest) = trimmed.strip_prefix('\u{203a}') {
        rest
    } else if let Some(rest) = trimmed.strip_prefix('>') {
        rest
    } else {
        return false;
    };
    let rest = rest.trim();
    !rest.is_empty() && !rest.contains("Enter a prompt...")
}

pub fn detect_codex_status_line(lines: &[String]) -> Option<String> {
    lines.iter().rev().take(8).find_map(|line| {
        let trimmed = line.trim();
        let lowered = trimmed.to_lowercase();
        if (trimmed.contains("Working (") && trimmed.contains("esc to interrupt"))
            || (lowered.contains("working")
                && (lowered.contains("esc to interrupt")
                    || lowered.contains("gpt-")
                    || lowered.contains("codex")))
        {
            Some(trimmed.to_string())
        } else {
            None
        }
    })
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_mode_detects_codex_working_status() {
        let text = "\n\nWorking (12s) esc to interrupt\n";
        let analysis = terminal_mode_and_summary("codex", text);
        assert_eq!(analysis.mode, "working");
        assert!(analysis.is_codex);
        assert_eq!(
            analysis.codex_status.as_deref(),
            Some("Working (12s) esc to interrupt")
        );
    }

    #[test]
    fn terminal_mode_detects_codex_prompt_draft() {
        let analysis = terminal_mode_and_summary("", "\n\u{203a} make a hello world example\n");
        assert_eq!(analysis.mode, "awaiting-input");
        assert!(analysis.is_codex);
        assert_eq!(analysis.codex_status, None);
    }

    #[test]
    fn terminal_mode_detects_compact_codex_working_status() {
        let text = "\n[working] gpt-5.5 xhigh fast \u{00b7} ~/makepad/makepad\n";
        let analysis = terminal_mode_and_summary("", text);
        assert_eq!(analysis.mode, "working");
        assert!(analysis.is_codex);
        assert_eq!(
            analysis.codex_status.as_deref(),
            Some("[working] gpt-5.5 xhigh fast \u{00b7} ~/makepad/makepad")
        );
    }

    #[test]
    fn terminal_mode_prefers_codex_working_status_over_prompt_line() {
        let text = "• Working (20s • esc to interrupt)\n\n› Improve documentation in @filename\n\n  gpt-5.5 xhigh fast \u{00b7} ~/makepad/makepad";
        let analysis = terminal_mode_and_summary("", text);
        assert_eq!(analysis.mode, "working");
        assert!(analysis.is_codex);
        assert_eq!(
            analysis.codex_status.as_deref(),
            Some("• Working (20s • esc to interrupt)")
        );
    }

    #[test]
    fn terminal_summary_skips_codex_prompt_chrome() {
        let lines = vec![
            "finished editing terminal_state.rs".to_string(),
            "Working (2s) esc to interrupt".to_string(),
            "\u{203a} Enter a prompt...".to_string(),
        ];
        assert_eq!(
            terminal_summary_line(&lines, true, Some("Working (2s) esc to interrupt")),
            "finished editing terminal_state.rs"
        );
    }

    #[test]
    fn truncate_terminal_excerpt_keeps_recent_nonempty_lines() {
        let excerpt = truncate_terminal_excerpt("one\n\n two  \nthree\nfour\n", 80, 2);
        assert_eq!(excerpt, "three\nfour");
    }

    #[test]
    fn truncate_terminal_excerpt_keeps_tail_when_too_long() {
        let excerpt = truncate_terminal_excerpt("abcdef\n1234567890", 8, 5);
        assert_eq!(excerpt, "...67890");
    }

    #[test]
    fn terminal_blocked_reason_matches_blocked_modes() {
        assert_eq!(
            terminal_blocked_reason("awaiting-input"),
            Some("Tracked terminal is awaiting input")
        );
        assert_eq!(
            terminal_blocked_reason("needs-attention"),
            Some("Tracked terminal needs attention")
        );
        assert_eq!(terminal_blocked_reason("working"), None);
    }
}
