pub mod app;
pub mod app_data;
pub mod app_ui;
pub mod desktop_code_editor;
pub mod desktop_file_tree;
pub mod desktop_log_view;
pub mod desktop_profiler_view;
pub mod desktop_run_list;
pub mod desktop_run_view;
pub mod desktop_terminal_view;

pub use makepad_code_editor;
pub use makepad_studio_backend;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_widgets::makepad_platform;
pub use makepad_widgets::makepad_platform::log;
pub use makepad_widgets::makepad_script;
pub use makepad_widgets::makepad_script::makepad_live_id;
pub use makepad_widgets::makepad_script::makepad_micro_serde;

use crate::makepad_widgets::*;

pub fn script_mod(vm: &mut ScriptVm) {
    crate::desktop_file_tree::script_mod(vm);
    crate::desktop_code_editor::script_mod(vm);
    crate::desktop_log_view::script_mod(vm);
    crate::desktop_profiler_view::script_mod(vm);
    crate::desktop_run_list::script_mod(vm);
    crate::desktop_run_view::script_mod(vm);
    crate::desktop_terminal_view::script_mod(vm);
    crate::app_ui::script_mod(vm);
}
