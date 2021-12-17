use {
    makepad_render::*,
    std::any::TypeId,
};

live_register!{
    InlineWidgetRegistry: {{InlineWidgetRegistry}} {}
}

#[derive(LiveHook, LiveRegistry)]
#[generate_registry(CxInlineWidgetRegistry, InlineWidget, InlineWidgetFactory)]
pub struct InlineWidgetRegistry();

pub enum InlineWidgetAction {
    None
}

pub trait InlineWidget: LiveApply {
    fn handle_inline_event(&mut self, cx: &mut Cx, event: &mut Event, live_ptr:LivePtr) -> InlineWidgetAction;
    fn draw_inline(&mut self, cx: &mut Cx, live_registry:&LiveRegistry, live_ptr:LivePtr);
}

pub enum CanEdit {
    No,
    Yes(f32),
    Sortof(f32)
}

pub trait InlineWidgetFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn InlineWidget>;
    fn can_edit_value(&self, live_registry: &LiveRegistry, live_ptr: LivePtr) -> CanEdit;
}

pub struct MatchedWidget {
    pub height: f32,
    pub live_type: LiveType
}

impl CxInlineWidgetRegistry {
    pub fn match_inline_widget(&self, live_registry: &LiveRegistry, live_ptr:LivePtr) -> Option<MatchedWidget> {
        let mut secondary = None;
        for (live_type, item) in &self.items {
            match item.factory.can_edit_value(live_registry, live_ptr) {
                CanEdit::Yes(height) => {return Some(MatchedWidget {height, live_type: *live_type})},
                CanEdit::Sortof(height) => {secondary = Some(MatchedWidget {height, live_type: *live_type})}
                _ => ()
            }
        }
        secondary
    }
}