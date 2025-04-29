//pub mod app_inner;
//pub mod app_state;
pub mod app;
pub mod app_ui;
pub mod build_manager;
pub mod file_system;
pub mod studio_editor;
pub mod studio_file_tree;
pub mod log_list;
pub mod run_list;
pub mod run_view;
pub mod profiler;
pub mod integration;
pub mod ai_chat;
pub mod search;

//pub use makepad_code_editor;
pub use makepad_platform::log;
pub use makepad_widgets::makepad_draw;
pub use makepad_draw::makepad_platform;
pub use makepad_platform::makepad_micro_serde;
pub use makepad_platform::makepad_live_id;

#[cfg(target_arch = "wasm32")]
pub use makepad_platform::makepad_wasm_bridge;
//pub use makepad_platform::makepad_live_tokenizer;
pub use makepad_platform::makepad_live_compiler;
pub use makepad_platform::makepad_math;
//pub use makepad_editor_core;
//pub use makepad_code_editor;
pub use makepad_file_protocol;
pub use makepad_file_server;
pub use makepad_widgets;
pub use makepad_code_editor;
pub use makepad_shell;

