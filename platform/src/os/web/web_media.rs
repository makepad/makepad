
use {
    std::sync::{Arc, Mutex},
    self::super::{
        web_audio::WebAudioAccess,
        web_midi::WebMidiAccess,
        web::CxOs,
    },
    crate::{
        event::*,
        thread::Signal,
        cx::Cx,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};


impl Cx {
    pub (crate) fn handle_media_signals(&mut self) {
        self.os.handle_web_midi_signals();
        
        if self.os.media.web_audio_change.check_and_clear() {
            let descs = self.os.web_audio().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent {
                descs
            }));
        }

        if self.os.media.web_midi_change.check_and_clear() {
            let descs = self.os.web_midi().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::MidiPorts(MidiPortsEvent {
                descs,
            }));
        }
    }
}

#[derive(Default)]
pub struct CxWebMedia {
    pub (crate) web_audio: Option<Arc<Mutex<WebAudioAccess >> >,
    pub (crate) web_audio_change: Signal,
    pub (crate) web_midi: Option<Arc<Mutex<WebMidiAccess >> >,
    pub (crate) web_midi_change: Signal,
}

impl CxOs {
    pub(crate) fn web_audio(&mut self) -> Arc<Mutex<WebAudioAccess >> {
        if self.media.web_audio.is_none() {
            self.media.web_audio = Some(WebAudioAccess::new(self, self.media.web_audio_change.clone()));
        }
        self.media.web_audio.as_ref().unwrap().clone()
    }
    
    pub(crate) fn web_midi(&mut self) -> Arc<Mutex<WebMidiAccess >> {
        if self.media.web_midi.is_none() {
            self.media.web_midi = Some(WebMidiAccess::new(self, self.media.web_midi_change.clone()));
        }
        self.media.web_midi.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        self.os.web_midi().lock().unwrap().create_midi_input()
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        self.os.web_midi().lock().unwrap().create_midi_output()
    }
    
    fn midi_reset(&mut self) {
        self.os.web_midi().lock().unwrap().midi_reset(&mut self.os)
    }
    
    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os.web_midi().lock().unwrap().use_midi_inputs(&mut self.os, ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os.web_midi().lock().unwrap().use_midi_outputs(&mut self.os, ports);
    }
    
    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.web_audio().lock().unwrap().use_audio_inputs(&mut self.os, devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.web_audio().lock().unwrap().use_audio_outputs(&mut self.os, devices);
    }
    
    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        *self.os.web_audio().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(f);
    }
    
    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        *self.os.web_audio().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(f);
    }
    
    fn video_input_box(&mut self, _index: usize, _f: VideoInputFn) {
    }
    
    fn use_video_input(&mut self, _inputs: &[(VideoInputId, VideoFormatId)]) {
    }
}
