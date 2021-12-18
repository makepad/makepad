use {
    makepad_render::*,
    makepad_widget::color_picker::*,
    crate::{
        design_editor::{
            inline_widget::*
        },
        code_editor::{
            position::Position,
            text::Text,
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
        
        fn can_edit_value(&self, live_registry: &LiveRegistry, live_ptr: LivePtr) -> CanEdit {
            let node = live_registry.ptr_to_node(live_ptr);
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
    fn handle_inline_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        live_ptr: LivePtr,
    ) -> InlineWidgetAction {
        
        match self.color_picker.handle_event(cx, event) {
            ColorPickerAction::Change {rgba} => {
                let live_registry_rc = cx.live_registry.clone();
                let live_registry = live_registry_rc.borrow();
                
                let mut s = String::new();
                s.push_str("#x");
                rgba.append_hex_to_string(&mut s);
                
                // alright we are going to fetch some tokens.
                let node = live_registry.ptr_to_node(live_ptr);
                let token_id = node.origin.token_id().unwrap();
                let doc = live_registry.token_id_to_origin_doc(token_id);
                let token = doc.tokens[token_id.token_index() + 2];
                
                let start_pos = Position::from(token.span.start);
                let end_pos = Position::from(token.span.end);
                
                return InlineWidgetAction::ReplaceText{
                    position: start_pos,
                    size: end_pos - start_pos,
                    text: Text::from(s)
                }
            }
            _ => ()
        }
        InlineWidgetAction::None
    }
    
    fn draw_inline(&mut self, cx: &mut Cx, live_registry: &LiveRegistry, live_ptr: LivePtr) {
        let node = live_registry.ptr_to_node(live_ptr);
        // alright so
        let color = if let LiveValue::Color(c) = &node.value {
            Vec4::from_u32(*c)
        }
        else {
            Vec4::default()
        };
        self.color_picker.size = 100.0;
        self.color_picker.draw(cx, color, 1.0);
    }
}

#[derive(Live, LiveHook)]
#[live_register(register_factory)]
pub struct InlineColorPicker {
    color_picker: ColorPicker
}