#![allow(dead_code)]
use crate::{
    makepad_micro_serde::*,
    makepad_live_tokenizer::{Range,Position},
};

// rust compiler output json structs
#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcTarget {
    pub kind: Vec<String>,
    pub crate_types: Vec<String>,
    pub name: String,
    pub src_path: String,
    pub edition: String,
    pub doc: Option<bool>,
    pub doctest: bool,
    pub test: bool
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcText {
    pub text: String,
    pub highlight_start: usize, 
    pub highlight_end: usize
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcSpan {
    pub file_name: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_start: usize,
    pub line_end: usize,
    pub column_start: usize,
    pub column_end: usize,
    pub is_primary: bool,
    pub text: Vec<RustcText>,
    pub label: Option<String>,
    pub suggested_replacement: Option<String>,
    pub suggestion_applicability: Option<String>,
    pub expansion: Option<Box<RustcExpansion >>,
    pub level: Option<String>
}

impl RustcSpan{
    pub fn to_range(&self)->Range{
        Range{
            start:Position{
                line:self.line_start - 1,
                column: self.column_start - 1
            },
            end:Position{
                line:self.line_end - 1,
                column:self.column_end - 1
            }
        }
    }
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcExpansion {
    pub span: Option<RustcSpan>,
    pub macro_decl_name: String,
    pub def_site_span: Option<RustcSpan>
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcCode {
    pub code: String,
    pub explanation: Option<String>
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcMessage {
    pub message: String,
    pub code: Option<RustcCode>,
    pub level: String,
    pub spans: Vec<RustcSpan>,
    pub children: Vec<RustcMessage>,
    pub rendered: Option<String>
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcProfile {
    pub opt_level: String,
    pub debuginfo: Option<usize>,
    pub debug_assertions: bool,
    pub overflow_checks: bool,
    pub test: bool
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcCompilerMessage {
    pub reason: String,
    pub success: Option<bool>,
    pub package_id: Option<String>,
    pub manifest_path: Option<String>,
    pub linked_libs: Option<Vec<String >>,
    pub linked_paths: Option<Vec<String >>,
    pub cfgs: Option<Vec<String >>,
    pub env: Option<Vec<String >>,
    pub target: Option<RustcTarget>,
    pub message: Option<RustcMessage>,
    pub profile: Option<RustcProfile>,
    pub out_dir: Option<String>,
    pub features: Option<Vec<String >>,
    pub filenames: Option<Vec<String >>,
    pub executable: Option<String>,
    pub fresh: Option<bool>
}
