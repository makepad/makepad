
pub use makepad_widgets::makepad_platform;
pub use makepad_platform::makepad_math;
pub use makepad_widgets::makepad_draw;
pub use makepad_platform::makepad_error_log;

pub mod code_editor;
pub mod state;
pub mod str_ext;

pub use self::{code_editor::CodeEditor, state::State, str_ext::StrExt};
