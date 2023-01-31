
use {
    crate::{
        cx::Cx,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};


impl Cx{
    pub (crate) fn handle_media_signals(&mut self) {
        self.os.handle_web_midi_signals();
    }
}


impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        self.os.web_midi_access.create_midi_input()
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        self.os.web_midi_access.create_midi_output()
    }
    
    fn midi_reset(&mut self) {
    }
    
    fn use_midi_inputs(&mut self, _ports: &[MidiPortId]) {
    }
    
    fn use_midi_outputs(&mut self, _ports: &[MidiPortId]) {
    }
    
    fn use_audio_inputs(&mut self, _devices: &[AudioDeviceId]) {
    }
    
    fn use_audio_outputs(&mut self, _devices: &[AudioDeviceId]) {
    }
    
    fn audio_output_box(&mut self, _index: usize, _f: AudioOutputFn) {
    }
    
    fn audio_input_box(&mut self, _index: usize, _f: AudioInputFn) {
    }
    
    fn video_input_box(&mut self, _index: usize, _f: VideoInputFn) {
    }
    
    fn use_video_input(&mut self, _inputs: &[(VideoInputId, VideoFormatId)]) {
    }
    /*
    fn send_midi_data(&mut self, _data:MidiData){
    }
    
    fn handle_midi_inputs(&mut self, event: &Event) -> Vec<MidiInputInfo> {
        if let Event::ToWasmMsg(event) = event {
            match event.id{
                live_id!(ToWasmMidiInputList)=>{
                    let tw = ToWasmMidiInputList::read_to_wasm(&mut event.as_ref());
                    let mut ret = Vec::new();
                    for input in tw.inputs{
                        ret.push(input.into())
                    }
                    return ret
                },
                _=>()
            }
        }
        Vec::new()
    }
    
    fn handle_midi_received(&mut self, event: &Event) -> Vec<MidiInputData> {
        if let Event::ToWasmMsg(event) = event {
            match event.id{
                live_id!(ToWasmMidiInputData)=>{
                    let tw = ToWasmMidiInputData::read_to_wasm(&mut event.as_ref());
                    return vec![tw.into()]
                },
                _=>()
            }
        }
        Vec::new()
    }
    
    fn start_midi_input(&mut self) {
        self.os.from_wasm(FromWasmStartMidiInput {
        });
    }
    
    fn start_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut AudioBuffer) + Send + 'static {
        let closure_ptr = Box::into_raw(Box::new(WebAudioOutputClosure {
            callback: Box::new(f),
            output_buffer: AudioBuffer::default()
        }));
        
        self.os.from_wasm(FromWasmSpawnAudioOutput {closure_ptr: closure_ptr as u32});
    }*/
}
