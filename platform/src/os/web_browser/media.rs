
use {
    crate::{
        makepad_wasm_bridge::*,
        cx::Cx,
        audio::*,
        midi::*,
        event::Event,
        os::web_browser::{
            to_wasm::{ToWasmMidiInputList, ToWasmMidiInputData},
            from_wasm::{FromWasmStartMidiInput, FromWasmSpawnAudioOutput}
        },
        media_api::CxMediaApi,
        os::web_browser::web_audio::*,
    }
};


impl CxMediaApi for Cx {
    
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
    }
}
