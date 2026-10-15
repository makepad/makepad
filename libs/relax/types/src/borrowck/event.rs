//! Structured borrow errors and the application's diagnostic scope.
use crate::program::{DefId, Program};
use crate::types::TyId;
use relax_session::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub line: u32, pub col: u32, pub end_line: u32, pub end_col: u32,
    pub text: String, pub primary: bool, pub source: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowError {
    pub fn_path: String, pub owner: DefId, pub krate: u32,
    pub code: &'static str, pub message: String,
    pub file: String, pub line: u32, pub col: u32, pub end_line: u32, pub end_col: u32,
    pub labels: Vec<Label>, pub notes: Vec<String>, pub helps: Vec<String>, pub fv_only: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin { Background, Gate, Check, Serve, Run }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GandalfEvent {
    pub seq: u64, pub origin: Origin, pub src: usize, pub epoch: u64,
    pub args: Vec<TyId>, pub error: BorrowError,
}

impl BorrowError {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let head = if self.code.is_empty() { "error".to_string() } else { format!("error[{}]", self.code) };
        out.push_str(&format!("{}: {}\n", head, self.message));
        out.push_str(&format!("  --> {}:{}:{}\n", self.file, self.line, self.col));
        let mut labs: Vec<_> = self.labels.iter().collect();
        labs.sort_by_key(|x| (x.line, x.col));
        let gutter = labs.iter().map(|x| x.line).max().unwrap_or(self.line).to_string().len();
        let pad = " ".repeat(gutter);
        out.push_str(&format!("{} |\n", pad));
        let mut last_line = 0u32;
        for l in labs {
            if l.line != last_line {
                if last_line != 0 && l.line > last_line + 1 { out.push_str("...\n"); }
                out.push_str(&format!("{:>g$} | {}\n", l.line, l.source, g = gutter));
                last_line = l.line;
            }
            let width = if l.end_line == l.line { l.end_col.saturating_sub(l.col).max(1) } else { 1 };
            let mark = if l.primary { "^" } else { "-" };
            out.push_str(&format!("{} | {}{} {}\n", pad, " ".repeat(l.col.saturating_sub(1) as usize), mark.repeat(width as usize), l.text));
        }
        out.push_str(&format!("{} |\n", pad));
        for n in &self.notes { out.push_str(&format!("{} = note: {}\n", pad, n)); }
        for h in &self.helps { out.push_str(&format!("{} = help: {}\n", pad, h)); }
        out
    }
    pub fn one_line(&self) -> String {
        let code = if self.code.is_empty() { String::new() } else { format!("[{}]", self.code) };
        format!("{}:{}:{}: error{}: {}", self.file, self.line, self.col, code, self.message)
    }
    pub fn to_diagnostic(&self) -> crate::diag::Diagnostic {
        let mut d = crate::diag::Diagnostic::new("borrowck", &self.file, self.line, self.col, &self.message);
        d.code = if self.code.is_empty() { "RB0001" } else { self.code }.into();
        d.end_line = self.end_line; d.end_col = self.end_col;
        d.notes.extend(self.notes.clone());
        d.notes.extend(self.labels.iter().map(|l| l.text.clone()));
        if !self.helps.is_empty() { d.suggestion = Some(crate::diag::Suggestion { message: self.helps.join("\n"), replacement: None }); }
        d.item = Some(self.fn_path.clone());
        d
    }
    pub fn mentions(&self, name: &str) -> bool {
        let needle = format!("`{}`", name);
        self.message.contains(&needle) || self.labels.iter().any(|l| l.text.contains(&needle)) || self.notes.iter().any(|n| n.contains(&needle))
    }
}

/// Compiled default: only the application's crates refuse live edits and publish events.
pub const APP_CRATES_ONLY: bool = true;
static APP_SCOPE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The application's crates, as their normalized root source paths. The compiler computes
/// them with `reload::gandalf_app_roots` from the unit graph (or the root crate alone when
/// there is none); this is the one app-crate rule.
pub fn set_app_scope(roots: Vec<String>) {
    let _k = crate::zone_alloc::kept();
    let roots = roots.iter().map(|r| crate::vfs::normalize_path(r)).collect();
    *APP_SCOPE.lock().unwrap_or_else(|e| e.into_inner()) = roots;
}

pub fn app_crate(prog: &Program, krate: u32) -> bool {
    let root = crate::vfs::normalize_path(prog.crates[krate as usize].root_path());
    APP_SCOPE.lock().unwrap_or_else(|e| e.into_inner()).contains(&root)
}

/// The one all-crates switch: the compiled APP_CRATES_ONLY, with MAKEPAD_RUN_GANDALF_ALL=1 on top.
pub fn all_crates() -> bool {
    !APP_CRATES_ONLY || std::env::var("MAKEPAD_RUN_GANDALF_ALL").as_deref() == Ok("1")
}

/// Whether a crate's borrow errors are reported (and refuse live edits).
pub fn admit(prog: &Program, krate: u32) -> bool { all_crates() || app_crate(prog, krate) }
