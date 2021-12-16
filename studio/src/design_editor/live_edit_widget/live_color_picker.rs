use {
    makepad_render::*,
    crate::{
        code_editor::{
            live_edit_widget::*
        }
    }
};

live_register!{
    LiveColorPicker: {{LiveColorPicker}} {
    }
}

fn register_factory(cx: &mut Cx) {
    struct Factory();
    impl LiveEditWidgetFactory for Factory {
        fn new_from_factory(&self, cx: &mut Cx) -> Box<dyn LiveEditWidget> {
            Box::new(LiveColorPicker::new(cx))
        }
        
        fn matches_value(&self, _live_registry: &LiveRegistry, _node: &LiveNode) -> bool {
            false
        }
    }
    cx.registries.register_live_edit_widget(
        LiveType::of::<LiveColorPicker>(),
        Box::new(Factory()),
        "color_picker",
        LiveEditWidgetPrio(1),
    )
}

impl LiveEditWidget for LiveColorPicker {
    fn handle_event_dyn(&mut self, _cx: &mut Cx, _event: &mut Event) -> LiveEditWidgetAction {
        LiveEditWidgetAction::None
    }
    fn draw_dyn(&mut self, _cx: &mut Cx) {
        
    }
    
}

#[derive(Live, LiveHook)]
#[live_register_hook(register_factory)]
pub struct LiveColorPicker {
}

impl LiveColorPicker {
    pub fn draw(&mut self, _cx: &mut Cx) {
    }
    
    pub fn handle_event(
        &mut self,
        _cx: &mut Cx,
        _event: &mut Event,
    )->LiveEditWidgetAction{
        LiveEditWidgetAction::None
    }
}