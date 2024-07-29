
use {
    //std::sync::{Arc, Mutex},
    crate::{
        cx::Cx,
        //event::Event,
        //thread::SignalToUI,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};

#[derive(Clone)]
pub struct OsMidiOutput {
}

impl OsMidiOutput {
    pub fn send(&self, _port_id: Option<MidiPortId>, _data: MidiData) {
    }
}

pub struct OsMidiInput {
}

impl OsMidiInput {
    pub fn receive(&mut self) -> Option<(MidiPortId, MidiData)> {
        None
    }
}


#[derive(Default)]
pub struct CxOpenHarmonyMedia {
}

impl Cx {
    pub (crate) fn handle_media_signals(&mut self/*, to_java: &AndroidToJava*/) {
    }
    
    pub fn reinitialise_media(&mut self){
    }
}

impl CxOpenHarmonyMedia {
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        MidiInput(Some(OsMidiInput {}))
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput {}))
    }
    
    fn midi_reset(&mut self) {}
    
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



