use {
    std::sync::{Arc, Mutex},
    std::os::raw::{c_void, c_char},
    std::ffi::CStr,
    crate::{
        makepad_live_id::*,
        thread::Signal,
        audio::*,
        os::linux::libc_sys,
        os::linux::alsa_sys::*
    }
};

struct AlsaDevice {
    name: String,
    desc: AudioDeviceDesc,
}

pub struct AlsaAccess {
    pub audio_input_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, AudioBuffer) -> AudioBuffer + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<Box<dyn FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static >> > >; MAX_AUDIO_DEVICE_INDEX],
    devices: Vec<AlsaDevice>,
}

#[derive(Debug)]
struct AlsaError(String);

impl AlsaError {
    pub fn from(prefix: &str, err: i32) -> Result<(), Self> {
        if err < 0 {
            Err(AlsaError(format!("{} - {}", prefix, unsafe {CStr::from_ptr(snd_strerror(err)).to_str().unwrap().to_string()})))
        }
        else {
            Ok(())
        }
    }
}

impl AlsaAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        change_signal.set();
        Arc::new(Mutex::new(
            AlsaAccess {
                audio_input_cb: Default::default(),
                audio_output_cb: Default::default(),
                devices: Default::default(),
            }
        ))
    }
    
    pub fn update_device_list(&mut self) {
        // alright lets do it
        fn inner() -> Result<Vec<AlsaDevice>, AlsaError> {
            let mut devices = Vec::new();
            let mut card_num = -1;
            unsafe {
                loop {
                    AlsaError::from("snd_card_next", snd_card_next(&mut card_num)) ?;
                    if card_num <0 {
                        break;
                    }
                    
                    let mut hints: *mut *mut c_void = 0 as * mut _;
                    AlsaError::from("snd_device_name_hint", snd_device_name_hint(card_num, "pcm\0".as_ptr(), &mut hints))?;
                    
                    fn from_alloc_string(s: *mut c_char) -> Option<String> {
                        if s.is_null() { return None };
                        unsafe{
                            let c = CStr::from_ptr(s).to_str().unwrap().to_string();
                            libc_sys::free(s as *mut c_void); 
                            Some(c)
                        }
                    }
                    
                    let mut index = 0;
                    while *hints.offset(index) != std::ptr::null_mut(){
                        let hint_ptr = *hints.offset(index);
                        let name_str = from_alloc_string(snd_device_name_get_hint(hint_ptr, "NAME\0".as_ptr())).unwrap_or("".into());
                        let desc_str = from_alloc_string(snd_device_name_get_hint(hint_ptr, "DESC\0".as_ptr())).unwrap_or("".into()).replace("\n"," ");
                        let ioid = from_alloc_string(snd_device_name_get_hint(hint_ptr, "IOID\0".as_ptr())).unwrap_or("".into());
                        let device_id = AudioDeviceId(LiveId::from_str_unchecked(&name_str));
                        let desc = AudioDeviceDesc {
                            device_id,
                            device_type: AudioDeviceType::Input,
                            is_default: name_str.starts_with("plughw:"),
                            channels: 2,
                            name: desc_str
                        };
                        if ioid == "" || ioid == "Input"{
                            devices.push(AlsaDevice{
                                name: name_str.clone(),
                                desc:desc.clone()
                            });
                        }
                        if ioid == "" || ioid == "Output"{
                            devices.push(AlsaDevice{
                                name: name_str,
                                desc:AudioDeviceDesc{device_type: AudioDeviceType::Output,..desc}
                            });
                        }
                        index += 1;
                    }
                }
            }
            Ok(devices)
        }
        self.devices.clear();
        match inner() {
            Err(e) => {
                println!("ALSA ERROR {}", e.0)
            }
            Ok(devices) => {
                self.devices = devices;
            }
        }
    }
    
    pub fn get_descs(&self) -> Vec<AudioDeviceDesc> {
        let mut out = Vec::new();
        for dev in &self.devices{
            out.push(dev.desc.clone());
        }
        out
    }
    
    pub fn use_audio_inputs(&mut self, _devices: &[AudioDeviceId]) {
        /*let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    //v.signal_termination();
                    // terminate
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
        for (index, device_id) in new {
            let audio_input_cb = self.audio_input_cb[index].clone();
            let audio_inputs = self.audio_inputs.clone();
            
            std::thread::spawn(move || {
                let mut wasapi = WasapiInput::new(device_id, 2);
                audio_inputs.lock().unwrap().push(wasapi.base.get_ref());
                while let Ok(buffer) = wasapi.wait_for_buffer() {
                    if audio_inputs.lock().unwrap().iter().find( | v | v.device_id == device_id && v.is_terminated).is_some() {
                        break;
                    }
                    if let Some(fbox) = &mut *audio_input_cb.lock().unwrap() {
                        let ret_buffer = fbox(
                            AudioInfo {
                                device_id,
                                time: None
                            },
                            buffer
                        );
                        wasapi.release_buffer(ret_buffer);
                    }
                    else {
                        wasapi.release_buffer(buffer);
                    }
                }
                let mut audio_inputs = audio_inputs.lock().unwrap();
                audio_inputs.retain( | v | v.device_id != device_id);
            });
        }*/
    }
    
    pub fn use_audio_outputs(&mut self, _devices: &[AudioDeviceId]) {
        /*let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    v.signal_termination();
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
        for (index, device_id) in new {
            let audio_output_cb = self.audio_output_cb[index].clone();
            let audio_outputs = self.audio_outputs.clone();
            
            std::thread::spawn(move || {
                let mut wasapi = WasapiOutput::new(device_id, 2);
                audio_outputs.lock().unwrap().push(wasapi.base.get_ref());
                while let Ok(mut buffer) = wasapi.wait_for_buffer() {
                    if audio_outputs.lock().unwrap().iter().find( | v | v.device_id == device_id && v.is_terminated).is_some() {
                        break;
                    }
                    if let Some(fbox) = &mut *audio_output_cb.lock().unwrap() {
                        fbox(
                            AudioInfo {
                                device_id,
                                time: None,
                            },
                            &mut buffer.audio_buffer
                        );
                        wasapi.release_buffer(buffer);
                    }
                    else {
                        wasapi.release_buffer(buffer);
                    }
                }
                let mut audio_outputs = audio_outputs.lock().unwrap();
                audio_outputs.retain( | v | v.device_id != device_id);
            });
        }-*/
    }
}
