//pub mod app_inner;
//pub mod app_state;
pub mod ai_chat;
pub mod app;
pub mod app_ui;
pub mod build_manager;
pub mod file_system;
pub mod integration;
pub mod log_list;
pub mod profiler;
pub mod run_list;
pub mod run_view;
pub mod search;
pub mod snapshot;
pub mod studio_editor;
pub mod studio_file_tree;

// Re-export from widgets2 (the new system) - aliased as makepad_widgets
pub use makepad_widgets::makepad_draw;
pub use makepad_widgets::makepad_platform;
pub use makepad_widgets::makepad_platform::log;
pub use makepad_widgets::makepad_script;
pub use makepad_widgets::makepad_script::makepad_live_id;
pub use makepad_widgets::makepad_script::makepad_math;
pub use makepad_widgets::makepad_script::makepad_micro_serde;
pub use makepad_widgets2 as makepad_widgets;

#[cfg(target_arch = "wasm32")]
pub use makepad_widgets::makepad_platform::makepad_wasm_bridge;

pub use makepad_code_editor;
pub use makepad_file_protocol;
pub use makepad_file_server;
pub use makepad_shell;

use crate::makepad_widgets::*;

// All modules now use script_mod! - no more live_design! registrations needed
pub fn live_design(_cx: &mut Cx) {}

pub fn script_mod(vm: &mut ScriptVm) {
    crate::makepad_code_editor::script_mod(vm);
    crate::run_list::script_mod(vm);
    crate::log_list::script_mod(vm);
    crate::integration::script_mod(vm);
    crate::profiler::script_mod(vm);
    crate::run_view::script_mod(vm);
    crate::studio_editor::script_mod(vm);
    crate::studio_file_tree::script_mod(vm);
    crate::search::script_mod(vm);
    crate::snapshot::script_mod(vm);
    crate::ai_chat::ai_chat_view::script_mod(vm);
}
