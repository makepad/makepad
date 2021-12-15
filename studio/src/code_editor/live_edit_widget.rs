use{
    makepad_render::*,
    std::collections::{
        HashMap,
    },
    std::any::TypeId,
};
 
pub fn add_live_edit_widget_registry(cx: &mut Cx) {
    cx.registries.add_registry(
        TypeId::of::<Registry>(),
        Box::new(Registry {component_new: HashMap::new()})
    )
}

pub trait LiveEditWidgetNew {
    fn new_component(&self, cx: &mut Cx) -> Box<dyn LiveEditWidget>;
    fn matches_value(&self, live_registry:&LiveRegistry, node:&LiveNode) -> bool;
}

pub enum LiveEditWidgetAction{}

pub trait LiveEditWidget: LiveApply {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event) -> LiveEditWidgetAction;
    fn draw_dyn(&mut self, cx: &mut Cx);
    fn apply_draw(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply_over(cx, nodes);
        self.draw_dyn(cx);
    }
}

struct Registry {
    component_new: HashMap<TypeId, Box<dyn LiveEditWidgetNew >>
}

pub trait CxRegistriesExt{
    fn register_live_edit_widget<T: 'static>(&mut self, component: Box<dyn LiveEditWidgetNew>);
    fn create_live_edit_widget(cx:&mut Cx, live_type: LiveType) -> Option<Box<dyn LiveEditWidget >>;
    fn match_live_edit_widget(&self, live_registry:&LiveRegistry, node:&LiveNode) -> Option<(f32, LiveType)>;
}

impl CxRegistriesExt for CxRegistries {
    fn register_live_edit_widget<T: 'static>(&mut self, component: Box<dyn LiveEditWidgetNew>) {
        self.0.borrow_mut()
            .get_mut(&TypeId::of::<Registry>()).unwrap()
            .downcast_mut::<Registry>().unwrap()
            .component_new.insert(TypeId::of::<T>(), component);
    }

    fn match_live_edit_widget(&self, live_registry:&LiveRegistry, node:&LiveNode) -> Option<(f32, LiveType)> {
        None
    }
    
    fn create_live_edit_widget(cx:&mut Cx, live_type: LiveType) -> Option<Box<dyn LiveEditWidget >> {
        let registries_cp = cx.registries.clone();
        let registries = registries_cp.0.borrow();
        registries
            .get(&TypeId::of::<Registry>()).unwrap()
            .downcast_ref::<Registry>().unwrap()
            .component_new.get(&live_type)
            .and_then(|cnew| Some(cnew.new_component(cx)) )
    }
}

