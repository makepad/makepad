
use {
    std::sync::{mpsc, Arc, Mutex},
    crate::{
        cx::Cx,
        event::Event,
        thread::Signal,
        os::linux::alsa_audio::AlsaAudioAccess,
        audio::*,
        midi::*,
        video::*,
        media_api::CxMediaApi,
    }
};


impl Cx {
    pub (crate) fn handle_media_signals(&mut self) {
        if self.os.media.alsa_audio_change.check_and_clear(){
            let descs = {
                let alsa = self.os.media.alsa_audio();
                let mut alsa = alsa.lock().unwrap();
                alsa.update_device_list();
                alsa.get_descs()
            };
            self.call_event_handler(&Event::AudioDevices(AudioDevicesEvent{
                descs
            }));
        }
    }
    
}

#[derive(Default)]
pub struct CxLinuxMedia{
    pub (crate) alsa_audio: Option<Arc<Mutex<AlsaAudioAccess >> >,
    pub (crate) alsa_audio_change: Signal,
}

impl CxLinuxMedia {

    pub fn alsa_audio(&mut self) -> Arc<Mutex<AlsaAudioAccess >> {
        if self.alsa_audio.is_none() {
            self.alsa_audio = Some(AlsaAudioAccess::new(self.alsa_audio_change.clone()));
        }
        self.alsa_audio.as_ref().unwrap().clone()
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

    fn use_midi_inputs(&mut self, _ports: &[MidiPortId]) {
    }
    
    fn use_midi_outputs(&mut self, _ports: &[MidiPortId]) {
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



