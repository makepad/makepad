use {
    std::sync::{Arc, Mutex},
    self::super::{
        alsa_audio::AlsaAudioAccess
    },
    crate::{
        thread::Signal,
        audio::*,
    }
};
 
struct PulseAudioDesc {
    _name: String,
    desc: AudioDeviceDesc,
}
/*
struct PulseAudioDevice {
}
*/

struct AlsaAudioDeviceRef {
    device_id: AudioDeviceId,
    _is_terminated: bool,
}

pub struct PulseAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    audio_outputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    audio_inputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    device_descs: Vec<PulseAudioDesc>,
}

impl PulseAudioAccess {
    pub fn new(_change_signal: Signal, alsa_audio: &AlsaAudioAccess) -> Arc<Mutex<Self >> {
        Arc::new(Mutex::new(
            PulseAudioAccess {
                audio_input_cb: alsa_audio.audio_input_cb.clone(),
                audio_output_cb: alsa_audio.audio_output_cb.clone(),
                device_descs: Default::default(),
                audio_inputs: Default::default(),
                audio_outputs: Default::default(),
            }
        ))
    }
    
    pub fn get_updated_descs(&mut self)-> Vec<AudioDeviceDesc> {
        let mut out = Vec::new();
        for dev in &self.device_descs {
            out.push(dev.desc.clone());
        }
        out
    }
    
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    //v.is_terminated = true;
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    new.push((index, *device_id))
                }
            }
            new
        };
        for (_index, _device_id) in new {
           
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                   // v.is_terminated = true;
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_outputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    new.push((index, *device_id))
                }
            }
            new
            
        };
        for (index, _device_id) in new {
            let _audio_output_cb = self.audio_output_cb[index].clone();
            let _audio_outputs = self.audio_outputs.clone();
        }
    }
}
