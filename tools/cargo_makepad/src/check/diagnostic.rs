//! Rich diagnostic output formatting for script check.
//!
//! Provides rustc-style diagnostic output with source context,
//! colored severity labels, and fix suggestions.

use super::error::{ScriptIssue, Severity};

/// ANSI escape code to reset formatting.
const RESET: &str = "\x1b[0m";
/// ANSI escape code for bold text.
const BOLD: &str = "\x1b[1m";
/// ANSI escape code for blue (used for line numbers and arrows).
const BLUE: &str = "\x1b[1;34m";

/// A rendered diagnostic with source context.
pub struct Diagnostic<'a> {
    issue: &'a ScriptIssue,
    source: Option<&'a str>,
    use_color: bool,
}

impl<'a> Diagnostic<'a> {
    /// Create a new diagnostic from an issue.
    pub fn new(issue: &'a ScriptIssue) -> Self {
        Self {
            issue,
            source: None,
            use_color: false,
        }
    }

    /// Add source code for context display.
    pub fn with_source(mut self, source: &'a str) -> Self {
        self.source = Some(source);
        self
    }

    /// Enable or disable colored output.
    pub fn with_color(mut self, use_color: bool) -> Self {
        self.use_color = use_color;
        self
    }

    /// Render the diagnostic to a string.
    pub fn render(&self) -> String {
        let mut out = String::new();

        // Header line: error[E0001]: message
        self.render_header(&mut out);

        // Location line: --> file:line:column
        self.render_location(&mut out);

        // Source context with caret
        if self.source.is_some() {
            self.render_source_context(&mut out);
        }

        // Suggestion/help
        self.render_suggestion(&mut out);

        out.push('\n');
        out
    }

    fn render_header(&self, out: &mut String) {
        let severity = self.issue.severity();

        if self.use_color {
            out.push_str(severity.color_code());
        }
        out.push_str(severity.label());

        // Add error code if present
        if let Some(code) = self.issue.kind.code() {
            out.push('[');
            out.push_str(code);
            out.push(']');
        }

        if self.use_color {
            out.push_str(RESET);
        }

        out.push_str(": ");

        if self.use_color {
            out.push_str(BOLD);
        }
        out.push_str(&self.issue.message);
        if self.use_color {
            out.push_str(RESET);
        }
        out.push('\n');
    }

    fn render_location(&self, out: &mut String) {
        if self.use_color {
            out.push_str(BLUE);
        }
        out.push_str("  --> ");
        if self.use_color {
            out.push_str(RESET);
        }
        out.push_str(&self.issue.file);
        out.push(':');
        out.push_str(&self.issue.line.to_string());
        out.push(':');
        out.push_str(&self.issue.column.to_string());
        out.push('\n');
    }

    fn render_source_context(&self, out: &mut String) {
        let source = match self.source {
            Some(s) => s,
            None => return,
        };

        let line_num = self.issue.line as usize;
        let source_line = source.lines().nth(line_num.saturating_sub(1));

        let line_str = self.issue.line.to_string();
        let padding = " ".repeat(line_str.len());

        // Empty line with pipe
        if self.use_color {
            out.push_str(BLUE);
        }
        out.push_str(&padding);
        out.push_str(" |\n");
        if self.use_color {
            out.push_str(RESET);
        }

        // Source line
        if let Some(line) = source_line {
            if self.use_color {
                out.push_str(BLUE);
            }
            out.push_str(&line_str);
            out.push_str(" | ");
            if self.use_color {
                out.push_str(RESET);
            }
            out.push_str(line);
            out.push('\n');

            // Caret line
            if self.use_color {
                out.push_str(BLUE);
            }
            out.push_str(&padding);
            out.push_str(" | ");
            if self.use_color {
                out.push_str(RESET);
            }

            let caret_pos = (self.issue.column as usize).saturating_sub(1);
            out.push_str(&" ".repeat(caret_pos));

            if self.use_color {
                out.push_str(self.issue.severity().color_code());
            }
            out.push('^');

            // Add secondary label if present
            if let Some(ref label) = self.issue.secondary_label {
                out.push(' ');
                out.push_str(label);
            }

            if self.use_color {
                out.push_str(RESET);
            }
            out.push('\n');
        }
    }

    fn render_suggestion(&self, out: &mut String) {
        if let Some(ref suggestion) = self.issue.suggestion {
            let line_str = self.issue.line.to_string();
            let padding = " ".repeat(line_str.len());

            if self.use_color {
                out.push_str(BLUE);
            }
            out.push_str(&padding);
            out.push_str(" |\n");
            if self.use_color {
                out.push_str(RESET);
            }

            out.push_str(&padding);
            out.push_str(" = ");
            if self.use_color {
                out.push_str(Severity::Help.color_code());
            }
            out.push_str("help");
            if self.use_color {
                out.push_str(RESET);
            }
            out.push_str(": ");
            out.push_str(suggestion);
            out.push('\n');
        }
    }
}

/// Print a collection of issues with rich formatting.
pub fn print_diagnostics(
    issues: &[ScriptIssue],
    sources: &std::collections::HashMap<String, String>,
    use_color: bool,
) {
    for issue in issues {
        let source = sources.get(&issue.file).map(String::as_str);
        let diag = Diagnostic::new(issue)
            .with_color(use_color);

        let diag = match source {
            Some(s) => diag.with_source(s),
            None => diag,
        };

        eprint!("{}", diag.render());
    }
}

/// Check if stderr supports colors (simple heuristic).
pub fn stderr_supports_color() -> bool {
    // Check for common environment variables
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if std::env::var("CLICOLOR_FORCE").is_ok() {
        return true;
    }
    if std::env::var("TERM").map_or(false, |t| t == "dumb") {
        return false;
    }

    // Check COLORTERM for modern terminals
    if std::env::var("COLORTERM").is_ok() {
        return true;
    }

    // Check if TERM looks like it supports color
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("color") || term.contains("256") || term.contains("xterm") || term.contains("screen") {
            return true;
        }
    }

    // Default: assume color support on unix-like systems
    #[cfg(unix)]
    {
        true
    }

    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::error::IssueKind;

    #[test]
    fn test_diagnostic_render_without_source() {
        let issue = ScriptIssue::new(
            IssueKind::ParseError,
            "test.splash".into(),
            10,
            5,
            "unexpected token".into(),
        );

        let diag = Diagnostic::new(&issue).with_color(false);
        let output = diag.render();

        assert!(output.contains("error[E0002]: unexpected token"));
        assert!(output.contains("--> test.splash:10:5"));
    }

    #[test]
    fn test_diagnostic_render_with_source() {
        let issue = ScriptIssue::new(
            IssueKind::ParseError,
            "test.splash".into(),
            2,
            13,
            "undefined variable `z`".into(),
        );

        let source = "let x = 1\nlet y = x + z\n";
        let diag = Diagnostic::new(&issue)
            .with_source(source)
            .with_color(false);
        let output = diag.render();

        assert!(output.contains("let y = x + z"));
        assert!(output.contains("^"));
    }

    #[test]
    fn test_diagnostic_with_suggestion() {
        let issue = ScriptIssue::new(
            IssueKind::ParseError,
            "test.splash".into(),
            2,
            13,
            "undefined variable `z`".into(),
        )
        .with_suggestion("did you mean `x`?".into());

        let diag = Diagnostic::new(&issue).with_color(false);
        let output = diag.render();

        assert!(output.contains("help: did you mean `x`?"));
    }
}
