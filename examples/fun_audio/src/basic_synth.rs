#![allow(unused_variables)]
use {
    crate::{
        register_as_audio_component,
        audio_registry::*,
        audio_graph::*,
        makepad_platform::*,
        makepad_platform::platform::apple::{
            audio_unit::*,
        },
    },
};

live_register!{
    BasicSynth: {{BasicSynth}} {
        prop:1.0
    }
}

//enum ToUI {}
enum FromUI {}

#[derive(Live, LiveHook)]
#[live_register( | cx: &mut Cx | {register_as_audio_component!(cx, BasicSynth)})]
struct BasicSynth {
    prop:f64,
    #[rust(FromUISender::new())] from_ui: FromUISender<FromUI>,
//    #[rust(ToUIReceiver::new(cx))] to_ui: ToUIReceiver<ToUI>,
}

#[derive(Default)]
struct Node {
    sample_time: u64,
    key_down_time: u64,
    note: u64,
}

impl AudioGraphNode for Node{
    fn handle_midi_1_data(&mut self, data:Midi1Data){
        match data.decode(){
            Midi1Event::Note(note) if note.is_on =>{
                self.key_down_time = self.sample_time;
                self.note = note.note_number as u64;
            }
            _=>()
        }
    }
    
    fn render_to_audio_buffer(&mut self, buffer: &mut AudioBuffer){
        let freq = 440.0 * 2.0f64.powf( (self.note as f64 - 69.0)/12.0);
        for i in 0..buffer.left.len(){
            let note_time = ((self.sample_time - self.key_down_time) as f64 / 44100.0).max(0.0).min(1.0);
            let ramp = (0.37*3.1415-note_time).powf(8.0).sin().max(0.0).min(1.0);
            let ft = self.sample_time as f64 / 44100.0;
            let sample = (ft * freq * 3.14).sin() * ramp;

            buffer.left[i] = sample as f32;
            buffer.right[i] = sample as f32;

            self.sample_time += 1;
        }
    }
}


impl AudioComponent for BasicSynth {
    fn type_id(&self) -> LiveType {LiveType::of::<Self>()}

    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        Box::new(Node::default())
    }
    
    fn handle_event_with_fn(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
    }
}
