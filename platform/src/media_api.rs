use crate::{
    audio::{AudioDeviceId, AudioInfo, AudioBuffer,AudioInputFn,AudioOutputFn},
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
    
    fn audio_output<F>(&mut self, index:usize, f: F) where F: FnMut(AudioInfo, &mut AudioBuffer) + Send  + 'static{
        self.audio_output_box(index, Box::new(f))
    }
    fn audio_input<F>(&mut self, index:usize, f: F) where F: FnMut(AudioInfo, &AudioBuffer) + Send  + 'static{
        self.audio_input_box(index, Box::new(f))
    }
    
    fn audio_output_box(&mut self, index:usize, f: AudioOutputFn);
    fn audio_input_box(&mut self, index:usize, f: AudioInputFn);

    fn video_input<F>(&mut self, index:usize, f: F) where F: FnMut(VideoFrame) + Send  + 'static{
        self.video_input_box(index, Box::new(f))
    }
    
    fn video_input_box(&mut self, index:usize, f: VideoInputFn);
    
    fn use_video_input(&mut self, devices:&[(VideoInputId, VideoFormatId)]);
} 
