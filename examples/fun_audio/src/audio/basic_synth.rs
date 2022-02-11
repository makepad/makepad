use {
    crate::{
        audio::*,
        makepad_platform::{*,audio::*, midi::*}
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
#[live_register(audio_component_factory!(BasicSynth))]
struct BasicSynth {
    prop:f64,
    #[rust] from_ui: FromUISender<FromUI>,
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
    
    fn render_to_audio_buffer(&mut self, _time: AudioTime, outputs: &mut [&mut AudioBuffer], _inputs: &[&AudioBuffer]){
        let freq = 440.0 * 2.0f64.powf( (self.note as f64 - 69.0)/12.0);
        // only do one output
        let output = &mut outputs[0];
        for i in 0..output.frame_count(){
            let note_time = ((self.sample_time - self.key_down_time) as f64 / 44100.0).max(0.0).min(1.0);
            let ramp = (0.37*3.1415-note_time).powf(8.0).sin().max(0.0).min(1.0);
            let ft = self.sample_time as f64 / 44100.0;
            let sample = (ft * freq * 3.14).sin() * ramp;
            
            for j in 0..output.channel_count(){
                let channel = output.channel_mut(j);
                channel[i] = sample as f32;
            }

            self.sample_time += 1;
        }
    }
}


impl AudioComponent for BasicSynth {
    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        Box::new(Node::default())
    }
    
    fn handle_event_with_fn(&mut self, _cx: &mut Cx, _event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
    }
}
