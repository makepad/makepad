use {
    std::any::TypeId,
    crate::makepad_render::*
};

live_register!{
    FrameComponentRegistry: {{FrameComponentRegistry}} {}
}

#[derive(LiveHook, LiveRegistry)]
#[generate_registry(CxFrameComponentRegistry, FrameComponent, FrameComponentFactory)]
pub struct FrameComponentRegistry();

pub trait FrameComponentFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn FrameComponent>;
}

pub trait FrameComponent: LiveApply {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event) -> Option<Box<dyn FrameComponentAction >>;
    fn draw_component(&mut self, cx: &mut Cx2d);
    fn apply_draw(&mut self, cx: &mut Cx2d, nodes: &[LiveNode]) {
        self.apply_over(cx, nodes);
        self.draw_component(cx);
    }
    fn type_id(&self) -> TypeId;
}

generate_ref_cast_api!(FrameComponent);

#[derive(Clone)]
pub struct FrameActionItem {
    pub id: LiveId,
    pub action: Box<dyn FrameComponentAction>
}

#[derive(Clone, IntoFrameComponentAction)]
pub enum FrameActions {
    None,
    Actions(Vec<FrameActionItem>)
}


impl Default for FrameActions {
    fn default() -> Self {Self::None}
}

pub struct FrameActionsIterator {
    iter: Option<std::vec::IntoIter<FrameActionItem >>
}

impl Iterator for FrameActionsIterator {
    type Item = FrameActionItem;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(iter) = self.iter.as_mut() {
            return iter.next()
        }
        else {
            None
        }
    }
}

// and we'll implement IntoIterator
impl IntoIterator for FrameActions {
    type Item = FrameActionItem;
    type IntoIter = FrameActionsIterator;
    
    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::None => FrameActionsIterator {iter: None},
            Self::Actions(actions) => FrameActionsIterator {iter: Some(actions.into_iter())},
        }
    }
}

pub trait FrameComponentAction: 'static {
    fn type_id(&self) -> TypeId;
    fn box_clone(&self) -> Box<dyn FrameComponentAction>;
}

impl<T: 'static + ? Sized + Clone> FrameComponentAction for T {
    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    
    fn box_clone(&self) -> Box<dyn FrameComponentAction> {
        Box::new((*self).clone())
    }
}

generate_clone_cast_api!(FrameComponentAction);

pub type OptionFrameComponentAction = Option<Box<dyn FrameComponentAction >>;

impl Clone for Box<dyn FrameComponentAction> {
    fn clone(&self) -> Box<dyn FrameComponentAction> {
        self.as_ref().box_clone()
    }
}

#[macro_export]
macro_rules!register_as_frame_component {
    ( $cx:expr, $ ty: ident) => {
        {
            struct Factory();
            impl FrameComponentFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn FrameComponent> {
                    Box::new( $ ty::new(cx))
                }
            }
            $cx.registries.clone().get_or_create::<CxFrameComponentRegistry>()
                .register($ ty::live_type_info($cx), Box::new(Factory()), LiveId::from_str(stringify!($ty)).unwrap());
        }
    }
}
