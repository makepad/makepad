use crate::{
    audio::{AudioTime, AudioBuffer},
    midi::*,
    event::Event
};

pub trait CxMediaApi {
    fn send_midi_data(&mut self, data:MidiData);
    fn handle_midi_received(&mut self, event:&Event)->Vec<MidiInputData>;
    fn handle_midi_inputs(&mut self, event:&Event)->Vec<MidiInputInfo>;
    fn start_midi_input(&mut self);
    fn start_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut AudioBuffer) + Send + 'static;
    fn start_audio_input<F>(&mut self, f: F) where F: FnMut(AudioTime, AudioBuffer)->AudioBuffer + Send + 'static;
}
