pub mod build_manager;
pub mod file_system;

//pub use makepad_code_editor;
pub use makepad_file_protocol;
pub use makepad_file_server;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_draw::makepad_platform;
pub use makepad_platform::makepad_micro_serde;
pub use makepad_platform::makepad_live_id;
pub use makepad_code_editor;
pub use makepad_shell;

#[cfg(target_arch = "wasm32")]
pub use makepad_platform::makepad_wasm_bridge;
pub use makepad_platform::makepad_live_compiler;
pub use makepad_platform::makepad_math;

