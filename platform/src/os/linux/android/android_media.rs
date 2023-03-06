
use {
    std::sync::{Arc, Mutex},
    self::super::{
        aaudio::*,
        android_jni::*,
        amidi::*,
        acamera::*,
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
    pub (crate) acamera_change: Signal,
    pub (crate) acamera: Option<Arc<Mutex<ACameraAccess >> >,
    
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
            let descs = self.os.media.amidi().lock().unwrap().get_updated_descs(to_java);
            if let Some(descs) = descs{
                self.call_event_handler(&Event::MidiPorts(MidiPortsEvent {
                    descs,
                }));
            }
        }
        if self.os.media.acamera_change.check_and_clear(){
            let descs = self.os.media.acamera().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::VideoInputs(VideoInputsEvent{
                descs
            }));
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
    pub fn acamera(&mut self) -> Arc<Mutex<ACameraAccess >> {
        if self.acamera.is_none() {
            self.acamera = Some(ACameraAccess::new(self.acamera_change.clone()));
        }
        self.acamera.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        let amidi = self.os.media.amidi().clone();
        self.os.media.amidi().lock().unwrap().create_midi_input(amidi)
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput {
            amidi: self.os.media.amidi()
        }))
    }
    
    fn midi_reset(&mut self) {
    }
    
    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
       self.os.media.amidi().lock().unwrap().use_midi_inputs(ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
       self.os.media.amidi().lock().unwrap().use_midi_outputs(ports);
    }
    
    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.aaudio().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.aaudio().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        *self.os.media.aaudio().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(f);
    }
    
    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        *self.os.media.aaudio().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(f);
    }
    
    fn video_input_box(&mut self, index: usize, f: VideoInputFn) {
        *self.os.media.acamera().lock().unwrap().video_input_cb[index].lock().unwrap() = Some(f);
    }
    
    fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        self.os.media.acamera().lock().unwrap().use_video_input(inputs);
    }
}



