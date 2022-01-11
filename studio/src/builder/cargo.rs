#![allow(dead_code)]
use{
    makepad_component::makepad_render::makepad_micro_serde::*,
};

// rust compiler output json structs
#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcTarget {
    kind: Vec<String>,
    crate_types: Vec<String>,
    name: String,
    src_path: String,
    edition: String,
    doc: Option<bool>,
    doctest: bool,
    test: bool
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcText {
    text: String,
    highlight_start: u32,
    highlight_end: u32
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcSpan {
    file_name: String,
    byte_start: u32,
    byte_end: u32,
    line_start: u32,
    line_end: u32,
    column_start: u32,
    column_end: u32,
    is_primary: bool,
    text: Vec<RustcText>,
    label: Option<String>,
    suggested_replacement: Option<String>,
    suggestion_applicability: Option<String>,
    expansion: Option<Box<RustcExpansion >>,
    level: Option<String>
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcExpansion {
    span: Option<RustcSpan>,
    macro_decl_name: String,
    def_site_span: Option<RustcSpan>
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcCode {
    code: String,
    explanation: Option<String>
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcMessage {
    message: String,
    code: Option<RustcCode>,
    level: String,
    spans: Vec<RustcSpan>,
    children: Vec<RustcMessage>,
    rendered: Option<String>
}

#[derive(Clone, DeJson, Debug, Default)]
pub struct RustcProfile {
    opt_level: String,
    debuginfo: Option<u32>,
    debug_assertions: bool,
    overflow_checks: bool,
    test: bool
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
