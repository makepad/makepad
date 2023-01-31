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
 
struct AlsaAudioDesc {
    name: String,
    desc: AudioDeviceDesc,
}

struct AlsaAudioDevice {
    device_handle: *mut snd_pcm_t,
    channel_count: usize,
    frame_count: usize,
    interleaved: Vec<f32>,
    _buffer_size: usize,
}

struct AlsaAudioDeviceRef {
    device_id: AudioDeviceId,
    is_terminated: bool,
}

pub struct AlsaAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn> > >; MAX_AUDIO_DEVICE_INDEX],
    audio_outputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    audio_inputs: Arc<Mutex<Vec<AlsaAudioDeviceRef >> >,
    device_descs: Vec<AlsaAudioDesc>,
}

#[derive(Debug)]
pub struct AlsaError(String);

macro_rules!alsa_error {
    ( $ call: expr) => {
        AlsaError::from(stringify!( $ call), $ call)
    }
}

impl AlsaAudioAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self >> {
        std::thread::spawn(move || {
            let mut last_card_count = 0;
            loop {
                let mut card_count = 0;
                let mut card_num = -1;
                loop {
                    unsafe{snd_card_next(&mut card_num);}
                    if card_num <0 {
                        break;
                    }
                    card_count += 1;
                }
                if card_count != last_card_count{
                    last_card_count = card_count;
                    change_signal.set();
                }
                let _ = std::thread::sleep(std::time::Duration::new(1, 0));
            }
        });
        
        Arc::new(Mutex::new(
            AlsaAudioAccess {
                audio_input_cb: Default::default(),
                audio_output_cb: Default::default(),
                device_descs: Default::default(),
                audio_inputs: Default::default(),
                audio_outputs: Default::default(),
            }
        ))
    }
    
    pub fn get_updated_descs(&mut self)-> Vec<AudioDeviceDesc> {
        // alright lets do it
        fn inner() -> Result<Vec<AlsaAudioDesc>, AlsaError> {
            let mut device_descs = Vec::new();
            let mut card_num = -1;
            unsafe {
                loop {
                    alsa_error!(snd_card_next(&mut card_num)) ?;
                    if card_num <0 {
                        break;
                    }
                    
                    let mut hints: *mut *mut c_void = 0 as *mut _;
                    alsa_error!(snd_device_name_hint(card_num, "pcm\0".as_ptr(), &mut hints)) ?;
                    
                    
                    let mut index = 0;
                    while *hints.offset(index) != std::ptr::null_mut() {
                        let hint_ptr = *hints.offset(index);
                        let name_str = from_alsa_string(snd_device_name_get_hint(hint_ptr, "NAME\0".as_ptr())).unwrap_or("".into());
                        let desc_str = from_alsa_string(snd_device_name_get_hint(hint_ptr, "DESC\0".as_ptr())).unwrap_or("".into()).replace("\n", " ");
                        let ioid = from_alsa_string(snd_device_name_get_hint(hint_ptr, "IOID\0".as_ptr())).unwrap_or("".into());
                        let device_id = AudioDeviceId(LiveId::from_str_unchecked(&name_str));
                        let desc = AudioDeviceDesc {
                            device_id,
                            device_type: AudioDeviceType::Input,
                            is_default: false,
                            channels: 2,
                            name: format!("[ALSA] {}",desc_str)
                        };
                        if ioid == "" || ioid == "Input" {
                            device_descs.push(AlsaAudioDesc {
                                name: name_str.clone(),
                                desc: desc.clone()
                            });
                        }
                        if ioid == "" || ioid == "Output" {
                            device_descs.push(AlsaAudioDesc {
                                name: name_str,
                                desc: AudioDeviceDesc {device_type: AudioDeviceType::Output, ..desc}
                            });
                        }
                        index += 1;
                    }
                }
            }
            Ok(device_descs)
        }
        self.device_descs.clear();
        match inner() {
            Err(e) => {
                println!("ALSA ERROR {}", e.0)
            }
            Ok(descs) => {
                // pick a single default device
                /*
                if let Some(descs) = descs.iter_mut().find( | v | v.desc.device_type.is_output() && v.name.starts_with("plughw:")) {
                    descs.desc.is_default = true;
                }
                else if let Some(descs) = descs.iter_mut().find( | v | v.desc.device_type.is_output() && v.name.starts_with("dmix:")) {
                    descs.desc.is_default = true;
                }
                else if let Some(descs) = descs.iter_mut().find( | v | v.desc.device_type.is_output()) {
                    descs.desc.is_default = true;
                }
                if let Some(descs) = descs.iter_mut().find( | v | v.desc.device_type.is_input() && v.name.starts_with("plughw:")) {
                    descs.desc.is_default = true;
                }
                else if let Some(descs) = descs.iter_mut().find( | v | v.desc.device_type.is_input() && v.name.starts_with("dmix:")) {
                    descs.desc.is_default = true;
                }
                else if let Some(descs) = descs.iter_mut().find( | v | v.desc.device_type.is_input()) {
                    descs.desc.is_default = true;
                }*/
                
                self.device_descs = descs;
            }
        }
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
                    v.is_terminated = true;
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    if let Some(v) = self.device_descs.iter().find( | v | v.desc.device_id == *device_id){
                        new.push((index, *device_id, v.name.clone()))
                    }
                }
            }
            new
        };
        for (index, device_id, name) in new {
            let audio_input_cb = self.audio_input_cb[index].clone();
            let audio_inputs = self.audio_inputs.clone();
            std::thread::spawn(move || {
                let (mut device, device_ref) = AlsaAudioDevice::new(&name, device_id, SND_PCM_STREAM_CAPTURE).unwrap();
                audio_inputs.lock().unwrap().push(device_ref);
                let mut audio_buffer = device.allocate_matching_buffer();
                loop {
                    if audio_inputs.lock().unwrap().iter().find( | v | v.device_id == device_id && v.is_terminated).is_some() {
                        break;
                    }
                    match device.read_input_buffer(&mut audio_buffer) {
                        Err(e) => {
                            println!("Write output buffer error {}", e.0);
                            break;
                        }
                        Ok(_) => ()
                    }
                    if let Some(fbox) = &mut *audio_input_cb.lock().unwrap() {
                        audio_buffer = fbox(
                            AudioInfo {
                                device_id,
                                time: None,
                            },
                            audio_buffer
                        );
                    }
                }
                let mut audio_inputs = audio_inputs.lock().unwrap();
                audio_inputs.retain( | v | v.device_id != device_id);
            });
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.iter_mut().for_each( | v | {
                if !devices.contains(&v.device_id) {
                    v.is_terminated = true;
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_outputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    if let Some(v) = self.device_descs.iter().find( | v | v.desc.device_id == *device_id){
                        new.push((index, *device_id, v.name.clone()))
                    }
                }
            }
            new
            
        };
        for (index, device_id, name) in new {
            let audio_output_cb = self.audio_output_cb[index].clone();
            let audio_outputs = self.audio_outputs.clone();
            std::thread::spawn(move || {
                
                let (mut device, device_ref) = AlsaAudioDevice::new(&name, device_id, SND_PCM_STREAM_PLAYBACK).expect("Alsa device failure ");
                audio_outputs.lock().unwrap().push(device_ref);
                // lets allocate an output buffer
                let mut audio_buffer = device.allocate_matching_buffer();
                loop {
                    if audio_outputs.lock().unwrap().iter().find( | v | v.device_id == device_id && v.is_terminated).is_some() {
                        break;
                    }
                    if let Some(fbox) = &mut *audio_output_cb.lock().unwrap() {
                        fbox(
                            AudioInfo {
                                device_id,
                                time: None,
                            },
                            &mut audio_buffer
                        );
                    }
                    match device.write_output_buffer(&audio_buffer) {
                        Err(e) => {
                            println!("Write output buffer error {}", e.0);
                            break;
                        }
                        Ok(_) => ()
                    }
                }
                
                audio_outputs.lock().unwrap().retain( | v | v.device_id != device_id);
            });
        }
    }
}


impl AlsaAudioDevice {
    fn new(device_name: &str, device_id: AudioDeviceId, direction: snd_pcm_stream_t) -> Result<(AlsaAudioDevice, AlsaAudioDeviceRef),
        AlsaError> {
        unsafe {
            let mut handle: *mut snd_pcm_t = 0 as *mut _;
            let mut hw_params: *mut snd_pcm_hw_params_t = 0 as *mut _;
            let name0 = format!("{}\0", device_name);
            let mut rate = 48000;
            println!("### Opening ALSA Device {} ###", name0);
            alsa_error!(snd_pcm_open(&mut handle, name0.as_ptr(), direction, 0)) ?;
            alsa_error!(snd_pcm_hw_params_malloc(&mut hw_params)) ?;
            alsa_error!(snd_pcm_hw_params_any(handle, hw_params)) ?;
            alsa_error!(snd_pcm_hw_params_set_access(handle, hw_params, SND_PCM_ACCESS_RW_INTERLEAVED)) ?;
            alsa_error!(snd_pcm_hw_params_set_format(handle, hw_params, SND_PCM_FORMAT_FLOAT_LE)) ?;
            alsa_error!(snd_pcm_hw_params_set_rate_near(handle, hw_params, &mut rate, 0 as *mut _)) ?;
            alsa_error!(snd_pcm_hw_params_set_channels(handle, hw_params, 2)) ?;
            let mut periods = 2;
            let mut dir = 0;
            alsa_error!(snd_pcm_hw_params_set_periods_near(handle, hw_params, &mut periods, &mut dir)) ?;
            let mut buffer_size = 512;
            alsa_error!(snd_pcm_hw_params_set_buffer_size_near(handle, hw_params, &mut buffer_size)) ?;
            alsa_error!(snd_pcm_hw_params(handle, hw_params)) ?;
            alsa_error!(snd_pcm_hw_params_set_rate_resample(handle, hw_params, 1)) ?;
            let mut buffer_size = 0;
            alsa_error!(snd_pcm_hw_params_get_buffer_size(hw_params, &mut buffer_size)) ?;
            let mut channel_count = 0;
            alsa_error!(snd_pcm_hw_params_get_channels(hw_params, &mut channel_count)) ?;
            let mut frame_count = 0;
            alsa_error!(snd_pcm_hw_params_get_period_size(hw_params, &mut frame_count, 0 as *mut _)) ?;
            snd_pcm_hw_params_free(hw_params);
            
            // alright device is prepared.
            Ok((Self {
                interleaved: {let mut n = Vec::new(); n.resize(frame_count as usize * channel_count as usize, 0.0); n},
                device_handle: handle,
                channel_count: channel_count as usize,
                frame_count: frame_count as usize,
                _buffer_size: buffer_size as usize,
            }, AlsaAudioDeviceRef {
                device_id,
                is_terminated: false,
            }))
        }
    }
    
    fn allocate_matching_buffer(&self) -> AudioBuffer {
        AudioBuffer::new_with_size(self.frame_count, self.channel_count)
    }
    
    fn write_output_buffer(&mut self, buffer: &AudioBuffer) -> Result<i32, AlsaError> {
        unsafe {
            // interleave the audio buffer
            let data = &buffer.data;
            for i in 0..self.frame_count {
                self.interleaved[i * 2] = data[i];
                self.interleaved[i * 2 + 1] = data[i + self.frame_count];
            }
            let result = snd_pcm_writei(self.device_handle, self.interleaved.as_ptr() as *mut _, self.frame_count as _);
            if result == -libc_sys::EPIPE as _ {
                snd_pcm_prepare(self.device_handle);
                return Ok(0)
            }
            //println!("buffer {:?}", buffer.data.as_ptr());
            AlsaError::from("snd_pcm_writei", result as _)
        }
    }
    
    fn read_input_buffer(&mut self, buffer: &mut AudioBuffer) -> Result<i32, AlsaError> {
        buffer.resize(self.frame_count, self.channel_count);
        unsafe {
            // interleave the audio buffer
            let result = snd_pcm_readi(self.device_handle, self.interleaved.as_ptr() as *mut _, self.frame_count as _);
            if result == -libc_sys::EPIPE as _ {
                snd_pcm_prepare(self.device_handle);
                return Ok(0)
            }
            for i in 0..self.frame_count {
                buffer.data[i] = self.interleaved[i * 2];
                buffer.data[i + self.frame_count] = self.interleaved[i * 2 + 1];
            }
            //println!("buffer {:?}", buffer.data.as_ptr());
            AlsaError::from("snd_pcm_writei", result as _)
        }
    }
}


impl AlsaError {
    pub fn from(prefix: &str, err: i32) -> Result<i32, Self> {
        if err < 0 {
            Err(AlsaError(format!("{} - {}", prefix, unsafe {CStr::from_ptr(snd_strerror(err)).to_str().unwrap().to_string()})))
        }
        else {
            Ok(err)
        }
    }
}

fn from_alsa_string(s: *mut c_char) -> Option<String> {
    if s.is_null() {return None};
    unsafe {
        let c = CStr::from_ptr(s).to_str().unwrap().to_string();
        libc_sys::free(s as *mut c_void);
        Some(c)
    }
}

