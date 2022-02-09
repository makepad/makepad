use {
    crate::{
        audio::*,
        makepad_platform::{*,audio::*, midi::*}
    },
};

live_register!{
    Instrument: {{Instrument}} {
    }
}

//enum ToUI {}
enum FromUI {}

#[derive(Live, LiveHook)]
#[live_register(audio_component_factory!(Instrument))]
struct Instrument {
    #[rust] from_ui: FromUISender<FromUI>,
}

#[derive(Default)]
struct Node {
}

impl AudioGraphNode for Node{
    fn handle_midi_1_data(&mut self, _data:Midi1Data){
    }
    
    fn render_to_audio_buffer(&mut self, _buffer: &mut AudioBuffer){
    }
}


impl AudioComponent for Instrument {
    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        Box::new(Node::default())
    }
    
    fn handle_event_with_fn(&mut self, _cx: &mut Cx, _event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
    }
}
