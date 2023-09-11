
use {
    std::sync::{Arc, Mutex},
    self::super::{
        android_audio::*,
        /*android_jni::*,*/
        android_midi::*,
        android_camera::*,
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
    pub (crate) android_audio_change: Signal,
    pub (crate) android_audio: Option<Arc<Mutex<AndroidAudioAccess >> >,
    pub (crate) android_midi_change: Signal,
    pub (crate) android_midi: Option<Arc<Mutex<AndroidMidiAccess >> >,
    pub (crate) android_camera_change: Signal,
    pub (crate) android_camera: Option<Arc<Mutex<AndroidCameraAccess >> >,
    
}

impl Cx {
    pub (crate) fn _handle_media_signals(&mut self/*, to_java: &AndroidToJava*/) {
        if self.os.media.android_audio_change.check_and_clear() {
            let descs = self.os.media.android_audio().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent {
                descs
            }));
        }
        if self.os.media.android_midi_change.check_and_clear() {
            let descs = self.os.media.android_midi().lock().unwrap().get_updated_descs();
            if let Some(descs) = descs{
                self.call_event_handler(&Event::MidiPorts(MidiPortsEvent {
                    descs,
                }));
            }
        }
        if self.os.media.android_camera_change.check_and_clear(){
            let descs = self.os.media.android_camera().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::VideoInputs(VideoInputsEvent{
                descs
            }));
        }
    }
    
    pub fn reinitialise_media(&mut self){
        // lets reinitialize cameras/midi/etc
        if self.os.media.android_audio.is_some(){
            self.os.media.android_audio_change.set();
        }
        if self.os.media.android_midi.is_some(){
            self.os.media.android_midi_change.set();
        }
        if self.os.media.android_camera.is_some(){
            self.os.media.android_camera_change.set();
        }
    }
}

impl CxAndroidMedia {
    pub fn android_audio(&mut self) -> Arc<Mutex<AndroidAudioAccess >> {
        if self.android_audio.is_none() {
            self.android_audio = Some(AndroidAudioAccess::new(self.android_audio_change.clone()));
        }
        self.android_audio.as_ref().unwrap().clone()
    }
    pub fn android_midi(&mut self) -> Arc<Mutex<AndroidMidiAccess >> {
        if self.android_midi.is_none() {
            self.android_midi = Some(AndroidMidiAccess::new(self.android_midi_change.clone()));
        }
        self.android_midi.as_ref().unwrap().clone()
    }
    pub fn android_camera(&mut self) -> Arc<Mutex<AndroidCameraAccess >> {
        if self.android_camera.is_none() {
            self.android_camera = Some(AndroidCameraAccess::new(self.android_camera_change.clone()));
        }
        self.android_camera.as_ref().unwrap().clone()
    }
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        let amidi = self.os.media.android_midi().clone();
        self.os.media.android_midi().lock().unwrap().create_midi_input(amidi)
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput {
            amidi: self.os.media.android_midi()
        }))
    }
    
    fn midi_reset(&mut self) {
    }
    
    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
       self.os.media.android_midi().lock().unwrap().use_midi_inputs(ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
       self.os.media.android_midi().lock().unwrap().use_midi_outputs(ports);
    }
    
    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.android_audio().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.android_audio().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output_box(&mut self, index: usize, f: AudioOutputFn) {
        *self.os.media.android_audio().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(f);
    }
    
    fn audio_input_box(&mut self, index: usize, f: AudioInputFn) {
        *self.os.media.android_audio().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(f);
    }
    
    fn video_input_box(&mut self, index: usize, f: VideoInputFn) {
        *self.os.media.android_camera().lock().unwrap().video_input_cb[index].lock().unwrap() = Some(f);
    }
    
    fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        self.os.media.android_camera().lock().unwrap().use_video_input(inputs);
    }
}



