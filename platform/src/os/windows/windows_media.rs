
use {
    std::sync::{Arc,Mutex},
    crate::{
        cx::Cx,
        audio::*,
        midi::*,
        video::*,
        thread::Signal,
        event::Event,
        media_api::CxMediaApi,
        os::windows::winrt_midi::*,
        os::windows::wasapi::*,
        os::windows::media_foundation::*,
    }
};

#[derive(Default)]
pub struct CxWindowsMedia{
    pub (crate) winrt_midi: Option<Arc<Mutex<WinRTMidiAccess >> >,
    pub (crate) wasapi: Option<Arc<Mutex<WasapiAccess >> >,
    pub (crate) media_foundation: Option<Arc<Mutex<MediaFoundationAccess >> >,
    pub (crate) wasapi_change: Signal,
    pub (crate) media_foundation_change: Signal,
    pub (crate) winrt_midi_change: Signal,
}

impl Cx {
    pub (crate) fn handle_media_signals(&mut self) {
        if self.os.media.winrt_midi_change.check_and_clear(){
            let descs = self.os.media.winrt_midi().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::MidiPorts(MidiPortsEvent {
                descs,
            }));
        }
        if self.os.media.wasapi_change.check_and_clear(){
            let descs = self.os.media.wasapi().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent{
                descs
            }));
        }
        if self.os.media.media_foundation_change.check_and_clear(){
            let descs = self.os.media.media_foundation().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::VideoInputs(VideoInputsEvent{
                descs
            }));
        }
    }
}

impl CxWindowsMedia {
    
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
        self.os.media.winrt_midi().lock().unwrap().create_midi_input()
    }
    
    fn midi_output(&mut self)->MidiOutput{
        MidiOutput(Some(OsMidiOutput(self.os.media.winrt_midi())))
    }

    fn midi_reset(&mut self){
        self.os.media.winrt_midi().lock().unwrap().midi_reset();
    }

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os.media.winrt_midi().lock().unwrap().use_midi_inputs(ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os.media.winrt_midi().lock().unwrap().use_midi_outputs(ports);
    }

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.wasapi().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.wasapi().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output_box(&mut self, index:usize, f: AudioOutputFn) {
        *self.os.media.wasapi().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(f);
    }
    
    fn audio_input_box(&mut self, index:usize, f: AudioInputFn) {
        *self.os.media.wasapi().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(f);
    }
    
    fn video_input_box(&mut self, index:usize, f: VideoInputFn){
        *self.os.media.media_foundation().lock().unwrap().video_input_cb[index].lock().unwrap() = Some(f);
    }

    fn use_video_input(&mut self, inputs:&[(VideoInputId, VideoFormatId)]){
        self.os.media.media_foundation().lock().unwrap().use_video_input(inputs);
    }

}



