use {
    crate::{
        makepad_platform::*,
        app_inner::AppInner,
        app_state::AppState,
    },
    //makepad_regex::regex::Regex
};

pub mod app_inner;
pub mod app_state;

pub mod collab_client;
pub mod build;

pub mod code_editor;
pub mod editors;
pub mod editor_state;
pub mod log_view;
pub mod rust_editor;
pub mod shader_view;
pub mod run_view;

pub use makepad_collab_protocol;
pub use makepad_collab_server;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_draw::makepad_platform;
pub use makepad_platform::makepad_micro_serde;
pub use makepad_platform::makepad_live_id;
pub use makepad_platform::makepad_error_log;

#[cfg(target_arch = "wasm32")]
pub use makepad_platform::makepad_wasm_bridge;
//pub use makepad_platform::makepad_live_tokenizer;
pub use makepad_platform::makepad_live_compiler;
pub use makepad_platform::makepad_math;
pub use makepad_editor_core;

live_design!{
    import makepad_widgets::theme::*;
    App= {{App}} {
        const FS_ROOT = ""
        inner: {
            window: {caption = "Makepad Studio", pass: {clear_color: (COLOR_BG_EDITOR)}}
            collab_client: {
                //bind: "127.0.0.1"
                path: (FS_ROOT)
            }
            build_manager: {
                //bind: "127.0.0.1"
                path: (FS_ROOT)
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    inner: AppInner,
    #[rust(AppState::new())] app_state: AppState,
}

app_main!(App); 

impl App {
    pub fn live_design(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        crate::build::build_manager::live_design(cx);
        crate::collab_client::live_design(cx);
        crate::rust_editor::live_design(cx);
        crate::log_view::live_design(cx);
        crate::shader_view::live_design(cx);
        crate::run_view::live_design(cx);
        crate::code_editor::code_editor_impl::live_design(cx);
        crate::editors::live_design(cx);
        crate::app_inner::live_design(cx);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.handle_live_edit_event(cx, event, live_id!(App));
        self.inner.handle_event(cx, event, &mut self.app_state);
    }
}
