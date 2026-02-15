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

use super::cargo::{canonicalize_path, find_manifest_path, read_cargo_metadata, select_package};
use super::error::{CheckError, IssueKind, ScriptIssue};
use super::parser::{
    discover_script_blocks, max_rust_value_index, normalize_rust_interpolations,
    ScriptBlock, ScriptMacroKind,
};
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

/// Run tokenize + parse on script code, capturing any parse errors.
fn parse_script(code: &str, file_name: &str) -> Vec<ScriptIssue> {
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

    // Restore old logger
    {
        let mut guard = makepad_error_log::LOG_WITH_LEVEL.write().expect("Logger lock poisoned");
        *guard = old_logger;
    }

    // Return captured errors
    PARSE_ERRORS.with(|cell| cell.take())
}

/// Count script_mod blocks per file.
fn count_script_mod_blocks(blocks: &[ScriptBlock]) -> HashMap<PathBuf, usize> {
    let mut counts = HashMap::new();
    for block in blocks {
        if block.macro_kind == ScriptMacroKind::ScriptMod {
            let canon = canonicalize_path(&block.path);
            *counts.entry(canon).or_insert(0) += 1;
        }
    }
    counts
}


/// Run parse-only checks for script blocks.
fn run_parse_checks(blocks: &[ScriptBlock]) -> Vec<ScriptIssue> {
    let mut issues = Vec::new();

    for block in blocks {
        let mut block_issues = parse_script(&block.code, &block.path.display().to_string());

        // Adjust line numbers by block offset
        for issue in &mut block_issues {
            issue.line += block.line_offset;
        }

        issues.extend(block_issues);
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
}

impl ScriptCheckResult {
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    pub fn print_issues(&self) {
        for issue in &self.issues {
            if issue.is_warning() {
                eprintln!("warning: {}", issue.message);
            } else {
                eprintln!("{}", issue);
            }
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

    let mut issues = Vec::<ScriptIssue>::new();
    let mut runtime_checked_files = HashSet::<PathBuf>::new();

    // Track script_mod counts per file
    let script_mod_counts = count_script_mod_blocks(&blocks);

    // Check if widgets prelude is needed
    let needs_widgets_prelude = blocks.iter().any(|b| b.code.contains("mod.prelude.widgets"));

    // Check if any script_mod blocks exist
    let has_script_mod = blocks.iter().any(|b| b.macro_kind == ScriptMacroKind::ScriptMod);

    // Try runtime validation for script_mod blocks
    if has_script_mod {
        match try_runtime_validation(cwd, needs_widgets_prelude) {
            Ok((runtime_issues, covered_files)) => {
                issues.extend(runtime_issues);
                runtime_checked_files = covered_files;
            }
            Err(warning) => {
                issues.push(ScriptIssue::fallback_warning(warning));
            }
        }
    }

    // Run parse checks for blocks not covered by runtime validation
    let parse_blocks: Vec<ScriptBlock> = blocks
        .into_iter()
        .filter(|block| {
            match block.macro_kind {
                // Always parse-check script! blocks
                ScriptMacroKind::Script => true,
                // Parse-check script_mod! if not runtime-checked, or if file has multiple blocks
                ScriptMacroKind::ScriptMod => {
                    let canon = canonicalize_path(&block.path);
                    if !runtime_checked_files.contains(&canon) {
                        true
                    } else {
                        // If file has multiple script_mod blocks, we need parse check too
                        script_mod_counts.get(&canon).copied().unwrap_or(0) > 1
                    }
                }
            }
        })
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
}
