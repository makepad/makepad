use {
    crate::{
        app_inner::AppInner,
        app_state::AppState,
        design_editor::{
            inline_widget::InlineWidgetRegistry
        }
    },
    makepad_render::*,
};

live_register!{
    App: {{App}} {
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
        makepad_widget::live_register(cx);
        crate::design_editor::live_register(cx);
        crate::code_editor::code_editor_impl::live_register(cx);
        crate::editors::live_register(cx);
        crate::app_inner::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        cx.profile_start(1);
        let ret = Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap();
        cx.profile_end(1);
        ret
    }
    
    pub fn draw(&mut self, cx: &mut Cx) {
        self.inner.draw(cx, &mut self.state);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.handle_live_edit_event(cx, event, id!(App));
        self.inner.handle_event(cx, event, &mut self.state);
    }
}
