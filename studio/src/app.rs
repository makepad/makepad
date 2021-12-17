use {
    crate::{
        app_inner::AppInner,
        app_state::AppState,
        design_editor::{
            live_widget::LiveWidgetRegistry
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
    live_widget_registry: LiveWidgetRegistry,
    #[rust(AppState::new())] state: AppState,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_widget::live_register(cx);
        crate::design_editor::live_widget::live_color_picker::live_register(cx);
        crate::design_editor::live_widget::live_widget::live_register(cx);
        crate::code_editor::code_editor_impl::live_register(cx);
        crate::design_editor::live_editor::live_register(cx);
        crate::editors::live_register(cx);
        crate::app_inner::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        
        Self::new_from_module_path_id(cx, &module_path!(), id!(App)).unwrap()
        
    }
    
    pub fn draw(&mut self, cx: &mut Cx) {
        //cx.profile_start(0);
        self.inner.draw(cx, &mut self.state);
        //cx.profile_end(0);
        //cx.debug_draw_tree(false);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.inner.handle_event(cx, event, &mut self.state);
    }
}
