use crate::audio::{AudioTime, AudioOutputBuffer};
use makepad_platform::*;
use crate::midi::*;

pub trait CxMediaApi {
    fn on_midi_1_input_data(&mut self, event:&Event)->Vec<Midi1InputData>;
    fn on_midi_input_list(&mut self, event:&Event)->Vec<MidiInputInfo>;
    fn start_midi_input(&mut self);
    fn start_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static;
}
