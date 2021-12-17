use {
    makepad_render::*,
    makepad_widget::color_picker::*,
    crate::{
        design_editor::{
            inline_widget::*
        }
    }
};

live_register!{
    InlineColorPicker: {{InlineColorPicker}} {
    }
}

fn register_factory(cx: &mut Cx) {
    struct Factory();
    impl InlineWidgetFactory for Factory {
        fn new(&self, cx: &mut Cx) -> Box<dyn InlineWidget> {
            Box::new(InlineColorPicker::new(cx))
        }
        
        fn can_edit_value(&self, _live_registry: &LiveRegistry, node: &LiveNode) -> CanEdit {
            if let LiveValue::Color(_) = &node.value {
                return CanEdit::Yes(100.0)
            }
            CanEdit::No
        }
    }
    cx.registries.clone().get_or_create::<CxInlineWidgetRegistry>().register(
        InlineColorPicker::live_type_info(cx),
        Box::new(Factory()),
        LiveId::from_str("color_picker").unwrap(),
    )
}

impl InlineWidget for InlineColorPicker {
    fn handle_inline_event(&mut self, cx: &mut Cx, event: &mut Event) -> InlineWidgetAction {
        self.color_picker.handle_event(cx, event);
        InlineWidgetAction::None
    }
    
    fn draw_inline(&mut self, cx: &mut Cx) {
        self.color_picker.size = 100.0;
        self.color_picker.draw(cx, Vec4::default(), 1.0);
    }
}

#[derive(Live, LiveHook)]
#[live_register(register_factory)]
pub struct InlineColorPicker {
    color_picker: ColorPicker
}