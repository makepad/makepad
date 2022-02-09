use {
    crate::{
        audio_component_factory,
        audio::*,
        makepad_platform::{*,audio::*}
    },
};

live_register!{
    Mixer: {{Mixer}} {
    }
}

//enum ToUI {}
enum FromUI {}

#[derive(Live, LiveHook)]
#[live_register(audio_component_factory!(Mixer))]
struct Mixer {
    #[rust] from_ui: FromUISender<FromUI>,
}

#[derive(Default)]
struct Node {
}

// ok so how do we spawn this shit up.

impl AudioGraphNode for Node{
    
    fn render_to_audio_buffer(&mut self, _buffer: &mut AudioBuffer){
    }
}


impl AudioComponent for Mixer {
    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        Box::new(Node::default())
    }
    
    fn handle_event_with_fn(&mut self, _cx: &mut Cx, _event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
    }
}

