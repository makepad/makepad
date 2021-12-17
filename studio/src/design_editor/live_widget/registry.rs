use {
    makepad_render::*,
    std::collections::{
        BTreeMap,
    },
    std::any::TypeId,
};

pub enum LiveWidgetAction {
    None
}
pub trait LiveWidget: LiveApply {
    fn handle_widget_event(&mut self, cx: &mut Cx, event: &mut Event) -> LiveWidgetAction;
    fn draw_widget(&mut self, cx: &mut Cx);
}

pub trait LiveWidgetFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn LiveWidget>;
    fn can_edit_value(&self, live_registry: &LiveRegistry, node: &LiveNode) -> CanEdit;
}

live_register!{
    LiveWidgetRegistry: {{LiveWidgetRegistry}} {}
}

#[derive(LiveHook, LiveRegistry)]
#[generate_registry(CxLiveWidgetRegistry, LiveWidget, LiveWidgetFactory)]
pub struct LiveWidgetRegistry();

pub enum CanEdit {
    No,
    Yes(f32),
    Sortof(f32)
}

pub struct MatchedWidget {
    pub height: f32,
    pub live_type: LiveType
}

impl CxLiveWidgetRegistry {
    pub fn match_live_widget(&self, live_registry: &LiveRegistry, node: &LiveNode) -> Option<MatchedWidget> {
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