
use {
    std::sync::{mpsc, Arc, Mutex},
    crate::{
        cx::Cx,
        event::Event,
        thread::Signal,
        os::linux::alsa_audio::AlsaAudioAccess,
        os::linux::alsa_midi::AlsaMidiAccess,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};

impl Cx {
    pub (crate) fn handle_media_signals(&mut self) {
        if self.os.media.alsa_audio_change.check_and_clear(){
            let descs = self.os.media.alsa_audio().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent{
                descs
            }));
        }
        if self.os.media.alsa_midi_change.check_and_clear(){
            let descs = self.os.media.alsa_midi().lock().unwrap().get_updated_descs();
            self.call_event_handler(&Event::MidiPorts(MidiPortsEvent{
                descs,
            }));
        }
    }
}

#[derive(Default)]
pub struct CxLinuxMedia{
    pub (crate) alsa_audio: Option<Arc<Mutex<AlsaAudioAccess >> >,
    pub (crate) alsa_audio_change: Signal,
    pub (crate) alsa_midi: Option<Arc<Mutex<AlsaMidiAccess >> >,
    pub (crate) alsa_midi_change: Signal,
}

impl CxLinuxMedia {

    pub fn alsa_audio(&mut self) -> Arc<Mutex<AlsaAudioAccess >> {
        if self.alsa_audio.is_none() {
            self.alsa_audio = Some(AlsaAudioAccess::new(self.alsa_audio_change.clone()));
        }
        self.alsa_audio.as_ref().unwrap().clone()
    }

    pub fn alsa_midi(&mut self) -> Arc<Mutex<AlsaMidiAccess >> {
        if self.alsa_midi.is_none() {
            self.alsa_midi = Some(AlsaMidiAccess::new(self.alsa_midi_change.clone()));
        }
        self.alsa_midi.as_ref().unwrap().clone()
    }        
}

pub struct OsMidiOutput();

impl OsMidiOutput{
    pub fn send(&self, _port: Option<MidiPortId>, _data: MidiData){
    }
}

impl CxMediaApi for Cx {
    
    fn midi_input(&mut self) -> MidiInput {
        let (_send, recv) = mpsc::channel();
        MidiInput(Some(recv))
    }
    
    fn midi_output(&mut self)->MidiOutput{
        MidiOutput(Some(OsMidiOutput()))
    }

    fn midi_reset(&mut self){
    }

    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.os.media.alsa_midi().lock().unwrap().use_midi_inputs(ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.os.media.alsa_midi().lock().unwrap().use_midi_outputs(ports);
    }

    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.alsa_audio().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.os.media.alsa_audio().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output<F>(&mut self, index:usize, f: F) where F: FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static {
        *self.os.media.alsa_audio().lock().unwrap().audio_output_cb[index].lock().unwrap() = Some(Box::new(f));
    }
    
    fn audio_input<F>(&mut self, index:usize, f: F)
    where F: FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static {
        *self.os.media.alsa_audio().lock().unwrap().audio_input_cb[index].lock().unwrap() = Some(Box::new(f));
    }
    
    fn video_input<F>(&mut self, _index:usize, _f: F)
    where F: FnMut(VideoFrame) + Send + 'static {
    }

    fn use_video_input(&mut self, _inputs:&[(VideoInputId, VideoFormatId)]){
    }
}



