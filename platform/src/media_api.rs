use crate::{
    audio::{AudioDeviceId, AudioInfo, AudioBuffer},
    video::*,
    midi::*,
};

pub trait CxMediaApi {
    fn midi_input(&mut self) -> MidiInput;
    fn midi_output(&mut self) -> MidiOutput;
    fn midi_reset(&mut self);

    fn use_midi_inputs(&mut self, ports:&[MidiPortId]);
    fn use_midi_outputs(&mut self, ports:&[MidiPortId]);
    
    fn use_audio_inputs(&mut self, devices:&[AudioDeviceId]);
    fn use_audio_outputs(&mut self, devices:&[AudioDeviceId]);
    
    fn audio_output<F>(&mut self, index:usize, f: F) where F: FnMut(AudioInfo, &mut AudioBuffer) + Send  + 'static;
    fn audio_input<F>(&mut self, index:usize, f: F) where F: FnMut(AudioInfo, AudioBuffer)->AudioBuffer + Send  + 'static;

    fn video_input<F>(&mut self, index:usize, f: F) where F: FnMut(VideoFrame) + Send  + 'static;
    fn use_video_input(&mut self, devices:&[(VideoInputId, VideoFormatId)]);
} 
