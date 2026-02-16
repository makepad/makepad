/// Script parsing and validation using makepad_script.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use makepad_script::{
    makepad_error_log,
    parser::ScriptParser,
    tokenizer::ScriptTokenizer,
    ScriptHeap,
    ScriptValue,
};

use super::cargo::{find_manifest_path, read_cargo_metadata, select_package};
use super::diagnostic::{print_diagnostics, stderr_supports_color};
use super::error::{CheckError, IssueKind, ScriptIssue};
use super::parser::{
    discover_script_blocks, max_rust_value_index, normalize_rust_interpolations,
    ScriptBlock, ScriptMacroKind,
};
use super::analyzer::analyze_opcodes;
use super::runtime::{build_harness_plan, run_harness};

thread_local! {
    /// Thread-local storage for capturing script parse errors.
    static PARSE_ERRORS: RefCell<Vec<ScriptIssue>> = RefCell::new(Vec::new());
}

/// Logger that captures parse errors into thread-local storage.
fn capture_parse_errors(
    file_name: &str,
    line_start: u32,
    column_start: u32,
    _line_end: u32,
    _column_end: u32,
    message: String,
    level: makepad_error_log::LogLevel,
) {
    if matches!(level, makepad_error_log::LogLevel::Error | makepad_error_log::LogLevel::Warning) {
        PARSE_ERRORS.with(|cell| {
            cell.borrow_mut().push(ScriptIssue::new(
                IssueKind::ParseError,
                file_name.to_string(),
                line_start + 1,
                column_start + 1,
                message,
            ));
        });
    }
}

/// Result from parsing script code with full opcode information.
pub struct ParseResult {
    /// Parse errors/warnings.
    pub issues: Vec<ScriptIssue>,
    /// Opcode stream from parser.
    pub opcodes: Vec<ScriptValue>,
    /// Source map: opcode index -> token index.
    pub source_map: Vec<Option<u32>>,
    /// File name for diagnostics.
    pub file: String,
    /// Tokenizer positions: token index -> (row, col).
    tokenizer_positions: Vec<(u32, u32)>,
}

impl ParseResult {
    /// Get source location (line, column) for an opcode index.
    pub fn get_location(&self, opcode_idx: usize) -> (u32, u32) {
        if let Some(Some(tok_idx)) = self.source_map.get(opcode_idx) {
            if let Some(&(row, col)) = self.tokenizer_positions.get(*tok_idx as usize) {
                return (row + 1, col + 1);
            }
        }
        (1, 1)
    }
}

/// Parse script code, returning full parse result including opcodes.
pub fn parse_script_full(code: &str, file_name: &str) -> ParseResult {
    // Clear previous errors
    PARSE_ERRORS.with(|cell| cell.borrow_mut().clear());

    // Install capture logger
    let old_logger = {
        let mut guard = makepad_error_log::LOG_WITH_LEVEL.write().expect("Logger lock poisoned");
        let old = *guard;
        *guard = capture_parse_errors;
        old
    };

    // Run parser
    let mut heap = ScriptHeap::empty();
    let mut tokenizer = ScriptTokenizer::default();
    let mut parser = ScriptParser::default();
    let normalized = normalize_rust_interpolations(code);

    tokenizer.tokenize(&normalized, &mut heap);
    let max_idx = max_rust_value_index(&normalized);
    let values: Vec<ScriptValue> = (0..=max_idx).map(|_| ScriptValue::default()).collect();
    parser.parse(&tokenizer, file_name, (0, 0), &values);

    // Extract token positions before restoring logger
    let tokenizer_positions: Vec<(u32, u32)> = (0..tokenizer.tokens.len())
        .filter_map(|i| tokenizer.token_index_to_row_col(i as u32))
        .collect();

    // Restore old logger
    {
        let mut guard = makepad_error_log::LOG_WITH_LEVEL.write().expect("Logger lock poisoned");
        *guard = old_logger;
    }

    let issues = PARSE_ERRORS.with(|cell| cell.take());

    ParseResult {
        issues,
        opcodes: parser.opcodes.clone(),
        source_map: parser.source_map.clone(),
        file: file_name.to_string(),
        tokenizer_positions,
    }
}

/// Run tokenize + parse on script code, capturing any parse errors.
#[allow(dead_code)]
fn parse_script(code: &str, file_name: &str) -> Vec<ScriptIssue> {
    parse_script_full(code, file_name).issues
}

/// Run parse and semantic analysis checks for script blocks.
fn run_parse_checks(blocks: &[ScriptBlock]) -> Vec<ScriptIssue> {
    let mut issues = Vec::new();

    for block in blocks {
        let file_name = block.path.display().to_string();
        let result = parse_script_full(&block.code, &file_name);

        // Run semantic analysis on the opcodes first (before consuming result.issues)
        let mut analysis_issues = analyze_opcodes(&result, block.line_offset);
        issues.append(&mut analysis_issues);

        // Add parse errors (adjusted for block offset)
        for mut issue in result.issues {
            issue.line += block.line_offset;
            issues.push(issue);
        }
    }

    issues
}

/// Configuration for script checking.
#[derive(Default)]
pub struct CheckConfig {
    /// Working directory.
    pub cwd: PathBuf,
}

impl CheckConfig {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }
}

/// Result of script checking.
pub struct ScriptCheckResult {
    /// All issues found.
    pub issues: Vec<ScriptIssue>,
    /// Number of errors (non-warning issues).
    pub error_count: usize,
    /// Number of warnings.
    pub warning_count: usize,
    /// Source files for diagnostic context.
    pub sources: HashMap<String, String>,
}

impl ScriptCheckResult {
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    /// Print all issues with rich diagnostic output.
    pub fn print_issues(&self) {
        let use_color = stderr_supports_color();
        print_diagnostics(&self.issues, &self.sources, use_color);
    }

    /// Print summary line.
    pub fn print_summary(&self) {
        if self.error_count > 0 || self.warning_count > 0 {
            let mut parts = Vec::new();
            if self.error_count > 0 {
                parts.push(format!(
                    "{} error{}",
                    self.error_count,
                    if self.error_count == 1 { "" } else { "s" }
                ));
            }
            if self.warning_count > 0 {
                parts.push(format!(
                    "{} warning{}",
                    self.warning_count,
                    if self.warning_count == 1 { "" } else { "s" }
                ));
            }
            eprintln!("{} generated", parts.join("; "));
        }
    }
}

/// Main entry point for script checking.
pub fn check_scripts(config: &CheckConfig) -> std::result::Result<ScriptCheckResult, CheckError> {
    let cwd = &config.cwd;

    // Discover all script blocks
    let blocks = discover_script_blocks(cwd);
    if blocks.is_empty() {
        return Err(CheckError::NoScriptSources);
    }

    // Collect source files for diagnostic context
    let mut sources = HashMap::<String, String>::new();
    for block in &blocks {
        let path_str = block.path.display().to_string();
        if !sources.contains_key(&path_str) {
            if let Ok(content) = std::fs::read_to_string(&block.path) {
                sources.insert(path_str, content);
            }
        }
    }

    let mut issues = Vec::<ScriptIssue>::new();
    // Check if widgets prelude is needed
    let needs_widgets_prelude = blocks.iter().any(|b| b.code.contains("mod.prelude.widgets"));

    // Check if any script_mod blocks exist
    let has_script_mod = blocks.iter().any(|b| b.macro_kind == ScriptMacroKind::ScriptMod);

    // Try runtime validation for script_mod blocks
    if has_script_mod {
        match try_runtime_validation(cwd, needs_widgets_prelude) {
            Ok((runtime_issues, _covered_files)) => {
                issues.extend(runtime_issues);
            }
            Err(warning) => {
                issues.push(ScriptIssue::fallback_warning(warning));
            }
        }
    }

    // Run parse checks for all blocks.
    // Runtime validation only executes reachable paths, so parser/semantic checks
    // must always run to catch static issues in dead/unreached code.
    let parse_blocks: Vec<ScriptBlock> = blocks
        .into_iter()
        .filter(|_block| true)
        .collect();

    let parse_issues = run_parse_checks(&parse_blocks);
    issues.extend(parse_issues);

    // Count errors and warnings
    let error_count = issues.iter().filter(|i| i.is_error()).count();
    let warning_count = issues.iter().filter(|i| i.is_warning()).count();

    Ok(ScriptCheckResult {
        issues,
        error_count,
        warning_count,
        sources,
    })
}

/// Try to run runtime validation, returning issues and covered files on success,
/// or an error message for fallback warning on failure.
fn try_runtime_validation(
    cwd: &Path,
    needs_widgets_prelude: bool,
) -> std::result::Result<(Vec<ScriptIssue>, HashSet<PathBuf>), String> {
    // Find and read cargo metadata
    let manifest_path = find_manifest_path(cwd)
        .ok_or_else(|| "No Cargo.toml found in current directory tree".to_string())?;

    let metadata = read_cargo_metadata(&manifest_path)
        .map_err(|e| format!("{}", e))?;

    let package = select_package(&metadata, &manifest_path, cwd)
        .ok_or_else(|| "Could not determine package for runtime script check".to_string())?;

    // Build harness plan
    let plan = build_harness_plan(package, cwd, &HashSet::new(), false, needs_widgets_prelude)
        .map_err(|e| format!("{}", e))?;

    if plan.modules.is_empty() {
        return Err(
            "Runtime script validation not applied: no resolvable script_mod modules found in package module tree; using parser fallback."
                .to_string()
        );
    }

    let covered_files = plan.covered_files();

    // Run the harness
    let runtime_issues = run_harness(&plan)
        .map_err(|e| format!("Runtime script validation unavailable: {}. Using parser fallback.", e))?;

    Ok((runtime_issues, covered_files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_script_basic() {
        let code = "x = 1\ny = 2";
        let issues = parse_script(code, "test.splash");
        // Should have no parse errors for valid script
        assert!(issues.is_empty() || issues.iter().all(|i| i.is_warning()));
    }

    #[test]
    fn test_parse_script_full_returns_opcodes() {
        let code = "x = 1";
        let result = parse_script_full(code, "test.splash");
        // Should have opcodes for the assignment
        assert!(!result.opcodes.is_empty(), "Expected opcodes to be generated");
        // Should have no parse errors
        assert!(result.issues.is_empty(), "Expected no parse errors");
    }

    #[test]
    fn test_parse_result_get_location() {
        let code = "x = 1\ny = 2";
        let result = parse_script_full(code, "test.splash");
        // Should be able to get location for first opcode
        let (line, col) = result.get_location(0);
        assert!(line >= 1, "Line should be at least 1");
        assert!(col >= 1, "Column should be at least 1");
    }
}
