use {
    makepad_render::*,
    std::collections::{
        BTreeMap,
    },
    std::any::TypeId,
};


pub trait LiveWidgetFactory {
    fn new_live_widget(&self, cx: &mut Cx) -> Box<dyn LiveWidget>;
    
    fn new_from_ptr(&self, cx: &mut Cx, live_ptr: LivePtr) -> Box<dyn LiveWidget> {
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        let doc = live_registry.ptr_to_doc(live_ptr);
        
        let mut ret = self.new_live_widget(cx);
        let apply_from = ApplyFrom::NewFromDoc {file_id: live_ptr.file_id};
        let next_index = ret.apply(cx, apply_from, live_ptr.index as usize, &doc.nodes);
        if next_index <= live_ptr.index as usize + 2 {
            cx.apply_error_empty_object(live_error_origin!(), apply_from, live_ptr.index as usize, &doc.nodes);
        }
        return ret
    }

    fn can_edit_value(&self, live_registry: &LiveRegistry, node: &LiveNode) -> CanEdit;
}

pub enum LiveWidgetAction {
    None
}

pub trait LiveWidget: LiveApply {
    fn handle_widget_event(&mut self, cx: &mut Cx, event: &mut Event) -> LiveWidgetAction;
    fn draw_widget(&mut self, cx: &mut Cx);
}

// this is our hook into the DSL structure. for our pluggable components
live_register!{
    LiveWidgetRegistry: {{LiveWidgetRegistry}} {
    }
}

#[derive(LiveHook)]
pub struct LiveWidgetRegistry();
impl LiveNew for LiveWidgetRegistry {
    fn new(_cx: &mut Cx) -> Self {
        return Self ()
    }
    fn live_register(_cx: &mut Cx) {}
    fn live_type_info(cx: &mut Cx) -> LiveTypeInfo {
        // alright so. lets fetch our registry
        let registry = cx.registries.get_or_create::<CxLiveWidgetRegistry>();
        let mut fields = Vec::new();
        for (_, item) in &registry.items {
            fields.push(LiveTypeField {
                id: item.id,
                live_type_info: item.live_type_info.clone(),
                live_field_kind: LiveFieldKind::Live
            });
        }
        LiveTypeInfo {
            live_type: LiveType::of::<Self>(),
            type_name: LiveId::from_str("LiveWidgetRegistry").unwrap(),
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            fields
        }
    }
}

impl LiveApply for LiveWidgetRegistry {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let Some(file_id) = apply_from.file_id() {
            let mut registry = cx.registries.get_or_create::<CxLiveWidgetRegistry>();
            for (_, item) in &mut registry.items {
                let index = nodes.child_by_name(index, item.id).unwrap();
                item.live_ptr = Some(LivePtr {file_id, index: index as u32})
            }
        }
        nodes.skip_node(index)
    }
}

pub enum CanEdit {
    No,
    Yes(f32),
    Sortof(f32)
}

pub struct RegItem {
    live_ptr: Option<LivePtr>,
    factory: Box<dyn LiveWidgetFactory >,
    id: LiveId,
    live_type_info: LiveTypeInfo
}

pub struct CxLiveWidgetRegistry {
    items: BTreeMap<TypeId, RegItem>
}

pub struct MatchedWidget {
    pub height: f32,
    pub live_type: LiveType
}

impl CxRegistryNew for CxLiveWidgetRegistry{
    fn new() -> Self {
        Self {
            items: BTreeMap::new()
        }
    }
}

impl CxLiveWidgetRegistry {
    pub fn register_live_widget(&mut self, live_type_info: LiveTypeInfo, factory: Box<dyn LiveWidgetFactory>, id: LiveId) {
        self.items.insert(live_type_info.live_type, RegItem {
            live_ptr: None,
            factory,
            live_type_info,
            id,
        });
    }
    
    pub fn new_live_widget(&self, cx: &mut Cx, live_type: LiveType) -> Option<Box<dyn LiveWidget >> {
        self.items.get(&live_type)
            .and_then( | cnew | Some(cnew.factory.new_from_ptr(cx, cnew.live_ptr.unwrap())))
    }
    
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