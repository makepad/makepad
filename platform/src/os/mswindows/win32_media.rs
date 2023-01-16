
use {
    std::sync::{Arc,Mutex},
    crate::{
        cx::Cx,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
        os::mswindows::winrt_midi::*,
        os::mswindows::wasapi::*,
        os::mswindows::media_foundation::*,
        os::mswindows::CxOs,
    }
};

impl CxOs {
    
    pub fn winrt_midi(&mut self) -> Arc<Mutex<WinRTMidiAccess >> {
        if self.winrt_midi.is_none() {
            self.winrt_midi = Some(WinRTMidiAccess::new(self.winrt_midi_change.clone()));
        }
        self.winrt_midi.as_ref().unwrap().clone()
    }
    
    pub fn wasapi(&mut self) -> Arc<Mutex<WasapiAccess >> {
        if self.wasapi.is_none() {
            self.wasapi = Some(WasapiAccess::new(self.wasapi_change.clone()));
        }
        self.wasapi.as_ref().unwrap().clone()
    }
    
    pub fn media_foundation(&mut self) -> Arc<Mutex<MediaFoundationAccess >> {
        if self.media_foundation.is_none() {
            self.media_foundation = Some(MediaFoundationAccess::new(self.media_foundation_change.clone()));
        }
        self.media_foundation.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        self.os.winrt_midi().lock().unwrap().create_midi_input()
    }
    
    fn midi_output(&mut self)->MidiOutput{
        MidiOutput(Some(OsMidiOutput(self.os.winrt_midi())))
    }

    fn midi_reset(&mut self){
        self.os.winrt_midi().lock().unwrap().midi_reset();
    }

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os.winrt_midi().lock().unwrap().use_midi_inputs(ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os.winrt_midi().lock().unwrap().use_midi_outputs(ports);
    }

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.wasapi().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.wasapi().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output<F>(&mut self, index:usize, f: F) where F: FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static {
        *self.os.wasapi().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(Box::new(f));
    }
    
    fn audio_input<F>(&mut self, index:usize, f: F)
    where F: FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static {
        *self.os.wasapi().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(Box::new(f));
    }
    
    fn video_input<F>(&mut self, index:usize, f: F)
    where F: FnMut(VideoFrame) + Send + 'static {
        *self.os.media_foundation().lock().unwrap().video_input_cb[index].lock().unwrap() = Some(Box::new(f));
    }

    fn use_video_input(&mut self, inputs:&[(VideoInputId, VideoFormatId)]){
        self.os.media_foundation().lock().unwrap().use_video_input(inputs);
    }

}



