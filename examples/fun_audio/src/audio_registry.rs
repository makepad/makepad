use {
    std::any::TypeId,
    crate::{
        makepad_platform::*,
        audio_graph::*
        //audio_engine::AudioEngine
    }
};

live_register!{
    AudioComponentRegistry: {{AudioComponentRegistry}} {}
}

#[derive(LiveHook, LiveRegistry)]
#[generate_registry(CxAudioComponentRegistry, AudioComponent, AudioComponentFactory)]
pub struct AudioComponentRegistry();

pub trait AudioComponentFactory {
    fn new(&self, cx: &mut Cx) -> Box<dyn AudioComponent>;
}

pub struct AudioComponentOption(Option<Box<dyn AudioComponent>>); 
impl AudioComponentOption{
    pub fn component(&mut self)->&mut Option<Box<dyn AudioComponent>>{
        &mut self.0
    }
}

impl LiveHook for AudioComponentOption {}
impl LiveApply for AudioComponentOption {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        if let LiveValue::Class{live_type,..} = nodes[index].value{
            if let Some(component) = &mut self.0{
                if component.type_id() != live_type{
                    self.0 = None;
                }
                else{
                    component.apply(cx, apply_from, index, nodes);
                    return nodes.skip_node(index);
                }
            }
            if let Some(mut component) = cx.registries.clone().get::<CxAudioComponentRegistry>().new(cx, live_type){
                component.apply(cx, apply_from, index, nodes);
                self.0 = Some(component);
            }
        }
        else{
            if let Some(component) = &mut self.0{
                component.apply(cx, apply_from, index, nodes);
            }
        }
        nodes.skip_node(index)
    }
}

impl LiveNew for AudioComponentOption{
    fn new(_cx: &mut Cx) -> Self {
        Self(None)
    }
    fn new_apply(cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Self {
        let mut ret = Self(None);
        ret.apply(cx, apply_from, index, nodes);
        ret
    }
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<dyn AudioComponent>(),
            fields: Vec::new(),
            type_name: LiveId(0),
        }
    }
}

generate_ref_cast_api!(AudioComponent);

#[macro_export]
macro_rules!register_as_audio_component {
    ( $cx:expr, $ ty: ident) => {
        {
            struct Factory();
            impl AudioComponentFactory for Factory {
                fn new(&self, cx: &mut Cx) -> Box<dyn AudioComponent> {
                    Box::new( $ ty::new(cx))
                }
            }
            $cx.registries.clone().get_or_create::<CxAudioComponentRegistry>()
                .register($ ty::live_type_info($cx), Box::new(Factory()), LiveId::from_str(stringify!($ty)).unwrap());
        }
    }
}
