use crate::audio::{AudioTime, AudioOutputBuffer};
use crate::makepad_platform::*;
use crate::midi::*;

pub trait CxMediaApi {
    fn send_midi_data(&mut self, data:MidiData);
    fn handle_midi_received(&mut self, event:&Event)->Vec<MidiInputData>;
    fn handle_midi_inputs(&mut self, event:&Event)->Vec<MidiInputInfo>;
    fn start_midi_input(&mut self);
    fn start_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static;
}
