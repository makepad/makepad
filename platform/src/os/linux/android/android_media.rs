
use {
    //std::sync::{Arc, Mutex},
    crate::{
        cx::Cx,
        //event::Event,
        //thread::Signal,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};

impl Cx {
    pub (crate) fn handle_media_signals(&mut self) {
        
    }
}

pub struct OsMidiOutput {}
impl OsMidiOutput{
    pub fn send(&self, _port_id: Option<MidiPortId>, _d: MidiData) {
    }
}

#[derive(Default)]
pub struct CxAndroidMedia {
}

impl CxAndroidMedia {
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        MidiInput(None)
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(None)
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
}



