use {
    crate::{
        makepad_platform::*,
        makepad_live_tokenizer::{
            position::Position,
            text::Text,
            size::Size
        },
        design_editor::{
            inline_cache::InlineEditBind
        },
    },
    std::any::TypeId,
};

live_register!{
    InlineWidgetRegistry: {{InlineWidgetRegistry}} {}
}

#[derive(LiveHook, LiveRegistry)]
#[generate_registry(CxInlineWidgetRegistry, InlineWidget, InlineWidgetFactory)]
pub struct InlineWidgetRegistry();

pub enum InlineWidgetAction {
    None,
    ReplaceText {
        position: Position,
        size: Size,
        text: Text
    }
}

pub trait InlineWidget: LiveApply {
    fn type_id(&self) -> LiveType;
    fn handle_widget_event(&mut self, cx: &mut Cx, event: &mut Event, bind: InlineEditBind) -> InlineWidgetAction;
    fn draw_widget(&mut self, cx: &mut Cx2d, live_registry: &LiveRegistry, bind: InlineEditBind);
}

generate_ref_cast_api!(InlineWidget);
/*
impl dyn InlineWidget {
    pub fn type_id(&self) -> std::any::TypeId {std::any::TypeId::of::<Self>()}
    
    pub fn is<T: InlineWidget + 'static >(&self) -> bool {
        let t = TypeId::of::<T>();
        let concrete = self.type_id();
        t == concrete
    }
    pub fn cast<T: InlineWidget + 'static >(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe {&*(self as *const dyn InlineWidget as *const T)})
        } else {
            None
        }
    }
    pub fn cast_mut<T: InlineWidget + 'static >(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe {&mut *(self as *const dyn InlineWidget as *mut T)})
        } else {
            None
        }
    }
}*/

pub enum CanEdit {
    No,
    Yes(f32),
    Sortof(f32)
}

pub trait InlineWidgetFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn InlineWidget>;
    fn can_edit_value(&self, live_registry: &LiveRegistry, bind: InlineEditBind) -> CanEdit;
}

pub struct MatchedWidget {
    pub height: f32,
    pub live_type: LiveType
}

impl CxInlineWidgetRegistry {
    pub fn match_inline_widget(&self, live_registry: &LiveRegistry, bind: InlineEditBind) -> Option<MatchedWidget> {
        let mut secondary = None;
        for (live_type, item) in &self.items {
            match item.factory.can_edit_value(live_registry, bind) {
                CanEdit::Yes(height) => {return Some(MatchedWidget {height, live_type: *live_type})},
                CanEdit::Sortof(height) => {secondary = Some(MatchedWidget {height, live_type: *live_type})}
                _ => ()
            }
        }
        secondary
    }
}