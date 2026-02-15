//! Semantic analysis for Splash scripts.
//!
//! Walks opcode stream to build symbol tables and detect issues like
//! undefined variables and unused bindings.

use std::collections::HashMap;

use makepad_script::opcode::Opcode;

use super::error::{IssueKind, ScriptIssue};
use super::script::ParseResult;

/// A symbol definition in the script.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The symbol name.
    pub name: String,
    /// Line number where defined.
    pub line: u32,
    /// Column number where defined.
    pub column: u32,
    /// Whether this symbol has been used.
    pub used: bool,
}

/// Scope-aware symbol table for tracking variable definitions.
pub struct SymbolTable {
    /// Stack of scopes, each containing symbol definitions.
    scopes: Vec<HashMap<String, Symbol>>,
}

impl SymbolTable {
    /// Create a new symbol table with a global scope.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    /// Push a new scope onto the stack.
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pop the current scope and return its symbols.
    pub fn pop_scope(&mut self) -> Vec<Symbol> {
        self.scopes.pop().unwrap_or_default().into_values().collect()
    }

    /// Define a new symbol in the current scope.
    pub fn define(&mut self, name: &str, line: u32, column: u32) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), Symbol {
                name: name.to_string(),
                line,
                column,
                used: false,
            });
        }
    }

    /// Check if a symbol is defined in any scope.
    pub fn is_defined(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|s| s.contains_key(name))
    }

    /// Mark a symbol as used.
    pub fn mark_used(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(sym) = scope.get_mut(name) {
                sym.used = true;
                return;
            }
        }
    }

    /// Lookup a symbol by name.
    #[allow(dead_code)]
    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    /// Get all defined symbol names for suggestions.
    pub fn all_names(&self) -> Vec<&str> {
        self.scopes.iter()
            .flat_map(|s| s.keys().map(String::as_str))
            .collect()
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Levenshtein edit distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();

    if m == 0 { return n; }
    if n == 0 { return m; }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1)
                .min(curr[j - 1] + 1)
                .min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Suggest a similar name using Levenshtein distance.
/// Returns None if no close match found (distance > 3, distance >= name length, or exact match).
pub fn suggest_similar<'a>(name: &str, candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .filter_map(|&c| {
            let dist = levenshtein(name, c);
            // Exclude exact matches (dist == 0) and distant matches
            if dist > 0 && dist <= 3 && dist < name.len() {
                Some((c, dist))
            } else {
                None
            }
        })
        .min_by_key(|(_, d)| *d)
        .map(|(c, _)| c)
}

/// Check if a name is a built-in that shouldn't be flagged as undefined.
fn is_builtin(name: &str) -> bool {
    matches!(name,
        "print" | "println" | "log" | "mod" | "self" | "true" | "false" | "null" |
        "Math" | "String" | "Array" | "Object" | "Number" | "Boolean" |
        "console" | "JSON" | "Date" | "RegExp" | "Error" |
        "parseInt" | "parseFloat" | "isNaN" | "isFinite"
    )
}

/// Analyze opcodes for semantic issues.
pub fn analyze_opcodes(result: &ParseResult, line_offset: u32) -> Vec<ScriptIssue> {
    let mut issues = Vec::new();
    let mut table = SymbolTable::new();
    let opcodes = &result.opcodes;

    let mut i = 0;
    while i < opcodes.len() {
        let value = opcodes[i];

        if let Some((opcode, _args)) = value.as_opcode() {
            match opcode {
                // Variable definitions
                Opcode::LET_DYN | Opcode::LET_TYPED => {
                    // Next value should be the variable name as LiveId
                    if i + 1 < opcodes.len() {
                        if let Some(name_id) = opcodes[i + 1].as_id() {
                            let name = format!("{}", name_id);
                            let (line, col) = result.get_location(i);
                            table.define(&name, line + line_offset, col);
                        }
                    }
                }
                // Variable references (dynamic)
                Opcode::VAR_DYN | Opcode::VAR_TYPED => {
                    if i + 1 < opcodes.len() {
                        if let Some(name_id) = opcodes[i + 1].as_id() {
                            let name = format!("{}", name_id);
                            if !table.is_defined(&name) && !is_builtin(&name) {
                                let (line, col) = result.get_location(i);
                                let all_names = table.all_names();
                                let suggestion = suggest_similar(&name, &all_names);

                                let mut issue = ScriptIssue::new(
                                    IssueKind::RuntimeError,
                                    result.file.clone(),
                                    line + line_offset,
                                    col,
                                    format!("undefined variable `{}`", name),
                                );

                                if let Some(s) = suggestion {
                                    issue = issue.with_suggestion(format!("did you mean `{}`?", s));
                                }

                                issues.push(issue);
                            } else {
                                table.mark_used(&name);
                            }
                        }
                    }
                }
                // Scope boundaries (function bodies)
                Opcode::FN_BODY_DYN | Opcode::FN_BODY_TYPED => {
                    table.push_scope();
                }
                Opcode::RETURN => {
                    // Check for unused variables when exiting scope
                    let unused = table.pop_scope();
                    for sym in unused {
                        if !sym.used && !sym.name.starts_with('_') {
                            issues.push(
                                ScriptIssue::new(
                                    IssueKind::FallbackWarning,
                                    result.file.clone(),
                                    sym.line,
                                    sym.column,
                                    format!("unused variable `{}`", sym.name),
                                )
                                .with_suggestion(format!("prefix with underscore: `_{}`", sym.name))
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }

    // Check global scope for unused variables at the end
    while table.scopes.len() > 0 {
        let unused = table.pop_scope();
        for sym in unused {
            if !sym.used && !sym.name.starts_with('_') {
                issues.push(
                    ScriptIssue::new(
                        IssueKind::FallbackWarning,
                        result.file.clone(),
                        sym.line,
                        sym.column,
                        format!("unused variable `{}`", sym.name),
                    )
                    .with_suggestion(format!("prefix with underscore: `_{}`", sym.name))
                );
            }
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_table_tracks_definitions() {
        let mut table = SymbolTable::new();
        table.define("x", 1, 1);
        assert!(table.is_defined("x"));
        assert!(!table.is_defined("y"));
    }

    #[test]
    fn test_symbol_table_scope_push_pop() {
        let mut table = SymbolTable::new();
        table.define("x", 1, 1);
        table.push_scope();
        table.define("y", 2, 1);
        assert!(table.is_defined("x"));
        assert!(table.is_defined("y"));
        table.pop_scope();
        assert!(table.is_defined("x"));
        assert!(!table.is_defined("y"));
    }

    #[test]
    fn test_symbol_table_mark_used() {
        let mut table = SymbolTable::new();
        table.define("x", 1, 1);
        assert!(!table.lookup("x").unwrap().used);
        table.mark_used("x");
        assert!(table.lookup("x").unwrap().used);
    }

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "ab"), 1);
        assert_eq!(levenshtein("abc", "abcd"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn test_suggest_similar() {
        let names = vec!["count", "counter", "total"];
        assert_eq!(suggest_similar("coutn", &names), Some("count"));
        assert_eq!(suggest_similar("cont", &names), Some("count"));
        assert_eq!(suggest_similar("xyz", &names), None); // too different
    }

    #[test]
    fn test_suggest_similar_exact_match_excluded() {
        let names = vec!["x", "y", "z"];
        // Distance 0 means exact match, which shouldn't be suggested
        assert_eq!(suggest_similar("x", &names), None);
    }

    #[test]
    fn test_analyze_opcodes_defined_variable_no_error() {
        use crate::check::script::parse_script_full;

        let code = "x = 1\ny = x + 1";
        let result = parse_script_full(code, "test.splash");
        let issues = analyze_opcodes(&result, 0);

        // Should have no undefined variable errors for properly defined vars
        let undefined_errors: Vec<_> = issues.iter()
            .filter(|i| i.message.contains("undefined"))
            .collect();
        assert!(undefined_errors.is_empty(), "Expected no undefined variable errors, got: {:?}", undefined_errors);
    }

    #[test]
    fn test_analyze_opcodes_basic_parsing() {
        use crate::check::script::parse_script_full;

        let code = "x = 1";
        let result = parse_script_full(code, "test.splash");

        // Verify opcodes were generated
        assert!(!result.opcodes.is_empty(), "Expected opcodes to be generated");

        // Run analysis - should not panic
        let _issues = analyze_opcodes(&result, 0);
    }
}
