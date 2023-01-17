
use {
    std::sync::{mpsc},
    crate::{
        cx::Cx,
        os::CxOS,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};

impl CxOs {
    
}

struct OsMidiOutput();

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        let (_send, recv) = mpsc::channel();
        MidiInput(Some(recv))
    }
    
    fn midi_output(&mut self)->MidiOutput{
        MidiOutput(Some(OsMidiOutput()))
    }

    fn midi_reset(&mut self){
    }

    fn use_midi_inputs(&mut self, _ports: &[MidiPortId]) {
    }
    
    fn use_midi_outputs(&mut self, _ports: &[MidiPortId]) {
    }

    fn use_audio_inputs(&mut self, _devices: &[AudioDeviceId]) {
    }
    
    fn use_audio_outputs(&mut self, _devices: &[AudioDeviceId]) {
    }
    
    fn audio_output<F>(&mut self, _index:usize, _f: F) where F: FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static {
    }
    
    fn audio_input<F>(&mut self, _index:usize, _f: F)
    where F: FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static {
    }
    
    fn video_input<F>(&mut self, _index:usize, _f: F)
    where F: FnMut(VideoFrame) + Send + 'static {
    }

    fn use_video_input(&mut self, _inputs:&[(VideoInputId, VideoFormatId)]){
    }

}



