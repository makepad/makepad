use {
    makepad_render::*,
    std::collections::{
        HashMap,
        hash_map::Entry
    },
    std::any::TypeId,
};


pub trait LiveEditWidgetFactory {
    fn new_from_factory(&self, cx: &mut Cx) -> Box<dyn LiveEditWidget>;
    fn matches_value(&self, live_registry: &LiveRegistry, node: &LiveNode) -> bool;
}

pub enum LiveEditWidgetAction {
    None
}

pub trait LiveEditWidget: LiveApply {
    fn handle_event_dyn(&mut self, cx: &mut Cx, event: &mut Event) -> LiveEditWidgetAction;
    fn draw_dyn(&mut self, cx: &mut Cx);
    fn apply_draw(&mut self, cx: &mut Cx, nodes: &[LiveNode]) {
        self.apply_over(cx, nodes);
        self.draw_dyn(cx);
    }
}


#[derive(LiveHook)]

pub struct LiveEditWidgetRegistry();
impl LiveNew for LiveEditWidgetRegistry {
    fn new(_cx: &mut Cx) -> Self {return Self ()}
    fn live_register(_cx: &mut Cx) {}
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo{
            live_type:LiveType::of::<Self>(),
            type_name:LiveId::from_str("LiveEditWidgetRegistry").unwrap(),
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            fields: Vec::new()
        }
    }
}

impl LiveApply for LiveEditWidgetRegistry {
    fn apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        nodes.skip_node(index)
    }
}

// so this means, this registry needs to return its child type list
struct Registry {
    factories: HashMap<TypeId, Box<dyn LiveEditWidgetFactory >>
}

pub struct MatchedLiveEditWidget{
    pub height: f32,
    pub live_type: LiveType
}

pub trait CxRegistriesExt{
    fn register_live_edit_widget(&self, live_type: LiveType, component: Box<dyn LiveEditWidgetFactory>);
    fn new_live_edit_widget(cx:&mut Cx, live_type: LiveType) -> Option<Box<dyn LiveEditWidget >>;
    fn match_live_edit_widget(&self, live_registry: &LiveRegistry, node: &LiveNode) -> Option<MatchedLiveEditWidget>;
}

impl CxRegistriesExt for CxRegistries {
    fn register_live_edit_widget(&self, live_type: LiveType, component: Box<dyn LiveEditWidgetFactory>) {
        let registries_cp = self.0.clone();
        let mut registries = registries_cp.borrow_mut();
        let registry = match registries.entry(TypeId::of::<Registry>()) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(Box::new(Registry {factories: HashMap::new()}))
        };
        registry
            .downcast_mut::<Registry>().unwrap()
            .factories.insert(live_type, component);
    }
    
    fn new_live_edit_widget(cx:&mut Cx, live_type: LiveType) -> Option<Box<dyn LiveEditWidget >> {
        let registries_cp = cx.registries.clone();
        let registries = registries_cp.0.borrow();
        registries
            .get(&TypeId::of::<Registry>()).unwrap()
            .downcast_ref::<Registry>().unwrap()
            .factories.get(&live_type)
            .and_then(|cnew| Some(cnew.new_from_factory(cx)) )
    }
    
    fn match_live_edit_widget(&self, _live_registry: &LiveRegistry, _node: &LiveNode) -> Option<MatchedLiveEditWidget>{
        // ok now what. we have to iterate our widget list matching on the factory.
        // if true return the livetype
        
        None
    }

}

