use {
    makepad_render::*,
    std::any::TypeId,
};

pub enum InlineWidgetAction {
    None
}

pub trait InlineWidget: LiveApply {
    fn handle_inline_event(&mut self, cx: &mut Cx, event: &mut Event) -> InlineWidgetAction;
    fn draw_inline(&mut self, cx: &mut Cx);
}

pub enum CanEdit {
    No,
    Yes(f32),
    Sortof(f32)
}

pub trait InlineWidgetFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn InlineWidget>;
    fn can_edit_value(&self, live_registry: &LiveRegistry, node: &LiveNode) -> CanEdit;
}

live_register!{
    InlineWidgetRegistry: {{InlineWidgetRegistry}} {}
}

// this generates a component registry 
#[derive(LiveHook, LiveRegistry)]
#[generate_registry(CxInlineWidgetRegistry, InlineWidget, InlineWidgetFactory)]
pub struct InlineWidgetRegistry();

pub struct MatchedWidget {
    pub height: f32,
    pub live_type: LiveType
}

impl CxInlineWidgetRegistry {
    pub fn match_inline_widget(&self, live_registry: &LiveRegistry, node: &LiveNode) -> Option<MatchedWidget> {
        let mut secondary = None;
        for (live_type, item) in &self.items {
            match item.factory.can_edit_value(live_registry, node) {
                CanEdit::Yes(height) => {return Some(MatchedWidget {height, live_type: *live_type})},
                CanEdit::Sortof(height) => {secondary = Some(MatchedWidget {height, live_type: *live_type})}
                _ => ()
            }
        }
        secondary
    }
}