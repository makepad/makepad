use {
    std::collections::BTreeMap,
    crate::{
        makepad_platform::*,
        makepad_platform::audio::*,
        audio::*
        //audio_engine::AudioEngine
    }
};


// Audio component API



pub enum AudioComponentAction {}
pub trait AudioComponent: LiveApply {
    fn type_id(&self) -> LiveType where Self: 'static {LiveType::of::<Self>()}
    fn handle_event_with_fn(&mut self, _cx: &mut Cx, event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction));
    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>;
}

pub trait AudioGraphNode {
    fn handle_midi_1_data(&mut self, data: Midi1Data);
    fn render_to_audio_buffer(&mut self, time: AudioTime, outputs: &mut [&mut AudioBuffer], inputs: &[&AudioBuffer]);
}

//pub type AudioGraphNodeRef = Option<Box<dyn AudioGraphNode + Send >>;


//generate_ref_cast_api!(AudioComponent);



// Audio component registry


#[derive(Default, LiveComponentRegistry)]
pub struct AudioComponentRegistry {
    pub map: BTreeMap<LiveType, (LiveComponentInfo, Box<dyn AudioComponentFactory>)>
}

pub trait AudioComponentFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn AudioComponent>;
}


// Live bindings for AudioComponentRef


pub struct AudioComponentRef(Option<Box<dyn AudioComponent >>);

impl AudioComponentRef {
    pub fn _as_ref(&mut self) -> Option<&Box<dyn AudioComponent >> {
        self.0.as_ref()
    }
    pub fn as_mut(&mut self) -> Option<&mut Box<dyn AudioComponent >> {
        self.0.as_mut()
    }
}

impl LiveHook for AudioComponentRef {}
impl LiveApply for AudioComponentRef {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let LiveValue::Class {live_type, ..} = nodes[index].value {
            if let Some(component) = &mut self.0 {
                if component.type_id() != live_type {
                    self.0 = None; // type changed, drop old component
                }
                else {
                    component.apply(cx, apply_from, index, nodes);
                    return nodes.skip_node(index);
                }
            }
            if let Some(mut component) = cx.live_registry.clone().borrow()
                .components.get::<AudioComponentRegistry>().new(cx, live_type) {
                component.apply(cx, apply_from, index, nodes);
                self.0 = Some(component);
            }
        }
        else if let Some(component) = &mut self.0 {
            component.apply(cx, apply_from, index, nodes);
        }
        nodes.skip_node(index)
    }
}

impl LiveNew for AudioComponentRef {
    fn new(_cx: &mut Cx) -> Self {
        Self (None)
    }
    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Self (None);
        ret.apply(cx, apply_from, index, nodes);
        ret
    }
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<dyn AudioComponent>(),
            fields: Vec::new(),
            type_name: LiveId(0)
        }
    }
}

#[macro_export]
macro_rules!audio_component_factory {
    ( $ ty: ident) => {
        | cx: &mut Cx | {
            struct Factory();
            impl AudioComponentFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn AudioComponent> {
                    Box::new( $ ty::new(cx))
                }
            }
            register_component_factory!(cx, AudioComponentRegistry, $ ty, Factory);
        }
    }
}
