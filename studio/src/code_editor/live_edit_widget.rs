use {
    makepad_render::*,
    std::collections::{
        BTreeMap,
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
        // alright so.
        // at this point we need to walk over our factories
        //
        LiveTypeInfo {
            live_type: LiveType::of::<Self>(),
            type_name: LiveId::from_str("LiveEditWidgetRegistry").unwrap(),
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

pub struct LiveEditWidgetPrio(pub u64);

pub struct RegItem {
    pub factory: Box<dyn LiveEditWidgetFactory >,
    pub name: String,
    pub prio: LiveEditWidgetPrio,
}

struct Registry {
    regs: BTreeMap<TypeId, RegItem>
}

pub struct MatchedLiveEditWidget {
    pub height: f32,
    pub live_type: LiveType
}

pub trait CxRegistriesExt {
    fn register_live_edit_widget(&self, live_type: LiveType, factory:Box<dyn LiveEditWidgetFactory>, name:&str, prio:LiveEditWidgetPrio);
    fn new_live_edit_widget(cx: &mut Cx, live_type: LiveType) -> Option<Box<dyn LiveEditWidget >>;
    fn match_live_edit_widget(&self, live_registry: &LiveRegistry, node: &LiveNode) -> Option<MatchedLiveEditWidget>;
}

impl CxRegistriesExt for CxRegistries {
    fn register_live_edit_widget(&self, live_type: LiveType, factory:Box<dyn LiveEditWidgetFactory>, name:&str, prio:LiveEditWidgetPrio){
        let registries = self.clone();
        let mut registry = registries.get_or_create::<Registry,_>(|| Registry {regs: BTreeMap::new()});
        registry.regs.insert(live_type, RegItem{factory, name:name.to_string(), prio});
    }
    
    fn new_live_edit_widget(cx: &mut Cx, live_type: LiveType) -> Option<Box<dyn LiveEditWidget >> {
        let registries = cx.registries.clone();
        let registry = registries.get::<Registry>();
        registry.regs.get(&live_type)
            .and_then( | cnew | Some(cnew.factory.new_from_factory(cx)))
    }
    
    fn match_live_edit_widget(&self, _live_registry: &LiveRegistry, _node: &LiveNode) -> Option<MatchedLiveEditWidget> {
        // can we make this less shit somehow?
        let registries= self.clone();
        let registry = registries.get::<Registry>();
        None
    }
}