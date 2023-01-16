
use {
    std::sync::{Arc,Mutex},
    crate::{
        cx::Cx,
        audio::*,
        video::*,
        midi::*,
        media_api::CxMediaApi,
        os::apple::core_midi::*,
        os::apple::CxOs,
        os::apple::audio_unit::AudioUnitAccess,
        os::apple::av_capture::AvCaptureAccess,
    }
};


impl CxOs{
    pub fn audio_unit(&mut self)->Arc<Mutex<AudioUnitAccess>>{
        if self.audio_unit.is_none(){
            self.audio_unit = Some(AudioUnitAccess::new(self.core_audio_change.clone()));
            self.core_audio_change.set();
        }
        self.audio_unit.as_ref().unwrap().clone()
    }
    
    pub fn core_midi(&mut self)->Arc<Mutex<CoreMidiAccess>>{
        if self.core_midi.is_none(){
            self.core_midi = Some(CoreMidiAccess::new(self.core_midi_change.clone()));
            self.core_midi_change.set();
        }
        self.core_midi.as_ref().unwrap().clone()
    }
    
    pub fn av_capture(&mut self)->Arc<Mutex<AvCaptureAccess>>{
        if self.av_capture.is_none(){
            self.av_capture = Some(AvCaptureAccess::new(self.av_capture_change.clone()));
            self.av_capture_change.set();
        }
        self.av_capture.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    fn midi_input(&mut self) -> MidiInput {
        self.os.core_midi().lock().unwrap().create_midi_input()
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput(self.os.core_midi())))
    }

    fn midi_reset(&mut self) {
        self.os.core_midi().lock().unwrap().midi_reset();
    }
    
    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os.core_midi().lock().unwrap().use_midi_inputs(ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os.core_midi().lock().unwrap().use_midi_outputs(ports);
    }
    
    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.audio_unit().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.audio_unit().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output<F>(&mut self, index:usize, f: F) where F: FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static {
        *self.os.audio_unit().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(Box::new(f));
    }
    
    fn audio_input<F>(&mut self, index:usize, f: F)
    where F: FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static {
        *self.os.audio_unit().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(Box::new(f));
    }
    
    fn video_input<F>(&mut self, index:usize, f: F)
    where F: FnMut(VideoFrame) + Send + 'static {
        *self.os.av_capture().lock().unwrap().video_input_cb[index].lock().unwrap() = Some(Box::new(f));
    }

    fn use_video_input(&mut self, inputs:&[(VideoInputId, VideoFormatId)]){
        self.os.av_capture().lock().unwrap().use_video_input(inputs);
    }

}

