use {
    crate::{
        makepad_platform::*,
        makepad_studio_component::color_picker::*,
        makepad_live_compiler::LiveToken,
        makepad_live_tokenizer::{
            position::Position,
            text::Text,
        },
        design_editor::{
            inline_widget::*,
            inline_cache::InlineEditBind
        },
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
        
        fn can_edit_value(&self, live_registry: &LiveRegistry, bind: InlineEditBind) -> CanEdit {
            let node = live_registry.ptr_to_node(bind.live_ptr);
            match &node.value {
                LiveValue::Color(_) => {
                    return CanEdit::Yes(100.0)
                }
                LiveValue::DSL {..} => {
                    let token = live_registry.token_id_to_token(bind.live_token_id);
                    if token.is_color() {
                        return CanEdit::Yes(100.0)
                    }
                }
                _ => ()
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
    
    fn type_id(&self) -> LiveType {LiveType::of::<Self>()}
    
    fn handle_widget_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        bind: InlineEditBind
    ) -> InlineWidgetAction {
        
        match self.color_picker.handle_event(cx, event) {
            ColorPickerAction::Change {rgba} => {
                let live_registry_rc = cx.live_registry.clone();
                let live_registry = live_registry_rc.borrow();
                
                let mut s = String::new();
                s.push_str("#x");
                rgba.append_hex_to_string(&mut s);
                
                // alright we are going to fetch some tokens.
                let token = live_registry.token_id_to_token(bind.live_token_id);
                let start_pos = Position::from(token.span.start);
                let end_pos = Position::from(token.span.end);
                
                return InlineWidgetAction::ReplaceText {
                    position: start_pos,
                    size: end_pos - start_pos,
                    text: Text::from(s)
                }
            }
            _ => ()
        }
        InlineWidgetAction::None
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d, live_registry: &LiveRegistry, bind: InlineEditBind) {
        let node = live_registry.ptr_to_node(bind.live_ptr);
        // alright so
        let color = match &node.value {
            LiveValue::Color(c) => {
                Vec4::from_u32(*c)
            }
            LiveValue::DSL {..} => {
                let token = live_registry.token_id_to_token(bind.live_token_id).token;
                match token {
                    LiveToken::Color(c) => {
                        Vec4::from_u32(c)
                    }
                    _ => Vec4::default()
                }
            }
            _ => Vec4::default()
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