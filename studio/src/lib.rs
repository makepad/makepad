//pub mod app_inner;
//pub mod app_state;
pub mod app;
pub mod file_system;
pub mod build_manager;

//pub mod code_editor;
//pub mod editors;
//pub mod editor_state;
//pub mod log_view;
//pub mod rust_editor;
pub mod run_view;

//pub use makepad_code_editor;
pub use makepad_file_protocol;
pub use makepad_file_server;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_draw::makepad_platform;
pub use makepad_platform::makepad_micro_serde;
pub use makepad_platform::makepad_live_id;
pub use makepad_platform::makepad_error_log;
pub use makepad_code_editor;

#[cfg(target_arch = "wasm32")]
pub use makepad_platform::makepad_wasm_bridge;
//pub use makepad_platform::makepad_live_tokenizer;
pub use makepad_platform::makepad_live_compiler;
pub use makepad_platform::makepad_math;
//pub use makepad_editor_core;
