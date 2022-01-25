use {
    crate::{
        makepad_platform::*,
        app_inner::AppInner,
        app_state::AppState,
        design_editor::{
            inline_widget::InlineWidgetRegistry
        }
    },
};

live_register!{
    App: {{App}} {
        const FS_ROOT: ""
        inner: {
            collab_client: {
                //bind: "127.0.0.1"
                path: (FS_ROOT) 
            }
            builder_client: {
                //bind: "127.0.0.1"
                path: (FS_ROOT)
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    inner: AppInner,
    inline_widget_registry: InlineWidgetRegistry,
    #[rust(AppState::new())] state: AppState,
}

impl App {
    
    pub fn live_register(cx: &mut Cx) {
        makepad_studio_component::live_register(cx);
        crate::builder::builder_client::live_register(cx);
        crate::collab::collab_client::live_register(cx);
        crate::design_editor::live_register(cx);
        crate::log_view::live_register(cx);
        crate::code_editor::code_editor_impl::live_register(cx);
        crate::editors::live_register(cx);
        crate::app_inner::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        let ret = Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap();
        ret
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.handle_live_edit_event(cx, event, id!(App));
        self.inner.handle_event(cx, event, &mut self.state);
    }
}
