#![allow(dead_code)]
use{
    makepad_component::makepad_render::makepad_micro_serde::*,
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
    pub highlight_start: u32,
    pub highlight_end: u32
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcSpan {
    pub file_name: String,
    pub byte_start: u32,
    pub byte_end: u32,
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
    pub is_primary: bool,
    pub text: Vec<RustcText>,
    pub label: Option<String>,
    pub suggested_replacement: Option<String>,
    pub suggestion_applicability: Option<String>,
    pub expansion: Option<Box<RustcExpansion >>,
    pub level: Option<String>
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
    pub debuginfo: Option<u32>,
    pub debug_assertions: bool,
    pub overflow_checks: bool,
    pub test: bool
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcCompilerMessage {
    reason: String,
    success: Option<bool>,
    //package_id: Option<String>,
    manifest_path: Option<String>,
    linked_libs: Option<Vec<String >>,
    linked_paths: Option<Vec<String >>,
    cfgs: Option<Vec<String >>,
    env: Option<Vec<String >>,
    target: Option<RustcTarget>,
    message: Option<RustcMessage>,
    profile: Option<RustcProfile>,
    out_dir: Option<String>,
    features: Option<Vec<String >>,
    filenames: Option<Vec<String >>,
    executable: Option<String>,
    fresh: Option<bool>
}
