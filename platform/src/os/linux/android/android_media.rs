
use {
    std::sync::{Arc, Mutex},
    self::super::{
        aaudio::*,
        android_jni::*,
        amidi::*
    },
    crate::{
        cx::Cx,
        event::Event,
        thread::Signal,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};

#[derive(Default)]
pub struct CxAndroidMedia {
    pub (crate) aaudio_change: Signal,
    pub (crate) aaudio: Option<Arc<Mutex<AAudioAccess >> >,
    pub (crate) amidi_change: Signal,
    pub (crate) amidi: Option<Arc<Mutex<AMidiAccess >> >,
}

impl Cx {
    pub (crate) fn handle_media_signals(&mut self, to_java: &AndroidToJava) {
        if self.os.media.aaudio_change.check_and_clear() {
            let descs = self.os.media.aaudio().lock().unwrap().get_updated_descs(to_java);
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent {
                descs
            }));
        }
        if self.os.media.amidi_change.check_and_clear() {
            if let Some(descs) = self.os.media.amidi().lock().unwrap().get_updated_descs(to_java){
                self.call_event_handler(&Event::MidiPorts(MidiPortsEvent {
                    descs,
                }));
            }
        }
    }
}

impl CxAndroidMedia {
    pub fn aaudio(&mut self) -> Arc<Mutex<AAudioAccess >> {
        if self.aaudio.is_none() {
            self.aaudio = Some(AAudioAccess::new(self.aaudio_change.clone()));
        }
        self.aaudio.as_ref().unwrap().clone()
    }
    pub fn amidi(&mut self) -> Arc<Mutex<AMidiAccess >> {
        if self.amidi.is_none() {
            self.amidi = Some(AMidiAccess::new(self.amidi_change.clone()));
        }
        self.amidi.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        self.os.media.amidi().lock().unwrap().create_midi_input()
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput(self.os.media.amidi())))
    }
    
    fn midi_reset(&mut self) {
    }
    
    fn use_midi_inputs(&mut self, _ports: &[MidiPortId]) {
    }
    
    fn use_midi_outputs(&mut self, _ports: &[MidiPortId]) {
    }
    
    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.aaudio().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.aaudio().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output_box(&mut self, index:usize, f: AudioOutputFn) {
        *self.os.media.aaudio().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(f);
    }
    
    fn audio_input_box(&mut self, index:usize, f: AudioInputFn) {
        *self.os.media.aaudio().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(f);
    }

    fn video_input_box(&mut self, _index: usize, _f: VideoInputFn) {
    }
    
    fn use_video_input(&mut self, _inputs: &[(VideoInputId, VideoFormatId)]) {
    }
}



