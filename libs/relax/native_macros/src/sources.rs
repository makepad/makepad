// Exact sources from the Makepad packages linked by Cargo, captured at build time.
include!(concat!(env!("OUT_DIR"), "/sources.rs"));

// Accepted proc-macro dependency graph (the optional relax-tokens feature is off there).
pub static DEPENDENCIES: &[(&str, &[&str])] = &[
    ("makepad_micro_proc_macro", &[]),
    ("makepad_live_id", &["makepad_live_id_macros", "makepad_relax_api"]),
    ("makepad_live_id_macros", &["makepad_live_id_macros_impl"]),
    ("makepad_live_id_macros_impl", &["makepad_micro_proc_macro"]),
    ("makepad_micro_serde_derive", &["makepad_micro_serde_derive_impl", "makepad_micro_proc_macro"]),
    ("makepad_micro_serde_derive_impl", &["makepad_micro_proc_macro"]),
    ("makepad_script_derive", &["makepad_micro_proc_macro", "makepad_script_derive_impl"]),
    ("makepad_script_derive_impl", &["makepad_micro_proc_macro"]),
    ("makepad_derive_widget", &["makepad_derive_widget_impl", "makepad_micro_proc_macro", "makepad_live_id"]),
    ("makepad_derive_widget_impl", &["makepad_micro_proc_macro"]),
    ("makepad_relax_api", &["makepad_relax_api_macro"]),
    ("makepad_relax_api_macro", &["makepad_relax_api_macro_impl"]),
    ("makepad_relax_api_macro_impl", &["makepad_micro_proc_macro"]),
];
