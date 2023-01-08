use crate::{
    audio::{AudioDevice, AudioTime, AudioBuffer},
    midi::*,
    event::Event,
};

pub trait CxMediaApi {
    // midi
    fn handle_midi_port_list(&mut self, event:&Event)->Vec<MidiPortId>;
    fn midi_port_desc(&self, port: MidiPortId) -> Option<MidiPortDesc>;
    
    fn midi_output(&mut self) -> MidiOutput;  
    fn midi_input(&mut self) -> MidiInput;
    
    // audio in/out
    fn handle_audio_device_list(&mut self, event:&Event)->Vec<AudioDevice>;
    fn request_audio_device_list(&mut self);
    fn start_audio_output<F>(&mut self, device:Option<&AudioDevice>, f: F) where F: FnMut(AudioTime, &mut AudioBuffer) + Send + 'static;
    fn start_audio_input<F>(&mut self, device:Option<&AudioDevice>, f: F) where F: FnMut(AudioTime, AudioBuffer)->AudioBuffer + Send + 'static;
    
    // video in
} 
