use {
    crate::{
        audio::*,
        makepad_platform::thread::*,
        midi::*,
        audio_graph::*,
        makepad_platform::*
    },
};

live_design!{
    BasicSynth= {{BasicSynth}} {
        prop:1.0
    }
}

//enum ToUI {}
enum FromUI {}

#[derive(Live, LiveHook)]
#[live_design_fn(audio_component!(BasicSynth))]
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

    fn all_notes_off(&mut self){
    }

    fn handle_midi_data(&mut self, data:MidiData){
        match data.decode(){
            MidiEvent::Note(note) if note.is_on =>{
                self.key_down_time = self.sample_time;
                self.note = note.note_number as u64;
            }
            _=>()
        }
    }
    
    fn render_to_audio_buffer(&mut self, _time: AudioTime, outputs: &mut [&mut AudioBuffer], _inputs: &[&AudioBuffer], _display:&mut DisplayAudioGraph){
        let freq = 440.0 * 2.0f64.powf( (self.note as f64 - 69.0)/12.0);
        // only do one output
        let output = &mut outputs[0];
        
        let frame_count = output.frame_count();
        let channel_count = output.channel_count();
        
        for i in 0..frame_count{
            let note_time = ((self.sample_time - self.key_down_time) as f64 / 44100.0).max(0.0).min(1.0);
            let ramp = (0.37*3.1415-note_time).powf(8.0).sin().max(0.0).min(1.0);
            let ft = self.sample_time as f64 / 44100.0;
            let sample = (ft * freq * 3.14).sin() * ramp;
            
            for j in 0..channel_count{
                let channel = output.channel_mut(j);
                channel[i] = sample as f32;
            }

            self.sample_time += 1;
        }
    }
}


impl AudioComponent for BasicSynth {
    fn get_graph_node(&mut self, _cx:&mut Cx) -> Box<dyn AudioGraphNode + Send>{
        self.from_ui.new_channel();
        Box::new(Node::default())
    }
    
    fn handle_event_fn(&mut self, _cx: &mut Cx, _event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)){
    }
    // we dont have inputs
    fn audio_query(&mut self, _query: &AudioQuery, _callback: &mut Option<AudioQueryCb>) -> AudioResult{
        AudioResult::not_found()
    }
}
