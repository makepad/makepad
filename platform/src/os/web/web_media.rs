
use {
    std::sync::{Arc, Mutex},
    self::super::{
        web_audio::WebAudioAccess,
        web::CxOs,
    },
    crate::{
        event::*,
        thread::Signal,
        cx::Cx,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};


impl Cx {
    pub (crate) fn handle_media_signals(&mut self) {
        self.os.handle_web_midi_signals();
        
        if self.os.media.web_audio_change.check_and_clear() {
            // alright so. if we 'failed' opening a device here
            // what do we do. we could flag our device as 'failed' on the desc
            let mut descs = self.os.web_audio().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent {
                descs
            }));
        }
    }
}

#[derive(Default)]
pub struct CxWebMedia {
    pub (crate) web_audio: Option<Arc<Mutex<WebAudioAccess >> >,
    pub (crate) web_audio_change: Signal,
}

impl CxOs {
    pub(crate) fn web_audio(&mut self) -> Arc<Mutex<WebAudioAccess >> {
        if self.media.web_audio.is_none() {
            self.media.web_audio = Some(WebAudioAccess::new(self, self.media.web_audio_change.clone()));
        }
        self.media.web_audio.as_ref().unwrap().clone()
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
    
    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.web_audio().lock().unwrap().use_audio_inputs(&mut self.os, devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.web_audio().lock().unwrap().use_audio_outputs(&mut self.os, devices);
    }
    
    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        *self.os.web_audio().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(f);
    }
    
    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        *self.os.web_audio().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(f);
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
